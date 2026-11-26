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
fn batch_refuses_patch_and_nested_batch_lines() {
    let d = two_files();
    for (input, expected) in [
        (
            "find alpha\npatch\n",
            &[
                "== find alpha\na.rs\n1\talpha\n",
                "== patch\nerror: `patch` cannot run in a batch\nhint: run `acus patch`",
            ][..],
        ),
        ("batch\n", &["error: `batch` cannot be nested"][..]),
    ] {
        let (code, out, _) = acus_in(d.path(), &["batch"], input);
        assert_eq!(code, 2, "{input}: {out}");
        for want in expected {
            assert!(out.contains(want), "{input}: {out}");
        }
    }
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

    let denied = two_files();
    std::fs::create_dir(denied.path().join(".git")).unwrap();
    std::fs::create_dir(denied.path().join(".claude")).unwrap();
    std::fs::write(
        denied.path().join(".claude/settings.json"),
        r#"{"permissions":{"deny":["Bash(rm:*)"]}}"#,
    )
    .unwrap();
    let none = denied.path().join("none.json");
    let managed = none.to_str().unwrap();
    let env = [("ACUS_MANAGED_SETTINGS", managed)];
    let input = "run 'if true; then rm -rf build; fi'\n";
    let (code, out, _) = acus_in_env(denied.path(), &["batch"], input, &env);
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
fn batch_applies_head_and_cut_per_line() {
    let d = many_hits();
    for (args, input, present, absent) in [
        (
            vec!["--head", "3"],
            "find hit\n",
            vec!["… output cut at --head 3; 28 more lines: acus find hit --head 31\n"],
            vec![],
        ),
        (
            vec!["--head", "3"],
            "find hit --head 5\n",
            vec!["… output cut at --head 5;"],
            vec!["output cut at --head 3;"],
        ),
        (
            vec!["--cut", "6"],
            "find hit\n",
            vec!["1\thit … (+1 chars)\n"],
            vec!["hit 10\n"],
        ),
        (
            vec!["--cut", "6"],
            "find hit --cut 100\n",
            vec!["hit 10\n"],
            vec![],
        ),
        (vec![], "find hit\n", vec!["hit 30\n"], vec!["cut at"]),
    ] {
        let mut full = vec!["batch"];
        full.extend(&args);
        let (code, out, _) = acus_in(d.path(), &full, input);
        assert_eq!(code, 0, "{args:?}: {out}");
        for want in present {
            assert!(out.contains(want), "{args:?}: {out}");
        }
        for unwanted in absent {
            assert!(!out.contains(unwanted), "{args:?}: {out}");
        }
    }
}
