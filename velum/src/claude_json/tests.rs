// SPDX-License-Identifier: MPL-2.0
use super::*;
use crate::scratch::Scratch;
use serde_json::json;

fn setup(name: &str, before: &Value) -> (Scratch, String) {
    let scratch = Scratch::new(name);
    let home = scratch.path().to_str().unwrap().to_string();
    std::fs::write(scratch.path().join(".claude.json"), before.to_string()).unwrap();
    (scratch, home)
}

fn read(home: &str) -> Value {
    let raw = std::fs::read(Path::new(home).join(".claude.json")).unwrap();
    serde_json::from_slice(&raw).unwrap()
}

fn write(home: &str, doc: &Value) {
    std::fs::write(Path::new(home).join(".claude.json"), doc.to_string()).unwrap();
}

#[test]
fn restores_only_changed_guarded_keys_and_keeps_everything_else() {
    let before = json!({
        "numStartups": 1,
        "mcpServers": {"keep": {"command": "a"}, "edit": {"command": "b"}},
        "projects": {
            "/p": {
                "mcpServers": {"x": {"command": "d"}},
                "allowedTools": ["Read"],
                "hasTrustDialogAccepted": false,
                "history": []
            },
            "/gone": {"enabledMcpjsonServers": ["a"], "other": 1}
        }
    });
    let (_s, home) = setup("cj-restore", &before);
    let guard = Guard::snapshot(&home).unwrap();
    write(
        &home,
        &json!({
            "numStartups": 2,
            "mcpServers": {"keep": {"command": "a"}, "edit": {"command": "evil"}},
            "projects": {
                "/p": {
                    "mcpServers": {"x": {"command": "d"}, "y": {"command": "f"}},
                    "allowedTools": ["Read", "Bash(*)"],
                    "hasTrustDialogAccepted": true,
                    "mcpContextUris": ["u"],
                    "history": [1]
                },
                "/q": {"mcpServers": {"z": {"command": "g"}}, "hasTrustDialogAccepted": true, "lastCost": 3},
                "/r": {"disabledMcpServers": ["a"]}
            }
        }),
    );
    let report = guard.restore().unwrap();
    assert_eq!(
        report.changed,
        [
            "allowedTools (project /p)",
            "enabledMcpjsonServers (project /gone)",
            "hasTrustDialogAccepted (project /p)",
            "hasTrustDialogAccepted (project /q)",
            "mcpContextUris (project /p)",
            "mcpServers",
            "mcpServers (project /p)",
            "mcpServers (project /q)",
            "disabledMcpServers (project /r)",
        ]
        .iter()
        .map(ToString::to_string)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>()
    );
    let after = read(&home);
    assert_eq!(after["mcpServers"], before["mcpServers"]);
    assert_eq!(
        after["projects"]["/p"]["mcpServers"],
        json!({"x": {"command": "d"}})
    );
    assert_eq!(after["projects"]["/p"]["allowedTools"], json!(["Read"]));
    assert_eq!(after["projects"]["/p"]["hasTrustDialogAccepted"], false);
    assert!(after["projects"]["/p"].get("mcpContextUris").is_none());
    assert_eq!(
        after["projects"]["/gone"]["enabledMcpjsonServers"],
        json!(["a"])
    );
    assert_eq!(after["projects"]["/q"], json!({"lastCost": 3}));
    assert!(after["projects"].get("/r").is_none());
    assert_eq!(after["numStartups"], 2);
    assert_eq!(after["projects"]["/p"]["history"], json!([1]));
    let warning = report.warnings().join("\n");
    assert!(warning.contains("hasTrustDialogAccepted (project /q)"));
}

#[test]
fn unchanged_guarded_keys_leave_the_file_untouched() {
    let before = json!({"mcpServers": {"a": {"command": "x"}}, "numStartups": 1});
    let (_s, home) = setup("cj-same", &before);
    let guard = Guard::snapshot(&home).unwrap();
    let edited = json!({"mcpServers": {"a": {"command": "x"}}, "numStartups": 9});
    write(&home, &edited);
    let report = guard.restore().unwrap();
    assert_eq!(report, Report::default());
    assert!(report.warnings().is_empty());
    assert_eq!(read(&home), edited);
}

#[test]
fn an_unparsable_file_is_replaced_by_the_pre_run_copy() {
    let before = json!({"mcpServers": {"a": {"command": "x"}}});
    let (_s, home) = setup("cj-garbage", &before);
    let guard = Guard::snapshot(&home).unwrap();
    std::fs::write(Path::new(&home).join(".claude.json"), "{not json").unwrap();
    let report = guard.restore().unwrap();
    assert_eq!(read(&home), before);
    assert!(report.warnings()[0].contains("pre-run copy"));
}

#[test]
fn a_file_created_during_the_run_has_its_guarded_keys_removed() {
    let scratch = Scratch::new("cj-created");
    let home = scratch.path().to_str().unwrap().to_string();
    let guard = Guard::snapshot(&home).unwrap();
    write(
        &home,
        &json!({"mcpServers": {"evil": {"command": "x"}}, "theme": "dark"}),
    );
    let report = guard.restore().unwrap();
    assert_eq!(report.changed, ["mcpServers"]);
    assert_eq!(read(&home), json!({"theme": "dark"}));
}

fn raw_file(home: &str) -> Vec<u8> {
    std::fs::read(Path::new(home).join(".claude.json")).unwrap()
}

#[test]
fn a_file_that_did_not_parse_before_is_restored_byte_for_byte() {
    let scratch = Scratch::new("cj-bad-before");
    let home = scratch.path().to_str().unwrap().to_string();
    let original = b"garbage {\xff\n".to_vec();
    std::fs::write(scratch.path().join(".claude.json"), &original).unwrap();
    let guard = Guard::snapshot(&home).unwrap();
    write(
        &home,
        &json!({"mcpServers": {"evil": {}}, "projects": {"/p": {"hasTrustDialogAccepted": true}}}),
    );
    let report = guard.restore().unwrap();
    assert_eq!(raw_file(&home), original);
    assert!(report.warnings()[0].contains("~/.claude.json"));
}

#[test]
fn a_non_object_root_before_the_run_is_restored_byte_for_byte() {
    let scratch = Scratch::new("cj-array-before");
    let home = scratch.path().to_str().unwrap().to_string();
    std::fs::write(scratch.path().join(".claude.json"), "[]").unwrap();
    let guard = Guard::snapshot(&home).unwrap();
    write(&home, &json!({"mcpServers": {"evil": {}}}));
    let report = guard.restore().unwrap();
    assert_eq!(raw_file(&home), b"[]");
    assert!(report.warnings()[0].contains("pre-run copy"));
}

#[test]
fn an_unchanged_unparsable_file_is_left_alone_without_warning() {
    let scratch = Scratch::new("cj-bad-same");
    let home = scratch.path().to_str().unwrap().to_string();
    std::fs::write(scratch.path().join(".claude.json"), "garbage").unwrap();
    let guard = Guard::snapshot(&home).unwrap();
    let report = guard.restore().unwrap();
    assert!(report.warnings().is_empty());
    assert_eq!(raw_file(&home), b"garbage");
}

#[test]
fn an_unparsable_file_deleted_during_the_run_is_put_back() {
    let scratch = Scratch::new("cj-bad-deleted");
    let home = scratch.path().to_str().unwrap().to_string();
    let path = scratch.path().join(".claude.json");
    std::fs::write(&path, "garbage").unwrap();
    let guard = Guard::snapshot(&home).unwrap();
    std::fs::remove_file(&path).unwrap();
    guard.restore().unwrap();
    assert_eq!(raw_file(&home), b"garbage");
}

#[test]
fn a_symlink_replacing_the_file_is_replaced_by_the_pre_run_copy() {
    let before = json!({"mcpServers": {"a": {"command": "x"}}});
    let (s, home) = setup("cj-symlink", &before);
    let guard = Guard::snapshot(&home).unwrap();
    let target = s.path().join("elsewhere.json");
    std::fs::write(&target, "{}").unwrap();
    let path = s.path().join(".claude.json");
    std::fs::remove_file(&path).unwrap();
    std::os::unix::fs::symlink(&target, &path).unwrap();
    let report = guard.restore().unwrap();
    assert!(report.warnings()[0].contains("not a regular file"));
    assert!(!path.symlink_metadata().unwrap().file_type().is_symlink());
    assert_eq!(read(&home), before);
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "{}");
}

#[test]
fn a_fifo_does_not_block_and_is_replaced() {
    let before = json!({"mcpServers": {}});
    let (s, home) = setup("cj-fifo", &before);
    let guard = Guard::snapshot(&home).unwrap();
    let path = s.path().join(".claude.json");
    std::fs::remove_file(&path).unwrap();
    let c_path = std::ffi::CString::new(path.to_str().unwrap()).unwrap();
    // SAFETY: `c_path` is a valid NUL-terminated string.
    assert_eq!(unsafe { libc::mkfifo(c_path.as_ptr(), 0o600) }, 0);
    let report = guard.restore().unwrap();
    assert!(report.warnings()[0].contains("pre-run copy"));
    assert_eq!(read(&home), before);
}

#[test]
fn an_oversized_file_is_replaced_and_a_file_that_was_absent_is_removed() {
    let before = json!({"mcpServers": {}});
    let (s, home) = setup("cj-big", &before);
    let guard = Guard::snapshot(&home).unwrap();
    let path = s.path().join(".claude.json");
    let file = std::fs::File::create(&path).unwrap();
    file.set_len(MAX_BYTES + 1).unwrap();
    drop(file);
    let report = guard.restore().unwrap();
    assert!(report.warnings()[0].contains("larger than"));
    assert_eq!(read(&home), before);

    let fresh = Scratch::new("cj-big-absent");
    let home = fresh.path().to_str().unwrap().to_string();
    let guard = Guard::snapshot(&home).unwrap();
    let path = fresh.path().join(".claude.json");
    std::os::unix::fs::symlink("/etc/hosts", &path).unwrap();
    let report = guard.restore().unwrap();
    assert!(report.warnings()[0].contains("removed"));
    assert!(path.symlink_metadata().is_err());
}

#[test]
fn snapshot_refuses_a_file_that_is_not_regular() {
    let scratch = Scratch::new("cj-snap-link");
    let home = scratch.path().to_str().unwrap().to_string();
    std::os::unix::fs::symlink("/etc/hosts", scratch.path().join(".claude.json")).unwrap();
    let err = Guard::snapshot(&home).unwrap_err();
    assert!(capsa_core::hint::find_hint(&err).is_some());
}
