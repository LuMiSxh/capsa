// SPDX-License-Identifier: MPL-2.0

use super::*;
use serde_json::{Value, json};

#[derive(Debug, PartialEq, Deserialize)]
struct Guard {
    session: Option<String>,
    rule: String,
    call: String,
}

fn counts(path: &Path) -> Vec<u64> {
    read::<Value>(path)
        .unwrap()
        .iter()
        .map(|e| e.fields["n"].as_u64().unwrap())
        .collect()
}

#[test]
fn four_threads_append_whole_lines() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("log.jsonl");
    std::thread::scope(|s| {
        for t in 0..4 {
            let path = &path;
            s.spawn(move || {
                for i in 0..50 {
                    append(
                        path,
                        u64::MAX,
                        Level::Info,
                        "tick",
                        &json!({"t": t, "i": i}),
                    )
                    .unwrap();
                }
            });
        }
    });
    let text = std::fs::read_to_string(&path).unwrap();
    assert_eq!(text.lines().count(), 200);
    for line in text.lines() {
        let value: Value = serde_json::from_str(line).unwrap();
        assert_eq!(value["event"], "tick");
    }
}

#[test]
fn rotates_to_old_at_the_threshold_and_read_returns_old_lines_first() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("guard.jsonl");
    for n in 0..4 {
        append(&path, 100, Level::Info, "decision", &json!({"n": n})).unwrap();
    }
    assert_eq!(rotated(&path).file_name().unwrap(), "guard.jsonl.old");
    assert_eq!(
        std::fs::read_to_string(rotated(&path))
            .unwrap()
            .lines()
            .count(),
        2
    );
    assert_eq!(std::fs::read_to_string(&path).unwrap().lines().count(), 2);
    assert_eq!(counts(&path), [0, 1, 2, 3]);
}

#[test]
fn envelope_fields_come_before_the_tool_fields() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("log.jsonl");
    append(
        &path,
        u64::MAX,
        Level::Warn,
        "hook.error",
        &json!({"message": "m"}),
    )
    .unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    let value: Value = serde_json::from_str(text.trim_end()).unwrap();
    let keys: Vec<&str> = value
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(keys, ["ts", "level", "event", "message"]);
    let entries = read::<Value>(&path).unwrap();
    assert_eq!(entries[0].level, Some(Level::Warn));
    assert_eq!(entries[0].event.as_deref(), Some("hook.error"));
}

#[test]
fn entry_parses_lines_written_before_level_and_event() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("guard.jsonl");
    let old = r#"{"ts":1,"session":"s","cwd":"c","tool":"Bash","rule":"grep","kind":"rewrite","call":"x"}"#;
    std::fs::write(&path, format!("{old}\n")).unwrap();
    let entries: Vec<Entry<Guard>> = read(&path).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].ts, 1);
    assert_eq!(entries[0].level, None);
    assert_eq!(entries[0].event, None);
    assert_eq!(
        entries[0].fields,
        Guard {
            session: Some("s".to_owned()),
            rule: "grep".to_owned(),
            call: "x".to_owned(),
        }
    );
}

#[test]
fn writer_keeps_info_lines_buffered_until_dropped() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("log.jsonl");
    let mut writer = Writer::open(&path, u64::MAX).unwrap();
    writer.write(Level::Info, "tick", &json!({"n": 1})).unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "");
    drop(writer);
    assert_eq!(counts(&path), [1]);
}

#[test]
fn writer_flushes_buffered_lines_on_warn() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("log.jsonl");
    let mut writer = Writer::open(&path, u64::MAX).unwrap();
    writer.write(Level::Info, "tick", &json!({"n": 1})).unwrap();
    writer.write(Level::Warn, "tick", &json!({"n": 2})).unwrap();
    assert_eq!(counts(&path), [1, 2]);
    drop(writer);
}

#[test]
fn writer_rotates_before_a_write_past_the_threshold() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("log.jsonl");
    let mut writer = Writer::open(&path, 100).unwrap();
    for n in 0..4 {
        writer.write(Level::Info, "tick", &json!({"n": n})).unwrap();
    }
    writer.flush().unwrap();
    assert_eq!(
        std::fs::read_to_string(rotated(&path))
            .unwrap()
            .lines()
            .count(),
        2
    );
    assert_eq!(counts(&path), [0, 1, 2, 3]);
}

#[test]
fn read_skips_malformed_lines() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("log.jsonl");
    let text =
        "{\"ts\":1,\"n\":1}\nnot json\n\n{\"n\":9}\n{\"ts\":2,\"n\":\"x\"\n{\"ts\":3,\"n\":3}\n";
    std::fs::write(&path, text).unwrap();
    assert_eq!(counts(&path), [1, 3]);
}

#[test]
fn read_of_a_missing_file_has_no_entries() {
    let dir = tempfile::tempdir().unwrap();
    assert!(
        read::<Value>(&dir.path().join("none.jsonl"))
            .unwrap()
            .is_empty()
    );
}

#[test]
fn read_fails_on_a_file_it_cannot_read() {
    let dir = tempfile::tempdir().unwrap();
    assert!(matches!(
        read::<Value>(dir.path()),
        Err(LogError::Read { .. })
    ));
}
