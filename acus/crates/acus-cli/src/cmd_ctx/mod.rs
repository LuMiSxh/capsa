// SPDX-License-Identifier: MPL-2.0
//! `acus ctx`: runs a command, drops build and test noise, keeps the failures and shows the
//! code their `path:line` references point at.

mod narrow;
mod noise;
mod print;
mod refs;

use crate::config::Config;
use crate::fmt::Out;
use anyhow::{Context, Result};
use capsa_core::exit::Outcome;
use capsa_core::format::Format;
use narrow::narrow;
use noise::{clean, quiet, save_log};
use print::{print_growing, print_json, print_lines, print_notes, print_snippets};
use refs::refs;
use regex::{Regex, RegexSet};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

#[derive(clap::Args)]
pub struct Args {
    /// Shell command, e.g. 'cargo test -q'; stdout and stderr are merged. Default: `[ctx] command`,
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
/// Limitation: on Windows only the shell process is killed, not its children.
fn capture(cmd: &str, command: &str, timeout: Option<u64>) -> Result<Captured> {
    let mut c = shell(cmd);
    c.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    if timeout.is_some() {
        use std::os::unix::process::CommandExt;
        c.process_group(0);
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
        #[cfg(unix)]
        // SAFETY: a negative pid signals the process group created by `process_group(0)`.
        unsafe {
            libc::kill(-(child.id() as i32), libc::SIGKILL);
        }
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

/// Runs a command, prints its output, then the code its `path:line` references point at.
/// Exit: the command's exit code (2 if it could not start).
#[expect(
    clippy::needless_pass_by_value,
    reason = "execute hands over the parsed arguments"
)]
pub fn run(a: Args, out: &Out, cfg: &Config) -> Result<Outcome> {
    let (extra, grep, context) = inputs(&a, cfg)?;
    let narrowing = a.tail.is_some() || grep.is_some();
    let (raw, code, timed_out, command) = source(&a, out, cfg)?;
    let text = if a.raw { raw } else { clean(&raw) };
    let cwd = std::env::current_dir()?;
    if out.format == Format::Json {
        let shown = if narrowing {
            let lines = text.lines().map(str::to_owned).collect();
            narrow(
                lines,
                (a.tail, grep.as_ref(), context),
                (a.max_output, a.max_lines),
            )
            .lines
            .join("\n")
        } else {
            text
        };
        return Ok(print_json(&a, &cwd, &shown, code));
    }
    let all: Vec<&str> = text.lines().collect();
    let (lines, passed, dropped) = if a.raw {
        (all.iter().map(|l| l.to_string()).collect(), 0, 0)
    } else {
        quiet(&all, &extra)
    };
    let nothing_matched = grep
        .as_ref()
        .is_some_and(|re| !lines.is_empty() && !lines.iter().any(|l| re.is_match(l)));
    let narrowed = narrow(
        lines,
        (a.tail, grep.as_ref(), context),
        (a.max_output, a.max_lines),
    );
    // Only what is shown decides which code is shown.
    let refs = if narrowing {
        refs(&narrowed.lines.join("\n"), &cwd)
    } else {
        refs(&text, &cwd)
    };
    // A saved log opens with its header, before any of its lines, so the first line says what
    // was read.
    if let Some(path) = &a.log {
        outln!(
            "{}",
            out.header(&format!("== log {} ({} lines)", path.display(), all.len()))
        );
    }
    let omitted = print_lines(&a, &narrowed.lines);
    // Whatever is not printed stays readable without running the command again.
    // Dropped noise matters only when looking into a failure.
    let keep_log = omitted > 0
        || narrowed.grep_dropped + narrowed.tail_dropped > 0
        || timed_out
        || (dropped > 0 && code != 0);
    let log = command
        .as_deref()
        .filter(|_| keep_log)
        .and_then(|c| save_log(&cwd, c, &text));
    let log_note = log.as_deref().map(|p| (p, text.lines().count()));
    print_notes(
        out,
        dropped,
        passed,
        narrowed.grep_dropped,
        nothing_matched,
        log_note,
    );
    if let (true, Some(secs)) = (timed_out, a.timeout) {
        outln!(
            "{}",
            out.dim(&format!(
                "… killed after {secs}s (--timeout); output above is what it printed until then. Raise --timeout, or --grep/--tail a narrower command"
            ))
        );
    }
    if a.log.is_none() {
        outln!("{}", out.header(&format!("== exit {code}")));
    }
    print_snippets(out, &a, &refs);
    if let Some(path) = &a.log {
        print_growing(out, path);
    }
    Ok(Outcome::Code(code.clamp(0, 255) as u8))
}

/// The compiled drop, grep and context inputs of one run.
type Inputs = (RegexSet, Option<Regex>, Option<(usize, usize)>);

/// Checks the flag combinations, then compiles the drop, grep and context inputs.
fn inputs(a: &Args, cfg: &Config) -> Result<Inputs> {
    // Checked here, not with clap's conflicts, so the error carries the hint.
    if a.log.is_some() && (a.command.is_some() || a.timeout.is_some()) {
        return Err(capsa_core::hint::Hinted::new(
            "--log reads a saved file, so it takes no command or --timeout",
            "use either a command or --log FILE",
        )
        .into());
    }
    let extra = RegexSet::new(cfg.ctx.drop.iter().chain(&a.drop)).with_context(|| {
        format!(
            "invalid --drop or [ctx] drop regex ({})",
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
        return Err(capsa_core::hint::Hinted::new(
            "-A, -B and -C need --grep",
            "add the pattern, e.g. acus ctx 'cargo build' --grep '^error' -A 4",
        )
        .into());
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

/// The text to narrow with its exit code and timeout flag; the command is `None` for a saved
/// log.
fn source(a: &Args, out: &Out, cfg: &Config) -> Result<(String, i32, bool, Option<String>)> {
    // A saved log is read, not run, so there is no command to save its output under.
    match &a.log {
        Some(path) => match std::fs::read(path) {
            Ok(bytes) => Ok((String::from_utf8_lossy(&bytes).into_owned(), 0, false, None)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                anyhow::bail!("{}: not found", path.display())
            }
            Err(e) => Err(e.into()),
        },
        None => {
            let command = command_line(a, out, cfg)?;
            // The redirect must cover the whole command line, not just its last `&&` part.
            let cmd = if cfg!(windows) {
                format!("({command}) 2>&1")
            } else {
                format!("exec 2>&1\n{command}")
            };
            let res = capture(&cmd, &command, a.timeout)?;
            let raw = String::from_utf8_lossy(&res.stdout).into_owned();
            Ok((raw, res.code, res.timed_out, Some(command)))
        }
    }
}

/// The command to run: the argument, else `[ctx] command`, else the detected test command.
fn command_line(a: &Args, out: &Out, cfg: &Config) -> Result<String> {
    match &a.command {
        Some(c) => Ok(c.clone()),
        None => {
            let cwd = std::env::current_dir()?;
            let c = cfg
                .ctx
                .command
                .clone()
                .or_else(|| detect_command(&cwd))
                .ok_or_else(|| {
                    capsa_core::hint::Hinted::new(
                        "no command given and no project detected",
                        format!(
                            "pass a command, e.g. acus ctx 'cargo test -q', or set [ctx] command in {}",
                            cfg.path.display()
                        ),
                    )
                })?;
            if out.format != Format::Json {
                outln!("{}", out.dim(&format!("$ {c}")));
            }
            Ok(c)
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
