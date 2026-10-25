// SPDX-License-Identifier: MPL-2.0
//! Runs the approved commands outside the sandbox with a time limit and a capped output.

use crate::proc;
use std::io::{self, Read};
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

/// How long an approved command may run.
pub const TIMEOUT: Duration = Duration::from_secs(10 * 60);

/// The most output bytes returned to the agent.
pub const MAX_OUTPUT: usize = 20 * 1024;

/// The exit code reported for a command that hit the time limit.
const TIMED_OUT: i32 = 124;

/// How long to wait for the output pipe to close after the command ended. A process that left
/// its group can hold the pipe open.
const DRAIN: Duration = Duration::from_secs(1);

/// What a finished command left behind.
#[derive(Debug, PartialEq, Eq)]
pub struct Ran {
    /// The exit code; 128 plus the signal number for a signal death.
    pub code: i32,
    /// stdout and stderr in arrival order, cut at `MAX_OUTPUT` with a note.
    pub output: String,
}

/// Runs a program with arguments in a directory. The broker uses it for the approved command
/// and for the git questions that come before it; tests replace it.
pub trait Executor: Send + Sync {
    /// Runs `argv` (program first) in `cwd`. Errors only when the program cannot start.
    fn run(&self, cwd: &Path, argv: &[String]) -> io::Result<Ran>;
}

/// Runs the real programs with velum's own environment.
pub struct System {
    pub timeout: Duration,
}

impl Executor for System {
    fn run(&self, cwd: &Path, argv: &[String]) -> io::Result<Ran> {
        let (program, args) = argv
            .split_first()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "empty command"))?;
        let mut command = Command::new(program);
        command
            .args(args)
            .current_dir(cwd)
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GH_PROMPT_DISABLED", "1");
        run_capped(command, self.timeout, MAX_OUTPUT)
    }
}

/// Output kept from the pipe: the first `cap` bytes and the full count.
#[derive(Default)]
struct Captured {
    bytes: Vec<u8>,
    total: usize,
}

fn drain(mut reader: impl Read, cap: usize, shared: &Mutex<Captured>) {
    let mut chunk = [0u8; 4096];
    while let Ok(n) = reader.read(&mut chunk) {
        if n == 0 {
            break;
        }
        let mut kept = shared.lock().unwrap_or_else(PoisonError::into_inner);
        let room = cap.saturating_sub(kept.bytes.len());
        kept.bytes.extend_from_slice(&chunk[..n.min(room)]);
        kept.total += n;
    }
}

/// Runs `command` with no stdin and stdout and stderr on one pipe, in its own process group.
/// After `timeout` the group is killed; when the command ends, what is left of its group is
/// killed too. The output is cut at `cap` bytes.
pub fn run_capped(mut command: Command, timeout: Duration, cap: usize) -> io::Result<Ran> {
    let (reader, writer) = io::pipe()?;
    command
        .stdin(Stdio::null())
        .stdout(writer.try_clone()?)
        .stderr(writer)
        .process_group(0);
    let mut child = command.spawn()?;
    // The command holds the write ends until it is dropped; the reader needs them closed.
    drop(command);
    let shared = Arc::new(Mutex::new(Captured::default()));
    let thread = {
        let shared = Arc::clone(&shared);
        std::thread::spawn(move || drain(reader, cap, &shared))
    };
    let start = Instant::now();
    let (status, timed_out) = loop {
        if let Some(status) = child.try_wait()? {
            break (status, false);
        }
        if start.elapsed() >= timeout {
            proc::kill_group(child.id());
            break (child.wait()?, true);
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    proc::kill_group(child.id());
    let drained = Instant::now();
    while !thread.is_finished() && drained.elapsed() < DRAIN {
        std::thread::sleep(Duration::from_millis(10));
    }
    let kept = shared.lock().unwrap_or_else(PoisonError::into_inner);
    let mut output = String::from_utf8_lossy(&kept.bytes).into_owned();
    if kept.total > cap {
        output.push_str(&format!(
            "\n[output truncated: showing the first {cap} of {} bytes]",
            kept.total
        ));
    }
    let code = if timed_out {
        output.push_str(&format!("\n[timed out after {} s]", timeout.as_secs()));
        TIMED_OUT
    } else {
        status
            .code()
            .or_else(|| status.signal().map(|signal| 128 + signal))
            .unwrap_or(1)
    };
    Ok(Ran { code, output })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sh(script: &str) -> Command {
        let mut command = Command::new("/bin/sh");
        command.args(["-c", script]);
        command
    }

    #[test]
    fn combines_stdout_and_stderr_and_returns_the_exit_code() {
        let ran = run_capped(sh("echo out; echo err >&2; exit 3"), TIMEOUT, MAX_OUTPUT).unwrap();
        assert_eq!(ran.code, 3);
        assert_eq!(ran.output, "out\nerr\n");
    }

    #[test]
    fn cuts_long_output_with_a_note() {
        let ran = run_capped(sh("head -c 5000 /dev/zero | tr '\\0' x"), TIMEOUT, 100).unwrap();
        assert_eq!(ran.code, 0);
        assert!(ran.output.starts_with(&"x".repeat(100)));
        assert!(
            ran.output
                .ends_with("[output truncated: showing the first 100 of 5000 bytes]")
        );
    }

    #[test]
    fn kills_a_command_that_runs_past_the_limit() {
        let start = Instant::now();
        let ran = run_capped(
            sh("echo started; sleep 30"),
            Duration::from_millis(300),
            100,
        )
        .unwrap();
        assert_eq!(ran.code, TIMED_OUT);
        assert!(ran.output.starts_with("started\n"));
        assert!(ran.output.contains("[timed out after 0 s]"));
        assert!(start.elapsed() < Duration::from_secs(10));
    }

    #[test]
    fn does_not_wait_for_a_background_process_that_holds_the_output() {
        let start = Instant::now();
        let ran = run_capped(sh("sleep 30 & echo done"), TIMEOUT, MAX_OUTPUT).unwrap();
        assert_eq!(ran.output, "done\n");
        assert!(start.elapsed() < Duration::from_secs(10));
    }

    #[test]
    fn a_missing_program_is_an_error() {
        let executor = System { timeout: TIMEOUT };
        let argv = ["/nonexistent/program".to_string()];
        assert!(executor.run(Path::new("/"), &argv).is_err());
    }
}
