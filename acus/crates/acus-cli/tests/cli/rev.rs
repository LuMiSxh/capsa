// SPDX-License-Identifier: MPL-2.0
use crate::common::*;

#[test]
fn show_reads_a_file_as_of_a_revision() {
    let _dropguard = snapshot_settings().bind_to_scope();
    let d = tempfile::tempdir().unwrap();
    let p = d.path();
    let (first, second) = ("2024-01-10T12:00:00+0000", "2024-02-01T12:00:00+0000");
    git_at(p, first, &["init", "-q"]);
    std::fs::write(p.join("a.rs"), "fn parse() {\n    old()\n}\n").unwrap();
    git_at(p, first, &["add", "."]);
    git_at(p, first, &["commit", "-qm", "first"]);
    std::fs::write(p.join("a.rs"), "fn parse() {\n    new()\n}\n").unwrap();
    std::fs::create_dir(p.join("sub")).unwrap();
    std::fs::write(p.join("sub/b.rs"), "fn b() {}\n").unwrap();
    git_at(p, second, &["add", "."]);
    git_at(p, second, &["commit", "-qm", "second"]);

    let (code, out, err) = acus_in(p, &["show", "HEAD~1:a.rs#parse"], "");
    assert_eq!((code, err.as_str()), (0, ""));
    assert!(out.starts_with("== HEAD~1:a.rs#parse 1-3\n"), "{out}");
    assert!(out.contains("old()") && !out.contains("new()"), "{out}");
    insta::assert_snapshot!("cli__show_rev_symbol", out);
    let (_, out, _) = acus_in(p, &["show", "a.rs#parse"], "");
    assert!(out.contains("new()"), "{out}");

    // `REV:./path` is relative to the current directory, `REV:path` to the repo root, as in git.
    let sub = p.join("sub");
    let (code, out, _) = acus_in(&sub, &["show", "HEAD:./b.rs"], "");
    assert_eq!(code, 0);
    assert!(out.starts_with("== HEAD:./b.rs 1-1\n"), "{out}");
    let (code, _, err) = acus_in(&sub, &["show", "HEAD:b.rs"], "");
    assert_eq!(code, 2);
    assert!(err.contains("hint: b.rs is not in HEAD"), "{err}");

    let (code, out, err) = acus_in(p, &["show", "nosuch:a.rs"], "");
    assert_eq!((code, out.as_str()), (2, ""));
    assert_eq!(
        err,
        "error: path not found: nosuch:a.rs\nhint: `nosuch` is not a revision (a commit, branch or tag)\n"
    );
    let (code, _, err) = acus_in(p, &["show", "HEAD:missing.rs"], "");
    assert_eq!(code, 2);
    assert!(
        err.starts_with("error: path not found: HEAD:missing.rs\nhint: missing.rs"),
        "{err}"
    );
}
