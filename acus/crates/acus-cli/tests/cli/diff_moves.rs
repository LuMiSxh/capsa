// SPDX-License-Identifier: MPL-2.0
use crate::common::*;

fn repo() -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    git(d.path(), &["init", "-q", "-b", "main"]);
    d
}

fn commit(d: &std::path::Path) {
    git(d, &["add", "."]);
    git(d, &["commit", "-qm", "x"]);
}

fn changed_lines(out: &str) -> usize {
    out.lines()
        .filter(|l| {
            l.starts_with('\t')
                || l.split_once('\t')
                    .is_some_and(|(_, rest)| rest.starts_with('+'))
        })
        .count()
}

#[test]
fn diff_outlines_added_files_and_names_the_body() {
    let d = repo();
    std::fs::write(d.path().join("keep.rs"), "fn keep() {}\n").unwrap();
    commit(d.path());
    let big: String = (1..=30)
        .map(|i| format!("fn f{i}() {{\n    body{i}();\n}}\n\n"))
        .collect();
    std::fs::write(d.path().join("new.rs"), &big).unwrap();
    let notes: String = (1..=25).map(|i| format!("line {i}\n")).collect();
    std::fs::write(d.path().join("notes.txt"), notes).unwrap();
    let (code, out, err) = acus_in(d.path(), &["diff", "-p"], "");
    assert_eq!((code, err.as_str()), (0, ""));
    assert!(out.contains("? new.rs +120\n  fn f1 1-3\n"), "{out}");
    assert!(out.contains("… body: acus show new.rs\n"), "{out}");
    assert!(!out.contains("\t+fn f1() {"), "{out}");
    assert!(out.contains("? notes.txt +25\n1\t+line 1\n"), "{out}");
    assert!(
        out.contains("20\t+line 20\n… body: acus show notes.txt\n"),
        "{out}"
    );
    assert!(!out.contains("21\t+line 21"), "{out}");
    // JSON keeps every changed line and the old grouping.
    let (code, json, _) = acus_in(d.path(), &["diff", "-p", "--json"], "");
    assert_eq!(code, 0);
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    let new = v["files"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["path"] == "new.rs")
        .unwrap();
    assert_eq!(new["symbols"][0]["lines"].as_array().unwrap().len(), 120);
    // `--full` prints the body again.
    let (code, full, _) = acus_in(d.path(), &["diff", "-p", "--full", "new.rs"], "");
    assert_eq!(code, 0);
    assert!(full.contains("? new.rs +120\n1\t+fn f1() {"), "{full}");
    assert!(!full.contains("… body:"), "{full}");
    assert!(!full.contains("  fn f1 1-3"), "{full}");
}

#[test]
fn moves_across_files_print_the_source_summary_and_one_destination_line() {
    let d = repo();
    std::fs::write(
        d.path().join("a.rs"),
        "fn a() {\n    1;\n}\n\nfn b() {\n    2;\n}\n",
    )
    .unwrap();
    std::fs::write(d.path().join("b.rs"), "fn keep() {}\n").unwrap();
    commit(d.path());
    std::fs::write(d.path().join("a.rs"), "fn a() {\n    1;\n}\n").unwrap();
    std::fs::write(
        d.path().join("b.rs"),
        "fn keep() {}\n\nfn b() {\n    2;\n}\n",
    )
    .unwrap();
    let (code, out, _) = acus_in(d.path(), &["diff", "-p"], "");
    assert_eq!(code, 0);
    assert_eq!(
        out,
        "on main · unstaged 2\nM a.rs -4\n  (top level) -1\n  fn b (removed) -3 moved → b.rs\nM b.rs +4\n  (top level) +1\n  moved fn b ← a.rs:5-7 (unchanged)\n== 2 files +4 -4\n"
    );
}

#[test]
fn moves_into_an_added_file_mark_the_outline_row() {
    let d = repo();
    std::fs::write(
        d.path().join("a.rs"),
        "fn a() {\n    1;\n}\n\nfn b() {\n    2;\n}\n",
    )
    .unwrap();
    commit(d.path());
    std::fs::write(d.path().join("a.rs"), "fn a() {\n    1;\n}\n").unwrap();
    std::fs::write(d.path().join("b.rs"), "fn b() {\n    2;\n}\n").unwrap();
    let (code, out, _) = acus_in(d.path(), &["diff", "-p"], "");
    assert_eq!(code, 0);
    assert!(out.contains("  fn b (removed) -3 moved → b.rs\n"), "{out}");
    assert!(
        out.contains("? b.rs +3\n  moved fn b ← a.rs:5-7 (unchanged)\n"),
        "{out}"
    );
    assert!(out.contains("… body: acus show b.rs\n"), "{out}");
    assert!(!out.contains("\t+    2;"), "{out}");
}

#[test]
fn a_move_within_one_file_shows_both_sides() {
    let d = repo();
    std::fs::write(
        d.path().join("a.rs"),
        "fn a() {\n    1;\n}\n\nfn b() {\n    2;\n}\n\nfn c() {\n    3;\n}\n",
    )
    .unwrap();
    commit(d.path());
    std::fs::write(
        d.path().join("a.rs"),
        "fn b() {\n    2;\n}\n\nfn c() {\n    3;\n}\n\nfn a() {\n    1;\n}\n",
    )
    .unwrap();
    let (code, out, _) = acus_in(d.path(), &["diff", "-p"], "");
    assert_eq!(code, 0);
    assert!(out.contains("  fn a (removed) -3 moved → a.rs\n"), "{out}");
    assert!(
        out.contains("  moved fn a ← a.rs:1-3 (unchanged)\n"),
        "{out}"
    );
    assert!(!out.contains("\t-    1;"), "{out}");
    assert!(!out.contains("\t+    1;"), "{out}");
}

#[test]
fn an_edit_shifted_down_by_inserted_lines_is_not_a_move() {
    let d = repo();
    std::fs::write(
        d.path().join("a.rs"),
        "fn a() {\n    1;\n}\n\nfn f() -> u8 { 1 }\n",
    )
    .unwrap();
    commit(d.path());
    std::fs::write(
        d.path().join("a.rs"),
        "fn x1() {}\nfn x2() {}\nfn x3() {}\nfn x4() {}\nfn x5() {}\n\nfn a() {\n    1;\n}\n\nfn f() -> u8 { 2 }\n",
    )
    .unwrap();
    let (code, out, _) = acus_in(d.path(), &["diff", "-p"], "");
    assert_eq!(code, 0);
    assert!(!out.contains("moved ←"), "{out}");
    assert!(!out.contains("moved →"), "{out}");
    assert!(out.contains("  fn f 11-11 +1 -1\n"), "{out}");
}

#[test]
fn identical_functions_in_different_files_are_not_paired() {
    let d = repo();
    std::fs::write(d.path().join("a.rs"), "fn f() {\n  1;\n}\n").unwrap();
    std::fs::write(d.path().join("b.rs"), "fn f() {\n  1;\n}\n").unwrap();
    commit(d.path());
    std::fs::write(d.path().join("a.rs"), "").unwrap();
    std::fs::write(d.path().join("b.rs"), "fn f() {\n    1;\n}\n").unwrap();
    let (code, out, _) = acus_in(d.path(), &["diff", "-p"], "");
    assert_eq!(code, 0);
    assert!(out.contains("  fn f (removed) -3\n"), "{out}");
    assert!(!out.contains("moved ←"), "{out}");
    assert!(!out.contains("moved →"), "{out}");
}

#[test]
fn a_copy_that_stays_in_its_file_is_not_a_move() {
    let d = repo();
    std::fs::write(
        d.path().join("a.py"),
        "def f():\n    1\n\n\ndef f():\n    1\n",
    )
    .unwrap();
    std::fs::write(d.path().join("b.py"), "def keep():\n    2\n").unwrap();
    commit(d.path());
    std::fs::write(d.path().join("a.py"), "def f():\n    1\n").unwrap();
    std::fs::write(
        d.path().join("b.py"),
        "def keep():\n    2\n\n\ndef f():\n    1\n",
    )
    .unwrap();
    let (code, out, _) = acus_in(d.path(), &["diff", "-p"], "");
    assert_eq!(code, 0);
    assert!(!out.contains("moved ←"), "{out}");
    assert!(!out.contains("moved →"), "{out}");
}

#[test]
fn a_moved_and_edited_function_prints_only_the_body_diff() {
    let d = repo();
    std::fs::write(
        d.path().join("a.rs"),
        "fn a() {\n    1;\n}\n\nfn b() {\n    2;\n}\n",
    )
    .unwrap();
    std::fs::write(d.path().join("b.rs"), "fn keep() {}\n").unwrap();
    commit(d.path());
    std::fs::write(d.path().join("a.rs"), "fn a() {\n    1;\n}\n").unwrap();
    std::fs::write(
        d.path().join("b.rs"),
        "fn keep() {}\n\nfn b() {\n    3;\n}\n",
    )
    .unwrap();
    let (code, out, _) = acus_in(d.path(), &["diff", "-p"], "");
    assert_eq!(code, 0);
    assert!(out.contains("  moved fn b ← a.rs:5-7 (edited)\n"), "{out}");
    assert!(out.contains("\t-    2;\n4\t+    3;\n"), "{out}");
    assert!(!out.contains("\t-fn b() {"), "{out}");
}

#[test]
fn the_per_file_cap_names_the_rerun_and_full_lifts_it() {
    let d = repo();
    let old: String = (1..=100).map(|i| format!("old {i}\n")).collect();
    std::fs::write(d.path().join("cap.txt"), &old).unwrap();
    commit(d.path());
    let new: String = (1..=100).map(|i| format!("new {i}\n")).collect();
    std::fs::write(d.path().join("cap.txt"), &new).unwrap();
    let (code, out, _) = acus_in(d.path(), &["diff", "-p"], "");
    assert_eq!(code, 0);
    assert_eq!(changed_lines(&out), 80, "{out}");
    assert!(
        out.contains("… 120 more changed lines: acus diff -p --full -- cap.txt\n"),
        "{out}"
    );
    assert!(out.ends_with("== 1 file +100 -100\n"), "{out}");
    let (code, full, _) = acus_in(d.path(), &["diff", "-p", "--full"], "");
    assert_eq!(code, 0);
    assert_eq!(changed_lines(&full), 200, "{full}");
    assert!(!full.contains("more changed lines"), "{full}");
    // A ranged diff names the revision in the rerun.
    commit(d.path());
    let (code, ranged, _) = acus_in(d.path(), &["diff", "-p", "HEAD~1..HEAD"], "");
    assert_eq!(code, 0);
    assert!(
        ranged.contains("more changed lines: acus diff -p --full HEAD~1..HEAD -- cap.txt\n"),
        "{ranged}"
    );
}

#[test]
fn blank_changed_lines_are_counted_but_not_printed() {
    let d = repo();
    std::fs::write(d.path().join("a.rs"), "fn f() {\n}\n").unwrap();
    commit(d.path());
    std::fs::write(d.path().join("a.rs"), "fn f() {\n\n    x();\n\n}\n").unwrap();
    let (code, out, _) = acus_in(d.path(), &["diff", "-p"], "");
    assert_eq!(code, 0);
    assert_eq!(
        out,
        "on main · unstaged 1\nM a.rs +3\n  fn f 1-5 +3\n3\t+    x();\n== 1 file +3\n"
    );
    assert!(
        !out.lines()
            .any(|l| l.strip_prefix('\t').is_some_and(|t| t.trim().is_empty()))
    );
    let (_, full, _) = acus_in(d.path(), &["diff", "-p", "--full"], "");
    assert_eq!(changed_lines(&full), 3, "{full}");
    assert!(full.contains("2\t+\n"), "{full}");
}
