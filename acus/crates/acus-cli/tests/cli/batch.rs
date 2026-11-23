// SPDX-License-Identifier: MPL-2.0
use super::common::{acus_in, acus_in_env};

/// A directory with two files of one word each, so each `find` has one hit.
fn two_files() -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join("a.rs"), "alpha\n").unwrap();
    std::fs::write(d.path().join("b.rs"), "beta\n").unwrap();
    d
}

#[test]
fn batch_runs_lines_and_prints_them_in_line_order() {
    let d = two_files();
    let input = "find beta\n# a comment\n\nacus find alpha\n";
    let (code, out, err) = acus_in(d.path(), &["batch"], input);
    assert_eq!((code, err.as_str()), (0, ""));
    assert_eq!(
        out,
        "== find beta\nb.rs\n1\tbeta\n== find alpha\na.rs\n1\talpha\n"
    );
}

#[test]
fn batch_keeps_the_other_lines_when_one_fails() {
    let d = two_files();
    let input = "find beta\nshow missing.rs\nfind alpha\n";
    let (code, out, err) = acus_in(d.path(), &["batch"], input);
    assert_eq!(code, 2, "{out}{err}");
    assert!(
        out.starts_with("== find beta\nb.rs\n1\tbeta\n== show missing.rs\n"),
        "{out}"
    );
    assert!(out.contains("== find alpha\na.rs\n1\talpha\n"), "{out}");
    assert!(out.contains("failed: 1 of 3 lines (line 2: "), "{out}");
    assert!(err.contains("1 of 3 lines failed"), "{err}");
}

#[test]
fn batch_refuses_patch_lines_and_names_acus_patch() {
    let d = two_files();
    let (code, out, _) = acus_in(d.path(), &["batch"], "find alpha\npatch\n");
    assert_eq!(code, 2, "{out}");
    assert!(out.contains("== find alpha\na.rs\n1\talpha\n"), "{out}");
    assert!(
        out.contains("== patch\nerror: `patch` cannot run in a batch\nhint: run `acus patch`"),
        "{out}"
    );
}

#[test]
fn batch_refuses_nested_batch_lines() {
    let d = two_files();
    let (code, out, _) = acus_in(d.path(), &["batch"], "batch\n");
    assert_eq!(code, 2, "{out}");
    assert!(out.contains("error: `batch` cannot be nested"), "{out}");
}

#[cfg(feature = "script")]
#[test]
fn batch_refuses_script_lines() {
    let d = two_files();
    let (code, out, _) = acus_in(d.path(), &["batch"], "script -e 'return 1'\n");
    assert_eq!(code, 2, "{out}");
    assert!(
        out.contains("error: `script` cannot run in a batch"),
        "{out}"
    );
}

#[test]
fn batch_checks_run_lines_with_the_guard() {
    let d = two_files();
    let (code, out, _) = acus_in(d.path(), &["batch"], "run 'cat a.rs'\n");
    assert_eq!(code, 2, "{out}");
    assert!(out.contains("the acus guard blocks it"), "{out}");
}

#[test]
fn batch_refuses_run_lines_a_bash_deny_rule_covers() {
    let d = two_files();
    std::fs::create_dir(d.path().join(".git")).unwrap();
    std::fs::create_dir(d.path().join(".claude")).unwrap();
    std::fs::write(
        d.path().join(".claude/settings.json"),
        r#"{"permissions":{"deny":["Bash(rm:*)"]}}"#,
    )
    .unwrap();
    let none = d.path().join("none.json");
    let managed = none.to_str().unwrap();
    let env = [("ACUS_MANAGED_SETTINGS", managed)];
    let input = "run 'if true; then rm -rf build; fi'\n";
    let (code, out, _) = acus_in_env(d.path(), &["batch"], input, &env);
    assert_eq!(code, 2, "{out}");
    assert!(out.contains("is denied by `Bash(rm:*)`"), "{out}");
}

#[test]
fn batch_exits_1_when_no_line_finds_anything() {
    let d = two_files();
    let (code, out, _) = acus_in(d.path(), &["batch"], "find zzz\n");
    assert_eq!(code, 1, "{out}");
    assert!(out.starts_with("== find zzz\n(no matches"), "{out}");
}

/// A directory with one text file of 30 hits, so `find hit` prints more lines than a `--head`.
fn many_hits() -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    let source: String = (1..=30).map(|i| format!("hit {i}\n")).collect();
    std::fs::write(d.path().join("big.txt"), source).unwrap();
    d
}

#[test]
fn batch_head_caps_each_line_and_names_the_rerun() {
    let d = many_hits();
    let (code, out, _) = acus_in(d.path(), &["batch", "--head", "3"], "find hit\n");
    assert_eq!(code, 0, "{out}");
    assert!(
        out.contains("… output cut at --head 3; 28 more lines: acus find hit --head 31\n"),
        "{out}"
    );
}

#[test]
fn batch_head_does_not_override_a_line_head() {
    let d = many_hits();
    let (code, out, _) = acus_in(d.path(), &["batch", "--head", "3"], "find hit --head 5\n");
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("… output cut at --head 5;"), "{out}");
    assert!(!out.contains("output cut at --head 3;"), "{out}");
}

#[test]
fn batch_cut_clips_each_printed_line_and_marks_it() {
    let d = many_hits();
    let (code, out, _) = acus_in(d.path(), &["batch", "--cut", "6"], "find hit\n");
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("1\thit … (+1 chars)\n"), "{out}");
    assert!(!out.contains("hit 10\n"), "{out}");
}

#[test]
fn batch_cut_does_not_override_a_line_cut() {
    let d = many_hits();
    let (code, out, _) = acus_in(d.path(), &["batch", "--cut", "6"], "find hit --cut 100\n");
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("hit 10\n"), "{out}");
}

#[test]
fn batch_defaults_neither_head_nor_cut_a_short_output() {
    let d = many_hits();
    let (code, out, _) = acus_in(d.path(), &["batch"], "find hit\n");
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("hit 30\n"), "{out}");
    assert!(!out.contains("cut at"), "{out}");
}
