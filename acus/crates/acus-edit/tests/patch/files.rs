// SPDX-License-Identifier: MPL-2.0
use super::*;

#[test]
fn unknown_directives_list_the_accepted_forms() {
    let e = |p: &str| parse(p).unwrap_err().shown();
    let m = e("*** Replace all: src\n");
    assert!(
        m.contains("did you mean `*** Replace All: PATH [PATH...]`"),
        "{m}"
    );
    assert!(
        m.contains("*** Replace All: PATH [PATH...] (files, directories or globs"),
        "{m}"
    );
    assert!(m.contains("`<<<<<<< SEARCH` or `<<<<<<< REGEX`"), "{m}");
    assert!(m.contains("*** Update File: PATH"), "{m}");
    for form in [
        "*** Copy File: PATH (then *** To: PATH)",
        "*** Insert Before: PATH#Symbol or PATH:LINE",
        "*** Insert After: PATH#Symbol or PATH:LINE",
    ] {
        assert!(m.contains(form), "{m}");
    }
    assert!(e("*** insert after: a.rs#f\n").contains("did you mean `*** Insert After:"));
    assert!(e("*** copy file: a.rs\n").contains("did you mean `*** Copy File: PATH`"));
    assert!(e("*** Replace All:src\n").contains("did you mean"));
    assert!(e("*** Replace All:\n").contains("did you mean"));
    assert!(e("*** update file: a.rs\n").contains("did you mean `*** Update File: PATH`"));
    let m = e("*** Bogus: x\n");
    assert!(
        !m.contains("did you mean") && m.contains("accepted: *** Add File: PATH"),
        "{m}"
    );
    // A REGEX block only exists under Replace All.
    let m = e("*** Update File: a.rs\n<<<<<<< REGEX\nx\n=======\ny\n>>>>>>> REPLACE\n");
    assert!(
        m.contains("REGEX blocks belong under `*** Replace All: PATH`"),
        "{m}"
    );
}

#[test]
fn append_file_adds_after_the_last_line() {
    let d = dir(&[("with.txt", "a\n"), ("without.txt", "a")]);
    let c = run(
        d.path(),
        "*** Append File: with.txt\n+b\n+c\n*** Append File: without.txt\n+b\n",
    )
    .unwrap();
    assert_eq!(read(d.path(), "with.txt"), "a\nb\nc\n");
    // No trailing newline before the patch means none after it, but one still separates the lines.
    assert_eq!(read(d.path(), "without.txt"), "a\nb");
    let s: Vec<_> = c.iter().map(ToString::to_string).collect();
    assert_eq!(s, ["M with.txt +2 -0", "M without.txt +2 -1"]);
}

#[test]
fn append_file_to_a_missing_file_needs_add_file() {
    let d = dir(&[]);
    let e = run(d.path(), "*** Append File: nope.txt\n+x\n")
        .unwrap_err()
        .shown();
    assert!(e.contains("nope.txt: file not found"), "{e}");
    assert!(e.contains("use `*** Add File: nope.txt`"), "{e}");
    assert!(!d.path().join("nope.txt").exists());
}

#[test]
fn insert_mixes_with_other_ops_all_or_nothing() {
    let d = dir(&[("a.rs", "fn one() {}\n"), ("b.txt", "keep\n")]);
    let e = run(
        d.path(),
        "*** Update File: b.txt\n<<<<<<< SEARCH\nkeep\n=======\nchanged\n>>>>>>> REPLACE\n*** Insert After: a.rs#missing\n+x\n",
    )
    .unwrap_err();
    assert!(e.to_string().contains("no symbol `missing`"), "{e:#}");
    assert_eq!(read(d.path(), "b.txt"), "keep\n");
}

#[test]
fn copy_file_then_update_the_copy() {
    let d = dir(&[("a.rs", "fn one() {}\r\n"), ("b.txt", "x\n")]);
    let c = run(
        d.path(),
        "*** Copy File: a.rs\n*** To: sub/c.rs\n*** Update File: sub/c.rs\n<<<<<<< SEARCH\nfn one() {}\n=======\nfn two() {}\n>>>>>>> REPLACE\n",
    )
    .unwrap();
    assert_eq!(read(d.path(), "a.rs"), "fn one() {}\r\n");
    assert_eq!(read(d.path(), "sub/c.rs"), "fn two() {}\r\n");
    let s: Vec<_> = c.iter().map(ToString::to_string).collect();
    assert_eq!(s, ["A sub/c.rs +1"]);
    let e = |p: &str| run(d.path(), p).unwrap_err().shown();
    assert!(e("*** Copy File: a.rs\n*** To: b.txt\n").contains("already exists"));
    assert!(e("*** Copy File: nope.rs\n*** To: n.rs\n").contains("nope.rs"));
    assert!(
        parse("*** Copy File: a.rs\n")
            .unwrap_err()
            .shown()
            .contains("needs a destination")
    );
    assert!(
        parse("*** To: x\n")
            .unwrap_err()
            .shown()
            .contains("`*** Copy File:`")
    );
    assert!(!d.path().join("n.rs").exists());
}

#[test]
fn add_delete_move() {
    let d = dir(&[("old.txt", "bye\n"), ("m.txt", "a\nb\n")]);
    let c = run(
        d.path(),
        "*** Begin Patch\n*** Add File: new/n.txt\n+hello\n+world\n*** Delete File: old.txt\n*** Update File: m.txt\n*** Move to: moved.txt\n<<<<<<< SEARCH\nb\n=======\nc\n>>>>>>> REPLACE\n*** End Patch",
    )
    .unwrap();
    assert_eq!(read(d.path(), "new/n.txt"), "hello\nworld\n");
    assert!(!d.path().join("old.txt").exists());
    assert!(!d.path().join("m.txt").exists());
    assert_eq!(read(d.path(), "moved.txt"), "a\nc\n");
    let s: Vec<String> = c.iter().map(ToString::to_string).collect();
    assert_eq!(
        s,
        [
            "A new/n.txt +2",
            "D old.txt -1",
            "R m.txt -> moved.txt +1 -1"
        ]
    );
}

// The counts match `git diff --numstat -M` on the same change: `2 2 a.txt => m.txt`.
#[test]
fn edit_then_move_gives_one_line_with_net_counts() {
    let d = dir(&[("a.txt", "a\nb\nc\nd\n")]);
    let c = run(
        d.path(),
        "*** Update File: a.txt\n<<<<<<< SEARCH\nb\n=======\nx\n>>>>>>> REPLACE\n*** Update File: a.txt\n*** Move to: m.txt\n<<<<<<< SEARCH\nc\n=======\ny\n>>>>>>> REPLACE\n",
    )
    .unwrap();
    assert!(!d.path().join("a.txt").exists());
    assert_eq!(read(d.path(), "m.txt"), "a\nx\ny\nd\n");
    let s: Vec<String> = c.iter().map(ToString::to_string).collect();
    assert_eq!(s, ["R a.txt -> m.txt +2 -2"]);
}

// The counts match `git diff --numstat -M` on the same change: `2 2 a.txt => m.txt`.
#[test]
fn move_then_edit_gives_one_line_with_net_counts() {
    let d = dir(&[("a.txt", "a\nb\nc\nd\n")]);
    let c = run(
        d.path(),
        "*** Update File: a.txt\n*** Move to: m.txt\n<<<<<<< SEARCH\nb\n=======\nx\n>>>>>>> REPLACE\n*** Update File: m.txt\n<<<<<<< SEARCH\nc\n=======\ny\n>>>>>>> REPLACE\n",
    )
    .unwrap();
    assert!(!d.path().join("a.txt").exists());
    assert_eq!(read(d.path(), "m.txt"), "a\nx\ny\nd\n");
    let s: Vec<String> = c.iter().map(ToString::to_string).collect();
    assert_eq!(s, ["R a.txt -> m.txt +2 -2"]);
}

#[test]
fn preserves_crlf_and_missing_trailing_newline() {
    let d = dir(&[("w.txt", "a\r\nb\r\nc")]);
    run(
        d.path(),
        "*** Update File: w.txt\n<<<<<<< SEARCH\nb\n=======\nB\n>>>>>>> REPLACE\n",
    )
    .unwrap();
    assert_eq!(read(d.path(), "w.txt"), "a\r\nB\r\nc");
}

#[test]
fn check_mode_writes_nothing() {
    let d = dir(&[("a.txt", "a\n")]);
    let ops =
        parse("*** Update File: a.txt\n<<<<<<< SEARCH\na\n=======\nb\n>>>>>>> REPLACE\n").unwrap();
    apply(&ops, d.path(), true).unwrap();
    assert_eq!(read(d.path(), "a.txt"), "a\n");
}

#[test]
fn applied_files_carry_text_before_and_after() {
    let d = dir(&[("a.txt", "a\n"), ("gone.txt", "x\n")]);
    let ops = parse(
        "*** Update File: a.txt\n<<<<<<< SEARCH\na\n=======\nb\n>>>>>>> REPLACE\n*** Delete File: gone.txt\n*** Add File: new.txt\n+hi\n",
    )
    .unwrap();
    let applied = apply_with(&ops, d.path(), true, false).unwrap();
    let text = |p: &str| applied.texts.iter().find(|t| t.path == p).unwrap();
    assert_eq!(text("a.txt").before.as_deref(), Some("a\n"));
    assert_eq!(text("a.txt").after.as_deref(), Some("b\n"));
    assert_eq!(text("gone.txt").before.as_deref(), Some("x\n"));
    assert_eq!(text("gone.txt").after, None);
    assert_eq!(text("new.txt").before, None);
    assert_eq!(text("new.txt").after.as_deref(), Some("hi\n"));
}

#[test]
fn parse_errors() {
    assert!(parse("*** Frobnicate: x\n").is_err());
    assert!(parse("*** Add File: x\nnot plus\n").is_err());
    assert!(parse("").is_err());
}

#[cfg(unix)]
#[test]
fn keeps_permissions() {
    use std::os::unix::fs::PermissionsExt;
    let d = dir(&[("s.sh", "echo a\n")]);
    let p = d.path().join("s.sh");
    fs::set_permissions(&p, fs::Permissions::from_mode(0o755)).unwrap();
    run(
        d.path(),
        "*** Update File: s.sh\n<<<<<<< SEARCH\necho a\n=======\necho b\n>>>>>>> REPLACE\n",
    )
    .unwrap();
    assert_eq!(
        fs::metadata(&p).unwrap().permissions().mode() & 0o777,
        0o755
    );
}

#[test]
fn reports_written_regions_with_final_line_numbers() {
    let d = dir(&[("a.txt", "1\n2\n3\n4\n5\n6\n")]);
    // The second block sits above the first one; its insertion shifts the first region down.
    let p = "*** Begin Patch\n*** Update File: a.txt\n<<<<<<< SEARCH\n5\n=======\nfive\nFIVE\n>>>>>>> REPLACE\n*** Update File: a.txt\n<<<<<<< SEARCH\n2\n=======\n2\none-and-a-half\n>>>>>>> REPLACE\n*** End Patch\n";
    let r = apply(&parse(p).unwrap(), d.path(), true).unwrap().regions;
    let got: Vec<_> = r.iter().map(|r| (r.start, r.lines.join(","))).collect();
    assert_eq!(
        got,
        [(2, "2,one-and-a-half".into()), (6, "five,FIVE".into())]
    );
    assert_eq!(
        read(d.path(), "a.txt"),
        "1\n2\n3\n4\n5\n6\n",
        "check wrote nothing"
    );
}
