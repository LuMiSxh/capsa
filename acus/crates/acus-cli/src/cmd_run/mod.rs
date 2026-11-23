// SPDX-License-Identifier: MPL-2.0
//! `acus run`: runs a command, drops build and test noise, keeps the failures and shows the
//! code their `path:line` references point at.

mod compute;
mod narrow;
mod noise;
mod print;
mod refs;
mod repeat;

#[cfg(feature = "script")]
pub(crate) use compute::RunReport;
pub(crate) use compute::compute;
pub(crate) use print::{json, outcome};

use crate::config::Config;
use crate::out::Out;
use anyhow::{Context, Result};
use capsa_core::exit::Outcome;
use capsa_core::hint::hinted;
use regex::{Regex, RegexSet};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

#[derive(clap::Args)]
pub struct Args {
    /// Shell command, e.g. 'cargo test -q'; stdout and stderr are merged. Default: `[run] command`,
    /// else the project's test command (Cargo.toml, Package.swift, go.mod, package.json, pytest).
    command: Option<String>,
    /// Output lines kept (head and tail halves).
    #[arg(long, default_value_t = 200)]
    max_output: usize,
    /// Code locations shown.
    #[arg(long, default_value_t = 8)]
    max_refs: usize,
    /// Lines per location.
    #[arg(long, default_value_t = 40)]
    max_lines: usize,
    /// Lines around a location outside any known symbol.
    #[arg(long, default_value_t = 3)]
    context: usize,
    /// Keep build noise (progress lines, passing tests, colour codes, repeated lines).
    #[arg(long)]
    raw: bool,
    /// Also drop output lines matching this regex (repeatable), e.g. --drop '^warning: unused'.
    #[arg(long, value_name = "REGEX")]
    drop: Vec<String>,
    /// Keep only the last N lines of the filtered output, plus every error and failure block and
    /// the code they point at; the gaps say how many lines were dropped.
    #[arg(long, value_name = "N")]
    tail: Option<usize>,
    /// Keep only output lines matching this regex, plus the code they point at. Applies after
    /// the noise filter and --drop (so --raw to match noise), before --tail.
    #[arg(long, value_name = "REGEX")]
    grep: Option<String>,
    /// With --grep: also keep N output lines after each match, like grep -A. Groups that do not
    /// touch are separated by `--`. The last of repeated flags wins; -A and -B override -C.
    #[arg(short = 'A', long = "after-context", value_name = "N")]
    after: Vec<usize>,
    /// With --grep: also keep N output lines before each match, like grep -B.
    #[arg(short = 'B', long = "before-context", value_name = "N")]
    before: Vec<usize>,
    /// With --grep: keep N output lines on both sides of each match, like grep -C.
    #[arg(short = 'C', value_name = "N")]
    around: Vec<usize>,
    /// Kill the command (and its children) after SECS seconds and report what it printed until
    /// then; exit code 124. For hung tests and watch-mode commands.
    #[arg(long, value_name = "SECS")]
    timeout: Option<u64>,
    /// Read FILE (a saved log, such as a background task's output) instead of running a command.
    /// The same filters apply; a note says when the file may still be growing.
    #[arg(long, value_name = "FILE")]
    log: Option<PathBuf>,
    /// Clip each printed line to N characters; a clipped line ends with `… (+N chars)`. Headers
    /// and footers stay whole.
    #[arg(long, value_name = "N", value_parser = clap::value_parser!(u64).range(1..))]
    cut: Option<u64>,
    /// Stop after N printed lines, headers and footers included; the rest is counted in a footer.
    #[arg(long, value_name = "N", value_parser = clap::value_parser!(u64).range(1..))]
    head: Option<u64>,
}

impl Args {
    /// Returns the arguments that `acus run COMMAND` parses to with every flag at its default,
    /// for a caller that already has the command.
    pub fn for_command(command: String) -> Self {
        Args {
            command: Some(command),
            max_output: 200,
            max_refs: 8,
            max_lines: 40,
            context: 3,
            raw: false,
            drop: Vec::new(),
            tail: None,
            grep: None,
            after: Vec::new(),
            before: Vec::new(),
            around: Vec::new(),
            timeout: None,
            log: None,
            cut: None,
            head: None,
        }
    }

    /// The shell command `acus run COMMAND` was given, or `None` when it runs the project's tests.
    pub(crate) fn command(&self) -> Option<&str> {
        self.command.as_deref()
    }
}

/// Exit code of a command killed by `--timeout`, as in timeout(1).
const TIMEOUT_CODE: i32 = 124;

/// The command's merged output, its exit code, and whether `--timeout` killed it.
struct Captured {
    stdout: Vec<u8>,
    code: i32,
    timed_out: bool,
}

/// Runs `cmd` and reads its output on a thread, so a chatty process cannot block on a full pipe
/// while we wait. The shell already merges stderr into stdout (`2>&1`), so there is one stream;
/// stderr is drained on a second thread only so nothing can block on it. With `timeout` the
/// command runs in its own process group and the group is killed when time is up.
/// On Windows the whole tree is killed with `taskkill`, as the group is on Unix.
fn capture(cmd: &str, command: &str, timeout: Option<u64>) -> Result<Captured> {
    let mut c = shell(cmd);
    c.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if timeout.is_some() {
        capsa_core::process::own_group(&mut c);
    }
    let mut child = c
        .spawn()
        .with_context(|| format!("cannot run `{command}`"))?;
    let (tx, rx) = mpsc::channel::<Vec<u8>>();
    let mut stdout = child.stdout.take().context("no stdout pipe")?;
    std::thread::spawn(move || {
        let mut buf = [0u8; 8192];
        while let Ok(n) = stdout.read(&mut buf) {
            if n == 0 || tx.send(buf[..n].to_vec()).is_err() {
                break;
            }
        }
    });
    if let Some(mut e) = child.stderr.take() {
        std::thread::spawn(move || {
            let _ = std::io::copy(&mut e, &mut std::io::sink());
        });
    }
    let mut data = Vec::new();
    let Some(secs) = timeout else {
        let status = child.wait()?;
        // Ends when every holder of the pipe has closed it, like `Command::output`.
        while let Ok(chunk) = rx.recv() {
            data.extend(chunk);
        }
        return Ok(Captured {
            stdout: data,
            code: status.code().unwrap_or(1),
            timed_out: false,
        });
    };
    let deadline = Instant::now() + Duration::from_secs(secs);
    let mut status = None;
    let mut open = true;
    while status.is_none() && Instant::now() < deadline {
        status = child.try_wait()?;
        // Collect while waiting, so the wait is also the poll interval. Once the reader has
        // ended (stdout closed, child still running) `recv_timeout` returns at once, so sleep.
        if open {
            match rx.recv_timeout(Duration::from_millis(20)) {
                Ok(chunk) => data.extend(chunk),
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => open = false,
            }
        } else {
            std::thread::sleep(Duration::from_millis(20));
        }
    }
    let timed_out = status.is_none();
    if timed_out {
        let _ = capsa_core::process::signal_group(child.id(), capsa_core::process::Signal::Kill);
        let _ = child.kill();
        let _ = child.wait();
    }
    // Whatever is already written; a grandchild that escaped the group must not hold us up.
    let end = Instant::now() + Duration::from_secs(if timed_out { 2 } else { 5 });
    while let Some(left) = end.checked_duration_since(Instant::now()) {
        match rx.recv_timeout(left) {
            Ok(chunk) => data.extend(chunk),
            Err(_) => break,
        }
    }
    let code = if timed_out {
        TIMEOUT_CODE
    } else {
        status.and_then(|s| s.code()).unwrap_or(1)
    };
    Ok(Captured {
        stdout: data,
        code,
        timed_out,
    })
}

/// Runs a command and prints its filtered output, then the code its `path:line` references
/// point at. Exit: the command's exit code (2 if it could not start).
pub fn run(a: &Args, out: &Out, cfg: &Config) -> Result<Outcome> {
    let report = compute::compute(a, out, cfg)?;
    Ok(print::print(&report, out))
}

/// The compiled drop, grep and context inputs of one run.
type Inputs = (RegexSet, Option<Regex>, Option<(usize, usize)>);

/// Checks the flag combinations, then compiles the drop, grep and context inputs.
fn inputs(a: &Args, cfg: &Config) -> Result<Inputs> {
    // Checked here, not with clap's conflicts, so the error carries the hint.
    if a.log.is_some() && (a.command.is_some() || a.timeout.is_some()) {
        return Err(hinted(
            "--log reads a saved file, so it takes no command or --timeout",
            "use either a command or --log FILE",
        ));
    }
    let extra = RegexSet::new(cfg.run.drop.iter().chain(&a.drop)).with_context(|| {
        format!(
            "invalid --drop or [run] drop regex ({})",
            cfg.path.display()
        )
    })?;
    let grep = a
        .grep
        .as_deref()
        .map(Regex::new)
        .transpose()
        .context("invalid --grep regex")?;
    let has_context = !(a.after.is_empty() && a.before.is_empty() && a.around.is_empty());
    if has_context && grep.is_none() {
        return Err(hinted(
            "-A, -B and -C need --grep",
            "add the pattern, e.g. acus run 'cargo build' --grep '^error' -A 4",
        ));
    }
    // The last of repeated flags wins, and -A and -B override -C.
    let context = has_context.then(|| {
        let around = a.around.last().copied();
        let before = a.before.last().copied().or(around).unwrap_or(0);
        let after = a.after.last().copied().or(around).unwrap_or(0);
        (before, after)
    });
    Ok((extra, grep, context))
}

/// What the command (or saved log) produced, and the command an agent view announces.
struct Source {
    text: String,
    code: i32,
    timed_out: bool,
    /// The command, `None` for a saved log.
    command: Option<String>,
    /// The command the user did not pass, which the agent view announces with `$`.
    announced: Option<String>,
}

/// The text to narrow with its exit code and timeout flag; the command is `None` for a saved
/// log.
fn source(a: &Args, cfg: &Config) -> Result<Source> {
    // A saved log is read, not run, so there is no command to save its output under.
    match &a.log {
        Some(path) => match std::fs::read(path) {
            Ok(bytes) => Ok(Source {
                text: String::from_utf8_lossy(&bytes).into_owned(),
                code: 0,
                timed_out: false,
                command: None,
                announced: None,
            }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                anyhow::bail!("{}: not found", path.display())
            }
            Err(e) => Err(e.into()),
        },
        None => {
            let (command, defaulted) = command_line(a, cfg)?;
            // The redirect must cover the whole command line, not just its last `&&` part.
            let cmd = if cfg!(windows) {
                format!("({command}) 2>&1")
            } else {
                format!("exec 2>&1\n{command}")
            };
            let res = capture(&cmd, &command, a.timeout)?;
            let text = String::from_utf8_lossy(&res.stdout).into_owned();
            let announced = defaulted.then(|| command.clone());
            Ok(Source {
                text,
                code: res.code,
                timed_out: res.timed_out,
                command: Some(command),
                announced,
            })
        }
    }
}

/// The command to run: the argument, else `[run] command`, else the detected test command; the
/// flag says the user did not pass it.
fn command_line(a: &Args, cfg: &Config) -> Result<(String, bool)> {
    match &a.command {
        Some(c) => Ok((c.clone(), false)),
        None => {
            let cwd = std::env::current_dir()?;
            let c = cfg
                .run
                .command
                .clone()
                .or_else(|| detect_command(&cwd))
                .ok_or_else(|| {
                    hinted(
                        "no command given and no project detected",
                        format!(
                            "pass a command, e.g. acus run 'cargo test -q', or set [run] command in {}; for several commands use acus batch",
                            cfg.path.display()
                        ),
                    )
                })?;
            Ok((c, true))
        }
    }
}

/// `sh -c`, or on Windows `cmd /S /C "…"` passed verbatim: Rust's argument quoting escapes `"`
/// as `\"`, which cmd does not understand, and `/S` strips exactly the outer pair of quotes.
fn shell(cmd: &str) -> Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let mut c = Command::new("cmd");
        c.args(["/S", "/C"]).raw_arg(format!("\"{cmd}\""));
        c
    }
    #[cfg(not(windows))]
    {
        let mut c = Command::new("sh");
        c.args(["-c", cmd]);
        c
    }
}

/// The test command of the nearest project root at or above `start`; one directory is checked
/// in a fixed order, so a Cargo.toml beside a package.json wins.
fn detect_command(start: &Path) -> Option<String> {
    start.ancestors().find_map(|d| {
        let has = |f: &str| d.join(f).exists();
        Some(if has("Cargo.toml") {
            "cargo test -q".into()
        } else if has("Package.swift") {
            "swift test".into()
        } else if has("go.mod") {
            "go test ./...".into()
        } else if has("package.json") {
            let pm = if has("pnpm-lock.yaml") {
                "pnpm"
            } else if has("yarn.lock") {
                "yarn"
            } else if has("bun.lock") || has("bun.lockb") {
                "bun"
            } else {
                "npm"
            };
            format!("{pm} test")
        } else if has("pyproject.toml") || has("pytest.ini") || has("setup.py") {
            "pytest -q".into()
        } else {
            return None;
        })
    })
}

#[cfg(test)]
mod tests {
    use super::detect_command;

    #[test]
    fn detects_project_command() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path();
        let sub = root.join("a/b");
        std::fs::create_dir_all(&sub).unwrap();
        let touch = |f: &str| std::fs::write(root.join(f), "").unwrap();
        touch("package.json");
        assert_eq!(detect_command(&sub).as_deref(), Some("npm test"));
        touch("yarn.lock");
        assert_eq!(detect_command(&sub).as_deref(), Some("yarn test"));
        touch("pnpm-lock.yaml");
        assert_eq!(detect_command(&sub).as_deref(), Some("pnpm test"));
        touch("pyproject.toml");
        assert_eq!(detect_command(&sub).as_deref(), Some("pnpm test"));
        touch("go.mod");
        assert_eq!(detect_command(&sub).as_deref(), Some("go test ./..."));
        touch("Cargo.toml");
        assert_eq!(detect_command(root).as_deref(), Some("cargo test -q"));
        // The nearest root wins over a farther one.
        std::fs::write(sub.join("pytest.ini"), "").unwrap();
        assert_eq!(detect_command(&sub).as_deref(), Some("pytest -q"));
    }
}
