// SPDX-License-Identifier: MPL-2.0
//! Append-only wire log, one line per message.

mod files;
mod render;

use std::ffi::OsString;
use std::fmt;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use chrono::Local;
use serde_json::Value;

pub use files::{newest_log, target};
use render::render;

#[derive(Debug, Clone, Copy)]
pub enum Dir {
    ClientToAgent,
    AgentToClient,
    PonsToClient,
    Pons,
}

impl fmt::Display for Dir {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::ClientToAgent => "client->agent",
            Self::AgentToClient => "agent->client",
            Self::PonsToClient => "pons->client",
            Self::Pons => "pons",
        })
    }
}

/// How much of each message the wire log keeps.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, clap::ValueEnum)]
pub enum LogLevel {
    /// Every message verbatim.
    All,
    /// Every message, but `session/update` notifications from the agent (and
    /// pons' folded ones) shrink to a one-line summary.
    #[default]
    Default,
    /// Only pons' own decisions plus method names.
    Meta,
}

/// Cheap to clone; a no-op when no log file is configured.
#[derive(Debug, Clone, Default)]
pub struct Logger {
    inner: Option<Arc<Mutex<Sink>>>,
    level: LogLevel,
}

#[derive(Debug)]
struct Sink {
    file: Option<File>,
    path: PathBuf,
    size: u64,
    /// 0 means unlimited.
    max_bytes: u64,
}

impl Sink {
    fn write(&mut self, entry: &[u8]) {
        let len = entry.len() as u64;
        if self.max_bytes > 0 && self.size > 0 && self.size + len > self.max_bytes {
            self.rotate();
        }
        if let Some(f) = &mut self.file
            && f.write_all(entry).is_ok()
        {
            self.size += len;
        }
    }

    /// Moves the current log to `<file>.1` (replacing an older one) and starts
    /// a fresh file. If the rename fails the file is truncated instead.
    fn rotate(&mut self) {
        self.file = None; // close first; Windows cannot rename an open file
        let mut rotated = self.path.clone().into_os_string();
        rotated.push(".1");
        let _ = std::fs::rename(&self.path, rotated);
        self.file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&self.path)
            .ok();
        self.size = 0;
    }
}

impl Logger {
    pub fn open(path: Option<&Path>, level: LogLevel, max_bytes: u64) -> std::io::Result<Self> {
        let inner = path
            .map(|p| -> std::io::Result<_> {
                let file = OpenOptions::new().create(true).append(true).open(p)?;
                let size = file.metadata().map_or(0, |m| m.len());
                Ok(Arc::new(Mutex::new(Sink {
                    file: Some(file),
                    path: p.to_owned(),
                    size,
                    max_bytes,
                })))
            })
            .transpose()?;
        Ok(Self { inner, level })
    }

    pub fn log(&self, dir: Dir, line: &[u8]) {
        let Some(inner) = &self.inner else { return };
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default();
        let text = String::from_utf8_lossy(line);
        let entry = format!(
            "{}.{:03} {dir} {}\n",
            ts.as_secs(),
            ts.subsec_millis(),
            render(self.level, dir, text.trim_end())
        );
        if let Ok(mut sink) = inner.lock() {
            // Logging must never take the proxy down.
            sink.write(entry.as_bytes());
        }
    }

    /// Logs a panic, then runs the previous hook. Only a panic on the main thread
    /// ends the process and is logged as `exiting: panic: <message>`; one in a
    /// spawned task is logged as `panic in task` so `doctor` does not read it as
    /// the exit. A panic the fail-safe catches is not logged.
    pub fn log_panics(&self) {
        let logger = self.clone();
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            if !crate::failsafe::is_caught() {
                let msg = crate::failsafe::panic_text(info.payload()).replace('\n', " ");
                let at = info
                    .location()
                    .map(|l| format!(" at {}:{}", l.file(), l.line()))
                    .unwrap_or_default();
                let what = if std::thread::current().name() == Some("main") {
                    "exiting: panic"
                } else {
                    "panic in task"
                };
                logger.log(Dir::Pons, format!("{what}: {msg}{at}").as_bytes());
            }
            previous(info);
        }));
    }
}

#[cfg(test)]
mod tests {
    use super::files::{default_dir, prune};

    use super::*;

    #[test]
    fn disabled_logger_is_noop() {
        Logger::default().log(Dir::Pons, b"x");
    }

    #[test]
    fn appends_direction_and_line() {
        let path = std::env::temp_dir().join(format!("pons-log-test-{}", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let logger = Logger::open(Some(&path), LogLevel::All, 0).unwrap();
        logger.log(Dir::ClientToAgent, b"{\"a\":1}");
        logger.log(Dir::PonsToClient, b"{\"b\":2}");
        let text = std::fs::read_to_string(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        let lines: Vec<_> = text.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains(" client->agent {\"a\":1}"));
        assert!(lines[1].contains(" pons->client {\"b\":2}"));
    }

    #[test]
    fn summarizes_updates_and_keeps_requests_full() {
        // Five characters, six bytes.
        let chunk = r#"{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"s1","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"hXllo"},"_meta":{"claudeCode":{"parentToolUseId":"P"}}}}}"#;
        let chunk = chunk.replace('X', &char::from_u32(0xe9).unwrap().to_string());
        let chunk = chunk.as_str();
        let out = render(LogLevel::Default, Dir::AgentToClient, chunk);
        assert_eq!(
            out,
            "session/update session=s1 kind=agent_message_chunk parentToolUseId=P textLen=5"
        );
        let tool = r#"{"method":"session/update","params":{"sessionId":"s1","update":{"sessionUpdate":"tool_call_update","toolCallId":"t","status":"completed","content":[{"type":"content","content":{"type":"text","text":"abc"}},{"type":"diff"}]}}}"#;
        let out = render(LogLevel::Default, Dir::PonsToClient, tool);
        assert_eq!(
            out,
            "session/update session=s1 kind=tool_call_update toolCallId=t status=completed textLen=3"
        );
        let usage = r#"{"method":"session/update","params":{"sessionId":"s1","update":{"sessionUpdate":"usage_update","used":1,"_meta":{"_claude/origin":{"kind":"task-notification","producer":"session-task"}}}}}"#;
        assert_eq!(
            render(LogLevel::Default, Dir::AgentToClient, usage),
            "session/update session=s1 kind=usage_update origin=task-notification textLen=0"
        );
        let req = r#"{"id":3,"method":"session/request_permission","params":{"sessionId":"s1"}}"#;
        assert_eq!(render(LogLevel::Default, Dir::AgentToClient, req), req);
        // Client messages and malformed lines are never summarized.
        assert_eq!(render(LogLevel::Default, Dir::ClientToAgent, chunk), chunk);
        assert_eq!(
            render(LogLevel::Default, Dir::AgentToClient, "session/update {"),
            "session/update {"
        );
        assert_eq!(render(LogLevel::All, Dir::AgentToClient, chunk), chunk);
    }

    #[test]
    fn meta_keeps_notes_and_method_names_only() {
        assert_eq!(
            render(LogLevel::Meta, Dir::Pons, "cancel filter fired"),
            "cancel filter fired"
        );
        let req = r#"{"id":3,"method":"session/prompt","params":{"secret":1}}"#;
        assert_eq!(
            render(LogLevel::Meta, Dir::ClientToAgent, req),
            "method=session/prompt"
        );
        let resp = r#"{"id":3,"result":{"x":1}}"#;
        assert_eq!(
            render(LogLevel::Meta, Dir::AgentToClient, resp),
            "response id=3"
        );
        assert_eq!(
            render(LogLevel::Meta, Dir::AgentToClient, "nope"),
            "unparsable"
        );
    }

    fn vars<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<OsString> + 'a {
        move |k| {
            pairs
                .iter()
                .find(|(name, _)| *name == k)
                .map(|(_, v)| OsString::from(*v))
        }
    }

    #[test]
    fn default_dir_per_platform() {
        let mac = [("HOME", "/Users/me")];
        assert_eq!(
            default_dir("macos", vars(&mac)),
            PathBuf::from("/Users/me/Library/Logs/pons")
        );
        let xdg = [("HOME", "/home/me"), ("XDG_STATE_HOME", "/state")];
        assert_eq!(
            default_dir("linux", vars(&xdg)),
            PathBuf::from("/state/pons")
        );
        let relative = [("HOME", "/home/me"), ("XDG_STATE_HOME", "rel")];
        assert_eq!(
            default_dir("linux", vars(&relative)),
            PathBuf::from("/home/me/.local/state/pons")
        );
        let win = [("LOCALAPPDATA", r"C:\Users\me\AppData\Local")];
        assert_eq!(
            default_dir("windows", vars(&win)),
            PathBuf::from(r"C:\Users\me\AppData\Local")
                .join("pons")
                .join("logs")
        );
    }

    #[test]
    fn prune_keeps_the_newest_ten_logs_and_nothing_else() {
        let dir = std::env::temp_dir().join(format!("pons-prune-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let name = |minute: u32| format!("pons-2026-10-14T22-{minute:02}-00-1.log");
        for minute in 0..12 {
            std::fs::write(dir.join(name(minute)), "x").unwrap();
        }
        let current = dir.join("pons-2026-10-14T22-12-00-2.log");
        std::fs::write(&current, "x").unwrap();
        let others = [
            "notes.txt",
            "pons-keep.log",
            "pons-x-1.log",
            "other-2026-10-14T22-00-00-1.log",
        ];
        for other in others {
            std::fs::write(dir.join(other), "x").unwrap();
        }
        prune(&dir, 10, &current);
        for minute in 0..3 {
            assert!(!dir.join(name(minute)).exists(), "minute {minute} kept");
        }
        for minute in 3..12 {
            assert!(dir.join(name(minute)).exists(), "minute {minute} removed");
        }
        assert!(current.exists());
        for other in others {
            assert!(dir.join(other).exists(), "{other} removed");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rotates_once_to_dot_one_when_over_cap() {
        let path = std::env::temp_dir().join(format!("pons-rot-test-{}", std::process::id()));
        let rotated = PathBuf::from(format!("{}.1", path.display()));
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(&rotated);
        let logger = Logger::open(Some(&path), LogLevel::All, 100).unwrap();
        for i in 0..6 {
            logger.log(
                Dir::Pons,
                format!("line-{i}-xxxxxxxxxxxxxxxxxxxx").as_bytes(),
            );
        }
        let cur = std::fs::read_to_string(&path).unwrap();
        let old = std::fs::read_to_string(&rotated).unwrap();
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(&rotated);
        assert!(
            cur.len() <= 100 && old.len() <= 100,
            "{} {}",
            cur.len(),
            old.len()
        );
        assert!(cur.contains("line-5-") && !cur.contains("line-0-"));
        assert!(old.contains("line-"));
    }

    #[test]
    fn an_uncaught_panic_is_logged_and_a_caught_one_is_not() {
        let path = std::env::temp_dir().join(format!("pons-panic-test-{}", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let logger = Logger::open(Some(&path), LogLevel::Default, 0).unwrap();
        logger.log_panics();
        let _ =
            std::panic::catch_unwind(|| crate::failsafe::catch_panic(|| panic!("caught quietly")));
        let _ = std::panic::catch_unwind(|| panic!("boom"));
        let text = std::fs::read_to_string(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        // The test thread is not `main`, so this is a task panic, not the exit.
        assert!(text.contains("panic in task: boom at "), "{text}");
        assert!(!text.contains("caught quietly"), "{text}");
    }
}
