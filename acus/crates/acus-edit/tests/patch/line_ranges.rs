// SPDX-License-Identifier: MPL-2.0
use super::*;

#[test]
fn delete_lines_cuts_the_range_after_its_expect_matches() {
    let d = dir(&[("a.txt", "a\nb\nc\nd\n")]);
    let c = run(
        d.path(),
        "*** Begin Patch\n*** Delete Lines: a.txt:2-3\n*** Expect: b\n*** End Patch\n",
    )
    .unwrap();
    assert_eq!(read(d.path(), "a.txt"), "a\nd\n");
    let s: Vec<_> = c.iter().map(ToString::to_string).collect();
    assert_eq!(s, ["M a.txt +0 -2"]);
}

#[test]
fn expect_ignores_trailing_whitespace_only() {
    let d = dir(&[("a.txt", "a  \nb\n")]);
    run(d.path(), "*** Delete Lines: a.txt:1\n*** Expect: a\n").unwrap();
    assert_eq!(read(d.path(), "a.txt"), "b\n");
}

#[test]
fn move_lines_up_and_down_within_a_file() {
    let d = dir(&[("a.txt", "a\nb\nc\nd\ne\n")]);
    run(
        d.path(),
        "*** Move Lines: a.txt:4-5\n*** Expect: d\n*** After: a.txt:1\n",
    )
    .unwrap();
    assert_eq!(read(d.path(), "a.txt"), "a\nd\ne\nb\nc\n");
    let d = dir(&[("a.txt", "a\nb\nc\nd\ne\n")]);
    run(
        d.path(),
        "*** Move Lines: a.txt:1-2\n*** Expect: a\n*** After: a.txt:3\n",
    )
    .unwrap();
    assert_eq!(read(d.path(), "a.txt"), "c\na\nb\nd\ne\n");
}

#[test]
fn move_lines_to_the_start_of_the_file_with_after_zero() {
    let d = dir(&[("a.txt", "a\nb\nc\n")]);
    run(
        d.path(),
        "*** Move Lines: a.txt:3-3\n*** Expect: c\n*** After: a.txt:0\n",
    )
    .unwrap();
    assert_eq!(read(d.path(), "a.txt"), "c\na\nb\n");
}

#[test]
fn move_lines_across_files() {
    let d = dir(&[("a.txt", "a\nb\nc\n"), ("b.txt", "x\ny\n")]);
    let c = run(
        d.path(),
        "*** Move Lines: a.txt:2-2\n*** Expect: b\n*** After: b.txt:1\n",
    )
    .unwrap();
    assert_eq!(read(d.path(), "a.txt"), "a\nc\n");
    assert_eq!(read(d.path(), "b.txt"), "x\nb\ny\n");
    let s: Vec<_> = c.iter().map(ToString::to_string).collect();
    assert_eq!(s, ["M a.txt +0 -1", "M b.txt +1 -0"]);
}

#[test]
fn move_lines_after_a_symbol_goes_below_its_last_line() {
    let d = dir(&[
        ("a.rs", "fn one() {\n    1\n}\n\nfn two() {\n    2\n}\n"),
        ("notes.txt", "const X: u8 = 1;\n"),
    ]);
    run(
        d.path(),
        "*** Move Lines: notes.txt:1-1\n*** Expect: const X: u8 = 1;\n*** After: a.rs#two\n",
    )
    .unwrap();
    assert_eq!(
        read(d.path(), "a.rs"),
        "fn one() {\n    1\n}\n\nfn two() {\n    2\n}\nconst X: u8 = 1;\n"
    );
    assert_eq!(read(d.path(), "notes.txt"), "");
}

#[test]
fn expect_mismatch_refuses_and_writes_nothing() {
    let d = dir(&[("a.txt", "a\nb\nc\n")]);
    let e = run(d.path(), "*** Delete Lines: a.txt:2-2\n*** Expect: x\n")
        .unwrap_err()
        .shown();
    assert!(e.contains("but line 2 is `b`"), "{e}");
    assert_eq!(read(d.path(), "a.txt"), "a\nb\nc\n");
}

#[test]
fn overlapping_ranges_are_refused() {
    let d = dir(&[("a.txt", "a\nb\nc\nd\n")]);
    let e = run(
        d.path(),
        "*** Delete Lines: a.txt:1-2\n*** Expect: a\n*** Delete Lines: a.txt:2-3\n*** Expect: b\n",
    )
    .unwrap_err()
    .shown();
    assert!(e.contains("1-2 and 2-3 overlap"), "{e}");
    assert_eq!(read(d.path(), "a.txt"), "a\nb\nc\nd\n");
}

#[test]
fn ranges_past_the_end_of_the_file_are_refused() {
    let d = dir(&[("a.txt", "a\nb\nc\n")]);
    let e = run(d.path(), "*** Delete Lines: a.txt:2-9\n*** Expect: b\n")
        .unwrap_err()
        .shown();
    assert!(e.contains("a.txt:2-9: the file has 3 lines"), "{e}");
    assert_eq!(read(d.path(), "a.txt"), "a\nb\nc\n");
}

#[test]
fn a_move_target_inside_the_moved_lines_is_refused() {
    let d = dir(&[("a.txt", "a\nb\nc\nd\n")]);
    let e = run(
        d.path(),
        "*** Move Lines: a.txt:2-3\n*** Expect: b\n*** After: a.txt:3\n",
    )
    .unwrap_err()
    .shown();
    assert!(e.contains("inside the moved lines 2-3"), "{e}");
    assert_eq!(read(d.path(), "a.txt"), "a\nb\nc\nd\n");
}

#[test]
fn malformed_line_directives_are_refused_by_parse() {
    let e = parse("*** Delete Lines: a.txt:3-2\n*** Expect: c\n")
        .unwrap_err()
        .shown();
    assert!(e.contains("1 <= A <= B"), "{e}");
    let e = parse("*** Delete Lines: a.txt:0-2\n*** Expect: c\n")
        .unwrap_err()
        .shown();
    assert!(e.contains("1 <= A <= B"), "{e}");
    let e = parse("*** Delete Lines: a.txt:1-1\n*** Delete File: x\n*** Expect: a\n")
        .unwrap_err()
        .shown();
    assert!(e.contains("right after"), "{e}");
    let e = parse("*** Delete Lines: a.txt:1-1\n").unwrap_err().shown();
    assert!(e.contains("needs `*** Expect:`"), "{e}");
    let e = parse("*** Move Lines: a.txt:1-1\n*** Expect: a\n")
        .unwrap_err()
        .shown();
    assert!(e.contains("needs `*** After:`"), "{e}");
    let e = parse("*** Delete Lines: a.txt:1-1\n*** Expect: a\n+x\n")
        .unwrap_err()
        .shown();
    assert!(e.contains("unexpected content"), "{e}");
}

#[test]
fn line_directives_combine_with_search_replace_in_one_patch() {
    let d = dir(&[("a.txt", "a\nb\nc\nd\n")]);
    run(
        d.path(),
        "*** Update File: a.txt\n<<<<<<< SEARCH\nb\n=======\nb1\nb2\n>>>>>>> REPLACE\n\
         *** Delete Lines: a.txt:4-4\n*** Expect: d\n",
    )
    .unwrap();
    assert_eq!(read(d.path(), "a.txt"), "a\nb1\nb2\nc\n");
}

#[test]
fn line_directives_keep_crlf_line_endings() {
    let d = dir(&[("a.txt", "a\r\nb\r\nc\r\n")]);
    run(d.path(), "*** Delete Lines: a.txt:2\n*** Expect: b\n").unwrap();
    assert_eq!(read(d.path(), "a.txt"), "a\r\nc\r\n");
    let d = dir(&[("a.txt", "a\r\nb\r\nc\r\n")]);
    run(
        d.path(),
        "*** Move Lines: a.txt:1\n*** Expect: a\n*** After: a.txt:3\n",
    )
    .unwrap();
    assert_eq!(read(d.path(), "a.txt"), "b\r\nc\r\na\r\n");
}
