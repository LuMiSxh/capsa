// SPDX-License-Identifier: MPL-2.0
//! Append-only wire log, one JSON line per message, written through `capsa_core::log`.

mod files;
mod render;

use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use capsa_core::log::{Level, Writer};
use chrono::Local;
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub use files::{newest_log, target};
use render::render;
pub use render::summarize_message;

#[derive(Debug, Clone, Copy)]
pub enum Dir {
    ClientToAgent,
    AgentToClient,
    PonsToClient,
    Pons,
}

impl Dir {
    fn as_str(&self) -> &'static str {
        match self {
            Self::ClientToAgent => "client->agent",
            Self::AgentToClient => "agent->client",
            Self::PonsToClient => "pons->client",
            Self::Pons => "pons",
        }
    }
}

impl fmt::Display for Dir {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The pons fields of a wire-log line, after the envelope that `capsa_core::log` writes.
#[derive(Debug, Deserialize)]
pub struct WireFields {
    pub dir: String,
    pub text: String,
}

/// The same fields borrowed for writing, so a line copies only its text.
#[derive(Serialize)]
struct Wire<'a> {
    ms: u32,
    dir: &'a str,
    text: &'a str,
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

/// Cheap to clone; a no-op when no log file is configured. The clones share one file and
/// its buffer, so [`Logger::flush`] writes what any of them has logged.
#[derive(Clone, Default)]
pub struct Logger {
    inner: Option<Arc<Mutex<Writer>>>,
    level: LogLevel,
}

impl fmt::Debug for Logger {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Logger")
            .field("enabled", &self.inner.is_some())
            .field("level", &self.level)
            .finish()
    }
}

impl Logger {
    /// Opens the wire log at `path`, or logs nothing for `None`. `max_bytes` 0 means no cap.
    ///
    /// # Errors
    /// Fails when the file cannot be opened.
    pub fn open(path: Option<&Path>, level: LogLevel, max_bytes: u64) -> std::io::Result<Self> {
        let max_bytes = if max_bytes == 0 { u64::MAX } else { max_bytes };
        let inner = path
            .map(|p| Writer::open(p, max_bytes).map(|w| Arc::new(Mutex::new(w))))
            .transpose()?;
        Ok(Self { inner, level })
    }

    /// Tells whether agent updates are logged as summaries, which the engine can
    /// hand over (see [`Logger::log_with`]).
    pub fn wants_summaries(&self) -> bool {
        self.inner.is_some() && self.level == LogLevel::Default
    }

    pub fn log(&self, dir: Dir, line: &[u8]) {
        self.log_with(dir, line, None);
    }

    /// Like [`Logger::log`], with the summary of an agent `session/update` the engine
    /// already parsed; it is used instead of parsing the line again.
    pub fn log_with(&self, dir: Dir, line: &[u8], summary: Option<String>) {
        let Some(inner) = &self.inner else { return };
        // The ms field comes from here. The ts of the envelope is read by capsa_core::log.
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default();
        let shown = match summary {
            Some(s) if self.level == LogLevel::Default && matches!(dir, Dir::AgentToClient) => s,
            _ => render(self.level, dir, String::from_utf8_lossy(line).trim_end()),
        };
        let (level, event) = classify(dir, &shown);
        let fields = Wire {
            ms: ts.subsec_millis(),
            dir: dir.as_str(),
            text: &shown,
        };
        if let Ok(mut writer) = inner.lock() {
            // Logging must never take the proxy down.
            let _ = writer.write(level, event, &fields);
        }
    }

    /// Writes out the buffered lines. Errors are dropped, as for every write.
    pub fn flush(&self) {
        if let Some(inner) = &self.inner
            && let Ok(mut writer) = inner.lock()
        {
            let _ = writer.flush();
        }
    }

    /// Logs a panic, then runs the previous hook. Only a panic on the main thread
    /// ends the process and is logged as `exiting: panic: <message>`; one in a
    /// spawned task is logged as `panic in task` so `doctor` does not read it as
    /// the exit. A panic the fail-safe catches is not logged. The panic line is an error, so
    /// the lines still buffered before it are written with it.
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

/// Gives a line its level and event name. Wire messages are `wire` at info. The notes of pons
/// are `note`s: `fail-safe` and `exiting:` notes warn, a failed exit or a panic is an error.
fn classify(dir: Dir, text: &str) -> (Level, &'static str) {
    if !matches!(dir, Dir::Pons) {
        return (Level::Info, "wire");
    }
    let level = if text.starts_with("exiting: error")
        || text.starts_with("exiting: panic")
        || text.starts_with("panic in task")
    {
        Level::Error
    } else if text.starts_with("fail-safe") || text.starts_with("exiting:") {
        Level::Warn
    } else {
        Level::Info
    };
    (level, "note")
}

#[cfg(test)]
mod tests {
    use super::files::prune;

    use super::*;

    #[test]
    fn disabled_logger_is_noop() {
        Logger::default().log(Dir::Pons, b"x");
    }

    fn temp_log(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("pons-{name}-{}", std::process::id()))
    }

    fn parsed(path: &Path) -> Vec<Value> {
        let text = std::fs::read_to_string(path).unwrap_or_default();
        text.lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }

    #[test]
    fn appends_direction_and_text_as_json_lines() {
        let path = temp_log("log-test");
        let _ = std::fs::remove_file(&path);
        let logger = Logger::open(Some(&path), LogLevel::All, 0).unwrap();
        logger.log(Dir::ClientToAgent, b"{\"a\":1}");
        logger.log(Dir::PonsToClient, b"{\"b\":2}");
        logger.flush();
        let lines = parsed(&path);
        let _ = std::fs::remove_file(&path);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0]["dir"], "client->agent");
        assert_eq!(lines[0]["text"], "{\"a\":1}");
        assert_eq!(lines[1]["dir"], "pons->client");
        assert_eq!(lines[1]["text"], "{\"b\":2}");
    }

    #[test]
    fn a_wire_line_has_the_envelope_then_the_pons_fields() {
        let path = temp_log("shape-test");
        let _ = std::fs::remove_file(&path);
        let logger = Logger::open(Some(&path), LogLevel::All, 0).unwrap();
        logger.log(Dir::ClientToAgent, b"{}");
        logger.flush();
        let text = std::fs::read_to_string(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        let keys = [
            "\"ts\":",
            "\"level\":",
            "\"event\":",
            "\"ms\":",
            "\"dir\":",
            "\"text\":",
        ];
        let at: Vec<usize> = keys
            .into_iter()
            .map(|key| {
                text.find(key)
                    .unwrap_or_else(|| panic!("no {key} in {text}"))
            })
            .collect();
        assert!(at.windows(2).all(|w| w[0] < w[1]), "{text}");
        let line: Value = serde_json::from_str(text.trim_end()).unwrap();
        assert!(line["ts"].is_u64());
        assert!(line["ms"].as_u64().is_some_and(|ms| ms < 1000));
        assert_eq!(line["level"], "info");
        assert_eq!(line["event"], "wire");
    }

    #[test]
    fn notes_are_warn_or_error_by_their_text_and_a_warning_is_written_at_once() {
        let path = temp_log("level-test");
        let _ = std::fs::remove_file(&path);
        let logger = Logger::open(Some(&path), LogLevel::All, 0).unwrap();
        logger.log(Dir::ClientToAgent, b"{}");
        logger.log(Dir::Pons, b"agent: claude");
        logger.log(Dir::Pons, b"fail-safe: rules failed");
        let early = parsed(&path);
        logger.log(Dir::Pons, b"exiting: SIGTERM to its process group");
        logger.log(Dir::Pons, b"exiting: error: boom");
        logger.flush();
        let all = parsed(&path);
        let _ = std::fs::remove_file(&path);
        assert_eq!(
            early.len(),
            3,
            "the warning writes the buffered wire line too"
        );
        let levels: Vec<String> = all
            .iter()
            .map(|l| {
                format!(
                    "{}/{}",
                    l["event"].as_str().unwrap(),
                    l["level"].as_str().unwrap()
                )
            })
            .collect();
        assert_eq!(
            levels,
            [
                "wire/info",
                "note/info",
                "note/warn",
                "note/warn",
                "note/error"
            ]
        );
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

    #[test]
    fn prune_keeps_the_newest_ten_logs_of_both_formats_and_nothing_else() {
        let dir = std::env::temp_dir().join(format!("pons-prune-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let name = |minute: u32| {
            let ext = if minute.is_multiple_of(2) {
                "log"
            } else {
                "jsonl"
            };
            format!("pons-2026-10-14T22-{minute:02}-00-1.{ext}")
        };
        for minute in 0..12 {
            std::fs::write(dir.join(name(minute)), "x").unwrap();
        }
        let current = dir.join("pons-2026-10-14T22-12-00-2.jsonl");
        std::fs::write(&current, "x").unwrap();
        let others = [
            "notes.txt",
            "pons-keep.log",
            "pons-x-1.log",
            "pons-x-1.jsonl",
            "pons-2026-10-14T22-00-00-1.jsonl.old",
            "other-2026-10-14T22-00-00-1.jsonl",
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
    fn rotates_to_dot_old_when_over_cap() {
        let path = temp_log("rot-test");
        let rotated = capsa_core::log::rotated(&path);
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(&rotated);
        let logger = Logger::open(Some(&path), LogLevel::All, 300).unwrap();
        for i in 0..6 {
            logger.log(
                Dir::Pons,
                format!("line-{i}-xxxxxxxxxxxxxxxxxxxx").as_bytes(),
            );
        }
        logger.flush();
        let cur = std::fs::read_to_string(&path).unwrap();
        let old = std::fs::read_to_string(&rotated).unwrap();
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(&rotated);
        assert!(cur.contains("line-5-") && !cur.contains("line-0-"), "{cur}");
        assert!(old.contains("line-0-") && !old.contains("line-5-"), "{old}");
    }

    #[test]
    fn an_uncaught_panic_is_logged_with_the_lines_before_it_and_a_caught_one_is_not() {
        let path = temp_log("panic-test");
        let _ = std::fs::remove_file(&path);
        let logger = Logger::open(Some(&path), LogLevel::Default, 0).unwrap();
        logger.log_panics();
        // Buffered: only the panic line can write it out.
        logger.log(Dir::ClientToAgent, b"{}");
        let _ =
            std::panic::catch_unwind(|| crate::failsafe::catch_panic(|| panic!("caught quietly")));
        // The test thread is not `main`, so this is a task panic, not the exit.
        let _ = std::panic::catch_unwind(|| panic!("boom"));
        let lines = parsed(&path);
        let _ = std::fs::remove_file(&path);
        let texts: Vec<&str> = lines.iter().filter_map(|l| l["text"].as_str()).collect();
        assert!(
            texts
                .iter()
                .any(|t| t.starts_with("panic in task: boom at ")),
            "{texts:?}"
        );
        assert!(
            texts.iter().all(|t| !t.contains("caught quietly")),
            "{texts:?}"
        );
        assert!(
            lines.iter().any(|l| l["dir"] == "client->agent"),
            "{lines:?}"
        );
        let panic_line = lines
            .iter()
            .find(|l| {
                l["text"]
                    .as_str()
                    .is_some_and(|t| t.starts_with("panic in task"))
            })
            .unwrap();
        assert_eq!(panic_line["level"], "error");
    }
}
