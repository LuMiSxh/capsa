// SPDX-License-Identifier: MPL-2.0
use crate::common::*;

fn file(d: &std::path::Path) {
    std::fs::write(
        d.join("a.rs"),
        (1..=30)
            .map(|i| format!("fn f{i}() {{}}\n"))
            .collect::<String>(),
    )
    .unwrap();
}

#[test]
fn show_head_cuts_and_names_the_rerun() {
    let d = tempfile::tempdir().unwrap();
    file(d.path());
    let (code, out, _) = acus_in(d.path(), &["show", "a.rs", "--head", "5"], "");
    assert_eq!(code, 0);
    snapshot_settings().bind(|| insta::assert_snapshot!("cli__budget_show_head", out));
}

#[test]
fn show_cut_clips_line_text() {
    let d = tempfile::tempdir().unwrap();
    file(d.path());
    let (_, out, _) = acus_in(d.path(), &["show", "a.rs:1-2", "--cut", "4"], "");
    snapshot_settings().bind(|| insta::assert_snapshot!("cli__budget_show_cut", out));
}

#[test]
fn find_head_counts_output_lines() {
    let d = tempfile::tempdir().unwrap();
    file(d.path());
    let (_, out, _) = acus_in(d.path(), &["find", "fn", "--head", "3"], "");
    assert_eq!(out.lines().count(), 4, "{out}"); // 3 + footer
    assert!(
        out.ends_with("(rerun with --head 31)\n") || out.contains("rerun with --head"),
        "{out}"
    );
}

#[test]
fn outline_and_diff_take_head() {
    let d = tempfile::tempdir().unwrap();
    file(d.path());
    let (_, out, _) = acus_in(d.path(), &["outline", "a.rs", "--head", "2"], "");
    assert!(out.contains("output cut at --head 2"), "{out}");
}
