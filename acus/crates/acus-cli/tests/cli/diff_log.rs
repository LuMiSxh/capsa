// SPDX-License-Identifier: MPL-2.0
use crate::common::*;

#[test]
fn diff_groups_changes_by_symbol() {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(
        d.path().join("a.rs"),
        "fn f() {\n    one()\n}\n\nfn gone() {}\n",
    )
    .unwrap();
    git(d.path(), &["init", "-q"]);
    git(d.path(), &["add", "."]);
    git(d.path(), &["commit", "-qm", "x"]);
    std::fs::write(d.path().join("a.rs"), "fn f() {\n    two()\n}\n").unwrap();
    std::fs::write(d.path().join("new.md"), "# Hi\n").unwrap();
    let (code, out, _) = acus_in(d.path(), &["diff", "-p"], "");
    assert_eq!(code, 0, "{out}");
    assert_eq!(
        out,
        "M a.rs +1 -3\n  fn f 1-3 +1 -1\n\t-    one()\n2\t+    two()\n  (top level) -1\n\t-\n  fn gone (removed) -1\n\t-fn gone() {}\n? new.md +1\n1\t+# Hi\n== 2 files +2 -3\n"
    );
    git(d.path(), &["add", "."]);
    git(d.path(), &["commit", "-qm", "y"]);
    assert_eq!(
        acus_in(d.path(), &["diff"], ""),
        (0, String::new(), "(no changes)\n".into())
    );
    let (code, out, err) = acus_in(d.path(), &["diff", "--json"], "");
    assert_eq!(code, 0);
    assert!(err.is_empty());
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&out).unwrap(),
        serde_json::Value::Null
    );
    let (code, out, err) = acus_in(d.path(), &["run", "--json"], r#"[["diff"]]"#);
    assert_eq!((code, out.as_str(), err.as_str()), (0, "null\n", ""));
}

#[test]
fn unstaged_diff_reads_the_index() {
    let d = tempfile::tempdir().unwrap();
    git(d.path(), &["init", "-q"]);
    std::fs::write(
        d.path().join("a.rs"),
        "fn one() {\n    1;\n}\n\nfn two() {\n    2;\n}\n",
    )
    .unwrap();
    git(d.path(), &["add", "."]);
    git(d.path(), &["commit", "-qm", "i"]);
    git(d.path(), &["mv", "a.rs", "b.rs"]);
    std::fs::write(
        d.path().join("b.rs"),
        "fn one() {\n    1;\n}\n\nfn two() {\n    3;\n}\n",
    )
    .unwrap();
    let (code, out, _) = acus_in(d.path(), &["diff"], "");
    assert_eq!(
        (code, out.as_str()),
        (
            0,
            "M b.rs +1 -1\n  fn two 5-7 +1 -1\n== 1 file +1 -1\nstaged: 1 file (acus diff --staged)\n",
        )
    );
}

#[test]
fn default_diff_names_staged_changes_in_a_footer() {
    let d = tempfile::tempdir().unwrap();
    git(d.path(), &["init", "-q"]);
    std::fs::write(d.path().join("a.rs"), "fn one() {\n    1;\n}\n").unwrap();
    git(d.path(), &["add", "."]);
    git(d.path(), &["commit", "-qm", "i"]);
    std::fs::write(d.path().join("a.rs"), "fn one() {\n    2;\n}\n").unwrap();
    git(d.path(), &["add", "a.rs"]);
    // Only staged changes: the footer replaces "(no changes)" and the exit code stays 0.
    let (code, out, err) = acus_in(d.path(), &["diff"], "");
    assert_eq!(
        (code, out.as_str(), err.as_str()),
        (0, "staged: 1 file +1 -1 (acus diff --staged)\n", "")
    );
    // The staged view shows the change itself, without the footer.
    let (code, out, _) = acus_in(d.path(), &["diff", "--staged"], "");
    assert_eq!(code, 0);
    assert!(out.ends_with("== 1 file +1 -1\n"), "{out}");
    // Unstaged work comes first, the footer last.
    std::fs::write(d.path().join("new.md"), "# Hi\n").unwrap();
    let (code, out, _) = acus_in(d.path(), &["diff"], "");
    assert_eq!(code, 0);
    assert_eq!(
        out,
        "? new.md +1\n== 1 file +1\nstaged: 1 file +1 -1 (acus diff --staged)\n"
    );
}

#[test]
fn untracked_files_are_marked_question_and_staged_new_files_a() {
    let d = tempfile::tempdir().unwrap();
    git(d.path(), &["init", "-q"]);
    std::fs::write(d.path().join("a.txt"), "old\n").unwrap();
    git(d.path(), &["add", "."]);
    git(d.path(), &["commit", "-qm", "i"]);
    std::fs::write(d.path().join("staged.txt"), "two\n").unwrap();
    git(d.path(), &["add", "staged.txt"]);
    std::fs::write(d.path().join("untracked.txt"), "one\n").unwrap();
    // Not in the index: `?`, like git's `??`; the staged footer still names the staged file.
    let (code, out, _) = acus_in(d.path(), &["diff"], "");
    assert_eq!(
        (code, out.as_str()),
        (
            0,
            "? untracked.txt +1\n== 1 file +1\nstaged: 1 file +1 (acus diff --staged)\n",
        )
    );
    // Staged new file: `A`.
    let (code, out, _) = acus_in(d.path(), &["diff", "--staged"], "");
    assert_eq!((code, out.as_str()), (0, "A staged.txt +1\n== 1 file +1\n"));
    // The JSON status matches the text marker.
    let (code, out, err) = acus_in(d.path(), &["diff", "--json"], "");
    assert_eq!((code, err.as_str()), (0, ""));
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v[0]["status"], "?");
}

#[test]
fn diff_human_view_aligns_file_counts() {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(
        d.path().join("a.rs"),
        "fn f() {\n    one()\n}\n\nfn gone() {}\n",
    )
    .unwrap();
    git(d.path(), &["init", "-q"]);
    git(d.path(), &["add", "."]);
    git(d.path(), &["commit", "-qm", "x"]);
    std::fs::write(d.path().join("a.rs"), "fn f() {\n    two()\n}\n").unwrap();
    std::fs::write(d.path().join("new.md"), "# Hi\n").unwrap();
    assert_eq!(
        acus_in(d.path(), &["diff", "--human"], ""),
        (
            0,
            "M a.rs    +1 -3\n  fn f 1-3 +1 -1\n  (top level) -1\n  fn gone (removed) -1\n? new.md     +1\n== 2 files +2 -3\n".into(),
            String::new()
        )
    );
}

#[test]
fn log_human_view_aligns_commits_in_a_table() {
    let d = tempfile::tempdir().unwrap();
    git(d.path(), &["init", "-q"]);
    std::fs::write(d.path().join("a.rs"), "fn f() {\n    1\n}\n").unwrap();
    git_at(d.path(), "2026-01-01T10:00:00+0000", &["add", "."]);
    git_at(
        d.path(),
        "2026-01-01T10:00:00+0000",
        &["commit", "-qm", "first"],
    );
    std::fs::write(d.path().join("a.rs"), "fn f() {\n    2\n}\n").unwrap();
    git_at(d.path(), "2026-01-02T10:00:00+0000", &["add", "."]);
    git_at(
        d.path(),
        "2026-01-02T10:00:00+0000",
        &["commit", "-qm", "second: two"],
    );
    let h2 = git(d.path(), &["rev-parse", "--short=7", "HEAD"])
        .trim()
        .to_owned();
    let h1 = git(d.path(), &["rev-parse", "--short=7", "HEAD~1"])
        .trim()
        .to_owned();
    let (code, out, err) = acus_in(d.path(), &["log", "--human", "-n", "2"], "");
    assert_eq!((code, err.as_str()), (0, ""));
    assert_eq!(
        out,
        format!(
            "2 commits\n\n  commit   date        author   changes  subject\n  {h2}  2026-01-02  t       +1 -1 1f  second: two\n  {h1}  2026-01-01  t       +3 -0 1f  first\n  more: -n N; one commit's changes: acus diff REV^..REV\n"
        )
    );
}

#[test]
fn human_views_print_no_ansi_under_no_color() {
    let d = tempfile::tempdir().unwrap();
    git(d.path(), &["init", "-q"]);
    std::fs::write(d.path().join("a.rs"), "fn f() {}\n").unwrap();
    git(d.path(), &["add", "."]);
    git(d.path(), &["commit", "-qm", "x"]);
    std::fs::write(d.path().join("a.rs"), "fn f() {\n}\n").unwrap();
    let check = |args: &[&str]| {
        let (code, out, err) = acus_in_env(d.path(), args, "", &[("NO_COLOR", "1")]);
        assert_eq!((code, err.as_str()), (0, ""), "{args:?}");
        assert!(!out.contains('\x1b'), "{args:?}: {out:?}");
    };
    check(&["diff", "--human", "-p"]);
    check(&["log", "--human"]);
    check(&["files", "--human", "--lines"]);
}

#[test]
fn oneline_and_stat_are_accepted_as_no_ops() {
    let d = tempfile::tempdir().unwrap();
    git(d.path(), &["init", "-q"]);
    for n in 1..=3 {
        std::fs::write(d.path().join("a.rs"), format!("fn f() {{\n    {n}\n}}\n")).unwrap();
        git(d.path(), &["add", "."]);
        git(d.path(), &["commit", "-qm", &format!("c{n}")]);
    }
    let log = acus_in(d.path(), &["log", "-n", "2"], "");
    assert_eq!(log.0, 0, "{}", log.2);
    assert!(log.1.contains("== 2 commits"), "{}", log.1);
    assert_eq!(acus_in(d.path(), &["log", "--oneline", "-n", "2"], ""), log);
    std::fs::write(d.path().join("a.rs"), "fn f() {\n    9\n}\n").unwrap();
    let diff = acus_in(d.path(), &["diff"], "");
    assert_eq!(diff.0, 0, "{}", diff.2);
    assert!(diff.1.contains("M a.rs +1 -1"), "{}", diff.1);
    assert_eq!(acus_in(d.path(), &["diff", "--stat"], ""), diff);
}
