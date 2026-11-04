// SPDX-License-Identifier: MPL-2.0
use crate::common::*;
use std::process::Command;

fn repo() -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    git(d.path(), &["init", "-q", "-b", "main"]);
    d
}

fn commit(d: &std::path::Path, msg: &str) {
    git(d, &["add", "."]);
    git(d, &["commit", "-qm", msg]);
}

fn first_line(out: &str) -> &str {
    out.lines().next().unwrap_or_default()
}

/// Runs the merge that leaves a conflict in `a.txt`, without asserting success.
fn merge_conflict(d: &std::path::Path) {
    std::fs::write(d.join("a.txt"), "one\ntwo\nthree\n").unwrap();
    commit(d, "base");
    git(d, &["checkout", "-q", "-b", "side"]);
    std::fs::write(d.join("a.txt"), "one\nside\nthree\n").unwrap();
    commit(d, "side");
    git(d, &["checkout", "-q", "main"]);
    std::fs::write(d.join("a.txt"), "one\nmain\nthree\n").unwrap();
    commit(d, "main");
    Command::new("git")
        .args(["-c", "user.name=t", "-c", "user.email=t@t", "merge", "side"])
        .current_dir(d)
        .output()
        .unwrap();
}

#[test]
fn header_names_branch_counts_and_a_clean_tree() {
    let d = repo();
    std::fs::write(d.path().join("a.rs"), "one\n").unwrap();
    commit(d.path(), "one");
    std::fs::write(d.path().join("a.rs"), "two\n").unwrap();
    std::fs::write(d.path().join("staged.txt"), "s\n").unwrap();
    git(d.path(), &["add", "staged.txt"]);
    std::fs::write(d.path().join("untracked.txt"), "u\n").unwrap();
    let (code, out, _) = acus_in(d.path(), &["diff"], "");
    assert_eq!(code, 0);
    assert_eq!(
        first_line(&out),
        "on main · staged 1 · unstaged 1 · untracked 1"
    );
    // A clean tree still prints the header, then the existing note on stderr.
    commit(d.path(), "all");
    assert_eq!(
        acus_in(d.path(), &["diff"], ""),
        (0, "on main\n".into(), "(no changes)\n".into())
    );
}

#[test]
fn detached_head_names_the_short_hash() {
    let d = repo();
    std::fs::write(d.path().join("a.rs"), "one\n").unwrap();
    commit(d.path(), "one");
    let short = git(d.path(), &["rev-parse", "--short=7", "HEAD"])
        .trim()
        .to_owned();
    git(d.path(), &["checkout", "-q", "--detach"]);
    std::fs::write(d.path().join("a.rs"), "two\n").unwrap();
    let (code, out, _) = acus_in(d.path(), &["diff"], "");
    assert_eq!(code, 0);
    assert_eq!(first_line(&out), format!("on {short} · unstaged 1"));
}

#[test]
fn header_names_the_upstream_and_its_ahead_behind() {
    let base = tempfile::tempdir().unwrap();
    let origin = base.path().join("origin.git");
    git(
        base.path(),
        &[
            "init",
            "-q",
            "--bare",
            "-b",
            "main",
            &origin.to_string_lossy(),
        ],
    );
    let d = repo();
    std::fs::write(d.path().join("a.txt"), "a\n").unwrap();
    commit(d.path(), "one");
    git(
        d.path(),
        &["remote", "add", "origin", &origin.to_string_lossy()],
    );
    git(d.path(), &["push", "-q", "-u", "origin", "main"]);
    // One local commit ahead and one pushed from another clone behind.
    std::fs::write(d.path().join("b.txt"), "b\n").unwrap();
    commit(d.path(), "ahead");
    let other = tempfile::tempdir().unwrap();
    git(
        other.path(),
        &["clone", "-q", &origin.to_string_lossy(), "."],
    );
    std::fs::write(other.path().join("c.txt"), "c\n").unwrap();
    commit(other.path(), "behind");
    git(other.path(), &["push", "-q", "origin", "main"]);
    git(d.path(), &["fetch", "-q"]);
    let (code, out, _) = acus_in(d.path(), &["diff"], "");
    assert_eq!(code, 0);
    assert_eq!(first_line(&out), "on main → origin/main ahead 1 behind 1");
}

#[test]
fn a_merge_conflict_shows_in_the_header_and_its_ranges() {
    let d = repo();
    merge_conflict(d.path());
    let (code, out, _) = acus_in(d.path(), &["diff"], "");
    assert_eq!(code, 0);
    assert_eq!(
        first_line(&out),
        "on main · conflicts 1 · merge in progress"
    );
    assert!(out.contains("U a.txt\n"), "{out}");
    let (code, out, _) = acus_in(d.path(), &["diff", "--conflicts"], "");
    assert_eq!(code, 0);
    assert!(out.contains("a.txt: 2-6\n"), "{out}");
    assert!(
        out.contains("*** Resolve Conflict: PATH --ours|--theirs"),
        "{out}"
    );
    let (code, out, _) = acus_in(d.path(), &["diff", "--conflicts", "--json"], "");
    assert_eq!(code, 0);
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["header"]["operation"], "merge");
    assert_eq!(v["conflicts"][0]["path"], "a.txt");
    assert_eq!(v["conflicts"][0]["ranges"], serde_json::json!([[2, 6]]));
}

#[test]
fn conflicts_without_a_conflict_exit_1() {
    let d = repo();
    std::fs::write(d.path().join("a.txt"), "a\n").unwrap();
    commit(d.path(), "one");
    let (code, out, err) = acus_in(d.path(), &["diff", "--conflicts"], "");
    assert_eq!((code, out.as_str(), err.as_str()), (1, "on main\n", ""));
    let (code, out, _) = acus_in(d.path(), &["diff", "--conflicts", "--json"], "");
    assert_eq!(code, 1);
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["conflicts"], serde_json::json!([]));
}

#[test]
fn stat_and_name_only_list_paths_with_their_status() {
    let d = repo();
    std::fs::write(d.path().join("a.rs"), "one\n").unwrap();
    commit(d.path(), "one");
    std::fs::write(d.path().join("a.rs"), "one\ntwo\n").unwrap();
    std::fs::write(d.path().join("new.md"), "# Hi\n").unwrap();
    let (code, out, _) = acus_in(d.path(), &["diff", "--stat"], "");
    assert_eq!(code, 0);
    assert_eq!(
        out,
        "on main · unstaged 1 · untracked 1\na.rs +1\nnew.md +1\n== 2 files +2\n"
    );
    let (code, out, _) = acus_in(d.path(), &["diff", "--name-only"], "");
    assert_eq!(code, 0);
    assert_eq!(
        out,
        "on main · unstaged 1 · untracked 1\nM a.rs\n? new.md\n"
    );
    // A staged rename keeps its old and new path.
    let d = repo();
    std::fs::write(d.path().join("a.rs"), "one\n").unwrap();
    commit(d.path(), "one");
    git(d.path(), &["mv", "a.rs", "b.rs"]);
    let (code, out, _) = acus_in(d.path(), &["diff", "--staged", "--name-only"], "");
    assert_eq!(code, 0);
    assert_eq!(out, "on main · staged 1\nR a.rs -> b.rs\n");
}

#[test]
fn a_revision_range_prints_no_header() {
    let d = repo();
    std::fs::write(d.path().join("a.rs"), "one\n").unwrap();
    commit(d.path(), "one");
    std::fs::write(d.path().join("a.rs"), "one\ntwo\n").unwrap();
    commit(d.path(), "two");
    let (code, out, _) = acus_in(d.path(), &["diff", "HEAD~1..HEAD", "--stat"], "");
    assert_eq!(code, 0);
    assert_eq!(out, "a.rs +1\n== 1 file +1\n");
    let (code, out, _) = acus_in(d.path(), &["diff", "HEAD~1..HEAD", "--name-only"], "");
    assert_eq!(code, 0);
    assert_eq!(out, "M a.rs\n");
    let (code, out, _) = acus_in(d.path(), &["diff", "HEAD~1..HEAD", "--json"], "");
    assert_eq!(code, 0);
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert!(v.get("header").is_none(), "{out}");
    assert_eq!(v["files"][0]["path"], "a.rs");
}

#[test]
fn json_header_fields_match_the_text_header() {
    let d = repo();
    std::fs::write(d.path().join("a.rs"), "one\n").unwrap();
    commit(d.path(), "one");
    std::fs::write(d.path().join("a.rs"), "two\n").unwrap();
    std::fs::write(d.path().join("staged.txt"), "s\n").unwrap();
    git(d.path(), &["add", "staged.txt"]);
    std::fs::write(d.path().join("untracked.txt"), "u\n").unwrap();
    let (code, out, _) = acus_in(d.path(), &["diff", "--json"], "");
    assert_eq!(code, 0);
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["header"]["branch"], "main");
    assert_eq!(v["header"]["staged"], 1);
    assert_eq!(v["header"]["unstaged"], 1);
    assert_eq!(v["header"]["untracked"], 1);
    assert_eq!(v["header"]["conflicts"], 0);
    assert!(v["files"].is_array(), "{out}");
    // `--stat` keeps the per-file keys and skips the symbol groups.
    let (code, out, _) = acus_in(d.path(), &["diff", "--stat", "--json"], "");
    assert_eq!(code, 0);
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["files"][0]["symbols"], serde_json::json!([]));
}
