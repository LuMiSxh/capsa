// SPDX-License-Identifier: MPL-2.0
//! Runs `census hook` and `census self skill` as binaries under a scratch state and config.

mod common;

use common::Scratch;
use serde_json::{Value, json};
use std::io::Write;
use std::process::{Command, Output, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

struct Hook {
    home: Scratch,
}

impl Hook {
    fn new(name: &str) -> Self {
        let home = Scratch::new(name);
        home.write("claude/settings.json", r#"{"autoCompactWindow": 400000}"#);
        Hook { home }
    }

    /// Writes a transcript whose last request holds `tokens` of context; returns its path.
    fn transcript(&self, tokens: u64) -> String {
        let line = json!({"type": "assistant", "timestamp": "2026-10-14T08:00:10.000Z",
            "message": {"id": "m1", "model": "claude-opus-5-5",
                "usage": {"input_tokens": tokens, "output_tokens": 10}}});
        let path = self
            .home
            .write("claude/projects/app/s1.jsonl", &format!("{line}\n"));
        path.display().to_string()
    }

    fn input(&self, event: &str, tokens: u64) -> String {
        json!({"hook_event_name": event, "session_id": "s1",
            "transcript_path": self.transcript(tokens), "cwd": "/tmp"})
        .to_string()
    }

    #[expect(
        clippy::expect_used,
        reason = "a binary that cannot run must fail the test"
    )]
    fn run(&self, args: &[&str], stdin: &[u8]) -> Output {
        let mut child = Command::new(env!("CARGO_BIN_EXE_census"))
            .args(args)
            .env("XDG_STATE_HOME", self.home.path().join("state"))
            .env("CLAUDE_CONFIG_DIR", self.home.path().join("claude"))
            .env("HOME", self.home.path())
            .env_remove("CLAUDE_CODE_AUTO_COMPACT_WINDOW")
            .env_remove("CLAUDE_AUTOCOMPACT_PCT_OVERRIDE")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("start census");
        if let Some(mut input) = child.stdin.take() {
            let _ = input.write_all(stdin);
        }
        child.wait_with_output().expect("wait for census")
    }

    /// Runs the hook and returns its `additionalContext`, after checking the exit code.
    #[expect(
        clippy::expect_used,
        reason = "output that is not JSON must fail the test"
    )]
    fn hook(&self, stdin: &[u8]) -> Option<String> {
        let out = self.run(&["hook"], stdin);
        assert_eq!(out.status.code(), Some(0));
        if out.stdout.is_empty() {
            return None;
        }
        let v: Value = serde_json::from_slice(&out.stdout).expect("hook output is JSON");
        Some(
            v["hookSpecificOutput"]["additionalContext"]
                .as_str()?
                .to_owned(),
        )
    }

    fn snapshot(&self, window: &str, used: f64, resets_at: u64) {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        let line = json!({"t": now, "source": "statusline", "window": window,
            "used_percentage": used, "resets_at": resets_at});
        self.home
            .write("state/census/snapshots", &format!("{line}\n"));
    }
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

#[test]
fn subagent_input_prints_nothing() {
    let h = Hook::new("hook-sub");
    let mut v: Value = serde_json::from_str(&h.input("PostToolUse", 330_000)).expect("json");
    v["agent_id"] = json!("a1");
    v["agent_type"] = json!("worker");
    assert_eq!(h.hook(v.to_string().as_bytes()), None);
}

#[test]
fn other_events_print_nothing() {
    let h = Hook::new("hook-event");
    assert_eq!(h.hook(h.input("SessionStart", 330_000).as_bytes()), None);
}

#[test]
fn context_message_shows_once_then_the_handoff_level_adds_its_text() {
    let h = Hook::new("hook-context");
    let first = h.hook(h.input("UserPromptSubmit", 245_000).as_bytes());
    assert_eq!(
        first.as_deref(),
        Some("census: context 245k of 400k (compaction at 400k).")
    );
    assert_eq!(h.hook(h.input("PostToolUse", 250_000).as_bytes()), None);
    let high = h.hook(h.input("PostToolUse", 325_000).as_bytes());
    assert_eq!(
        high.as_deref(),
        Some(
            "census: context 325k of 400k (compaction at 400k). At the next task boundary, suggest a handoff to Luca (session-handoff skill)."
        )
    );
    assert_eq!(h.hook(h.input("PostToolUse", 330_000).as_bytes()), None);
}

#[test]
fn context_levels_show_again_after_a_drop_below_half() {
    let h = Hook::new("hook-rearm");
    assert!(h.hook(h.input("PostToolUse", 330_000).as_bytes()).is_some());
    assert_eq!(h.hook(h.input("PostToolUse", 335_000).as_bytes()), None);
    assert_eq!(h.hook(h.input("PostToolUse", 100_000).as_bytes()), None);
    let again = h.hook(h.input("PostToolUse", 245_000).as_bytes());
    assert_eq!(
        again.as_deref(),
        Some("census: context 245k of 400k (compaction at 400k).")
    );
}

#[test]
fn output_has_exactly_the_hook_shape() {
    let h = Hook::new("hook-shape");
    let out = h.run(&["hook"], h.input("UserPromptSubmit", 245_000).as_bytes());
    let v: Value = serde_json::from_slice(&out.stdout).expect("json");
    assert_eq!(
        v,
        json!({"hookSpecificOutput": {"hookEventName": "UserPromptSubmit",
            "additionalContext": "census: context 245k of 400k (compaction at 400k)."}})
    );
}

#[test]
fn malformed_and_oversized_input_print_nothing_and_exit_zero() {
    let h = Hook::new("hook-bad");
    assert_eq!(h.hook(b"not json"), None);
    assert_eq!(h.hook(b""), None);
    let mut big = h.input("UserPromptSubmit", 330_000).into_bytes();
    big.extend(std::iter::repeat_n(b' ', 1 << 21));
    assert_eq!(h.hook(&big), None);
}

#[test]
fn a_path_like_session_id_is_refused() {
    let h = Hook::new("hook-id");
    let mut v: Value = serde_json::from_str(&h.input("UserPromptSubmit", 330_000)).expect("json");
    v["session_id"] = json!("../../x");
    assert_eq!(h.hook(v.to_string().as_bytes()), None);
}

#[test]
fn a_limit_level_shows_once_per_window_and_again_for_a_new_one() {
    let h = Hook::new("hook-limit");
    let input = h.input("UserPromptSubmit", 10_000);
    h.snapshot("five_hour", 82.0, now() + 3 * 3600);
    let first = h.hook(input.as_bytes()).expect("a limit message");
    assert!(first.starts_with("census: 5h limit 82%"), "{first}");
    assert!(
        first.ends_with("Avoid large parallel agent batches; tell Luca."),
        "{first}"
    );
    assert_eq!(h.hook(input.as_bytes()), None);
    h.snapshot("five_hour", 82.0, now() + 8 * 3600);
    assert!(h.hook(input.as_bytes()).is_some());
}

#[test]
fn self_skill_prints_the_skill_and_install_writes_it() {
    let h = Hook::new("hook-skill");
    let out = h.run(&["self", "skill"], b"");
    assert_eq!(out.status.code(), Some(0));
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(text.starts_with("---\nname: census\n"), "{text}");
    assert!(text.lines().count() < 60);
    let out = h.run(&["self", "skill", "--install"], b"");
    assert_eq!(out.status.code(), Some(0));
    let installed = h.home.path().join(".claude/skills/census/SKILL.md");
    assert_eq!(std::fs::read_to_string(installed).expect("installed"), text);
}
