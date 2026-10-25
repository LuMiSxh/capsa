// SPDX-License-Identifier: MPL-2.0
//! Runs the approved commands outside the sandbox with a time limit and a capped output.

use super::repo::Recorded;
use crate::proc;
use std::collections::HashSet;
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
    /// Runs `argv` (program first) in `cwd`. With `repo`, git and the programs that call it
    /// work on that repository only (`GIT_DIR`, `GIT_WORK_TREE`) and its hooks and monitor
    /// cannot run. Errors only when the program cannot start.
    fn run(&self, cwd: &Path, argv: &[String], repo: Option<&Recorded>) -> io::Result<Ran>;
}

/// The process groups of running actions and dialogs, so that the end of the run can kill them.
#[derive(Default)]
pub struct Registry {
    state: Mutex<(bool, HashSet<u32>)>,
}

impl Registry {
    /// Records a group. Returns `false` when the registry is closed, and the caller must kill it.
    fn add(&self, pgid: u32) -> bool {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if state.0 {
            return false;
        }
        state.1.insert(pgid);
        true
    }

    fn remove(&self, pgid: u32) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        state.1.remove(&pgid);
    }

    /// Kills every recorded group and refuses new ones from now on.
    pub fn close(&self) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        state.0 = true;
        state.1.drain().for_each(proc::kill_group);
    }
}

/// Git variables that select another repository or add configuration.
const GIT_DROP: [&str; 9] = [
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_INDEX_FILE",
    "GIT_OBJECT_DIRECTORY",
    "GIT_COMMON_DIR",
    "GIT_NAMESPACE",
    "GIT_CONFIG",
    "GIT_CONFIG_PARAMETERS",
    "GIT_EXTERNAL_DIFF",
];

/// Configuration that git and gh's own git calls take before any file: no hooks, no monitor.
const GIT_FORCED: [(&str, &str); 2] =
    [("core.hooksPath", "/dev/null"), ("core.fsmonitor", "false")];

/// Runs the real programs with velum's own environment.
pub struct System {
    pub timeout: Duration,
    pub registry: Arc<Registry>,
}

impl System {
    /// Builds the command for `argv` in `cwd`.
    pub(super) fn command(
        cwd: &Path,
        argv: &[String],
        repo: Option<&Recorded>,
    ) -> io::Result<Command> {
        let (program, args) = argv
            .split_first()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "empty command"))?;
        let mut command = Command::new(program);
        command
            .args(args)
            .current_dir(cwd)
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GH_PROMPT_DISABLED", "1");
        if let Some(repo) = repo {
            GIT_DROP.iter().for_each(|name| {
                command.env_remove(name);
            });
            command.env("GIT_DIR", repo.git_dir());
            command.env("GIT_COMMON_DIR", repo.common_dir());
            command.env("GIT_WORK_TREE", &repo.root);
            command.env("GIT_CONFIG_COUNT", GIT_FORCED.len().to_string());
            for (index, (key, value)) in GIT_FORCED.iter().enumerate() {
                command.env(format!("GIT_CONFIG_KEY_{index}"), key);
                command.env(format!("GIT_CONFIG_VALUE_{index}"), value);
            }
        }
        Ok(command)
    }
}

impl Executor for System {
    fn run(&self, cwd: &Path, argv: &[String], repo: Option<&Recorded>) -> io::Result<Ran> {
        let command = Self::command(cwd, argv, repo)?;
        run_capped(command, self.timeout, MAX_OUTPUT, &self.registry)
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
pub fn run_capped(
    mut command: Command,
    timeout: Duration,
    cap: usize,
    registry: &Registry,
) -> io::Result<Ran> {
    let (reader, writer) = io::pipe()?;
    command
        .stdin(Stdio::null())
        .stdout(writer.try_clone()?)
        .stderr(writer)
        .process_group(0);
    let mut child = command.spawn()?;
    if !registry.add(child.id()) {
        proc::kill_group(child.id());
        let _ = child.wait();
        return Err(io::Error::other("the run has ended"));
    }
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
    registry.remove(child.id());
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

    fn capped(script: &str, timeout: Duration, cap: usize) -> Ran {
        run_capped(sh(script), timeout, cap, &Registry::default()).unwrap()
    }

    #[test]
    fn combines_stdout_and_stderr_and_returns_the_exit_code() {
        let ran = capped("echo out; echo err >&2; exit 3", TIMEOUT, MAX_OUTPUT);
        assert_eq!(ran.code, 3);
        assert_eq!(ran.output, "out\nerr\n");
    }

    #[test]
    fn cuts_long_output_with_a_note() {
        let ran = capped("head -c 5000 /dev/zero | tr '\\0' x", TIMEOUT, 100);
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
        let ran = capped("echo started; sleep 30", Duration::from_millis(300), 100);
        assert_eq!(ran.code, TIMED_OUT);
        assert!(ran.output.starts_with("started\n"));
        assert!(ran.output.contains("[timed out after 0 s]"));
        assert!(start.elapsed() < Duration::from_secs(10));
    }

    #[test]
    fn does_not_wait_for_a_background_process_that_holds_the_output() {
        let start = Instant::now();
        let ran = capped("sleep 30 & echo done", TIMEOUT, MAX_OUTPUT);
        assert_eq!(ran.output, "done\n");
        assert!(start.elapsed() < Duration::from_secs(10));
    }

    #[test]
    fn closing_the_registry_kills_a_running_command_and_refuses_new_ones() {
        let registry = Arc::new(Registry::default());
        let worker = {
            let registry = Arc::clone(&registry);
            std::thread::spawn(move || run_capped(sh("sleep 30"), TIMEOUT, 100, &registry))
        };
        std::thread::sleep(Duration::from_millis(300));
        let start = Instant::now();
        registry.close();
        let ran = worker.join().unwrap().unwrap();
        assert_ne!(ran.code, 0);
        assert!(start.elapsed() < Duration::from_secs(10));
        assert!(run_capped(sh("true"), TIMEOUT, 100, &registry).is_err());
    }

    #[test]
    fn a_missing_program_is_an_error() {
        let executor = System {
            timeout: TIMEOUT,
            registry: Arc::new(Registry::default()),
        };
        let argv = ["/nonexistent/program".to_string()];
        assert!(executor.run(Path::new("/"), &argv, None).is_err());
    }
}
