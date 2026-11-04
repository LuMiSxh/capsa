// SPDX-License-Identifier: MPL-2.0
use super::*;

#[test]
fn context_repeated_in_replace_is_not_counted() {
    let d = dir(&[(
        "a.rs",
        "fn f() {\n    let x = 1;\n    let y = 2;\n    let z = 3;\n}\n",
    )]);
    let c = run(
        d.path(),
        "*** Begin Patch\n*** Update File: a.rs\n<<<<<<< SEARCH\n    let x = 1;\n    let y = 2;\n    let z = 3;\n=======\n    let x = 1;\n    let y = 2;\n    let z = 3;\n    let w = 4;\n    let v = 5;\n>>>>>>> REPLACE\n*** End Patch\n",
    )
    .unwrap();
    assert_eq!(c[0].to_string(), "M a.rs +2 -0");
    no_temp_files(d.path());
}

#[test]
fn pure_replacement_counts_removed_and_added_lines() {
    let d = dir(&[("a.rs", "fn f() {\n    let y = 2;\n    let z = 3;\n}\n")]);
    let c = run(
        d.path(),
        "*** Begin Patch\n*** Update File: a.rs\n<<<<<<< SEARCH\n    let y = 2;\n    let z = 3;\n=======\n    let y = 20;\n    let z = 30;\n>>>>>>> REPLACE\n*** End Patch\n",
    )
    .unwrap();
    assert_eq!(c[0].to_string(), "M a.rs +2 -2");
}

#[test]
fn search_block_adds_lines_after_a_unique_match() {
    let d = dir(&[(
        "a.rs",
        "fn one() {\n    let x = 1;\n}\n\nfn two() {\n    let x = 1;\n}\n",
    )]);
    let c = run(
        d.path(),
        "*** Begin Patch\n*** Update File: a.rs\n<<<<<<< SEARCH\nfn two() {\n    let x = 1;\n=======\nfn two() {\n    let x = 2;\n    let y = 3;\n>>>>>>> REPLACE\n*** End Patch\n",
    )
    .unwrap();
    assert_eq!(
        read(d.path(), "a.rs"),
        "fn one() {\n    let x = 1;\n}\n\nfn two() {\n    let x = 2;\n    let y = 3;\n}\n"
    );
    assert_eq!(c[0].to_string(), "M a.rs +2 -1");
    no_temp_files(d.path());
}

#[test]
fn ambiguous_context_is_rejected_and_nothing_written() {
    let d = dir(&[("a.rs", "x\ny\nx\n"), ("b.rs", "keep\n")]);
    let err = run(
        d.path(),
        "*** Update File: b.rs\n<<<<<<< SEARCH\nkeep\n=======\nchanged\n>>>>>>> REPLACE\n*** Update File: a.rs\n<<<<<<< SEARCH\nx\n=======\nz\n>>>>>>> REPLACE\n",
    )
    .unwrap_err().shown();
    assert!(err.contains("a.rs") && err.contains("lines 1, 3"), "{err}");
    assert_eq!(read(d.path(), "b.rs"), "keep\n", "all or nothing");
    no_temp_files(d.path());
}

#[test]
fn missing_search_names_the_line() {
    let d = dir(&[("a.rs", "a\n")]);
    let err = run(
        d.path(),
        "*** Update File: a.rs\n<<<<<<< SEARCH\nnope\n=======\nx\n>>>>>>> REPLACE\n",
    )
    .unwrap_err()
    .shown();
    assert!(err.contains("a.rs") && err.contains("`nope`"), "{err}");
}

#[test]
fn search_blocks_are_all_or_nothing() {
    let d = dir(&[("a.txt", "one two\nthree\n")]);
    let p = "*** Update File: a.txt\n<<<<<<< SEARCH\none\n=======\n1\n>>>>>>> REPLACE\n<<<<<<< SEARCH\nnope\n=======\n2\n>>>>>>> REPLACE\n";
    assert!(run(d.path(), p).is_err());
    assert_eq!(read(d.path(), "a.txt"), "one two\nthree\n");
}

#[test]
fn search_ignores_trailing_whitespace_only_when_exact_text_is_missing() {
    // SEARCH blocks ignore trailing whitespace when the exact text does not occur.
    let d = dir(&[("a.rs", "fn f() {\n    a();   \n}\n")]);
    let a = apply(
        &parse("*** Update File: a.rs\n<<<<<<< SEARCH\n    a();\n}\n=======\n    b();\n}\n>>>>>>> REPLACE\n")
            .unwrap(),
        d.path(),
        false,
    )
    .unwrap();
    assert_eq!(read(d.path(), "a.rs"), "fn f() {\n    b();\n}\n");
    assert_eq!(a.trailing, ["a.rs block 1"]);
}

#[test]
fn context_not_found_names_the_nearest_region() {
    let lines: Vec<String> = (1..=30).map(|i| format!("filler line {i}")).collect();
    let mut text = lines.join("\n");
    text.push_str("\nfn target(a: u8) {\n    let value = compute(a);\n    finish(value);\n}\n");
    let d = dir(&[("src/a.rs", &text)]);
    let e = run(
        d.path(),
        "*** Update File: src/a.rs\n<<<<<<< SEARCH\nfn target(a: u32) {\n    let value = compute(a, 1);\n    finish(value);\n=======\nfn target(a: u32) {\n    done();\n>>>>>>> REPLACE\n",
    )
    .unwrap_err().shown();
    assert!(e.contains("nearest: src/a.rs:31-33\n31\tfn target(a: u8) {\n32\t    let value = compute(a);\n33\t    finish(value);"), "{e}");
    // Nothing alike: no nearest line.
    let e = run(
        d.path(),
        "*** Update File: src/a.rs\n<<<<<<< SEARCH\nqqqq zzzz\n=======\nx\n>>>>>>> REPLACE\n",
    )
    .unwrap_err()
    .shown();
    assert!(!e.contains("nearest:"), "{e}");
    // Long regions are capped.
    let text: String = (1..=12).map(|i| format!("row number {i}\n")).collect();
    let d = dir(&[("b.txt", &text)]);
    let patch: String = (1..=12).map(|i| format!("row numbr {i}\n")).collect();
    let e = run(
        d.path(),
        &format!("*** Update File: b.txt\n<<<<<<< SEARCH\n{patch}=======\nx\n>>>>>>> REPLACE\n"),
    )
    .unwrap_err()
    .shown();
    assert!(
        e.contains("nearest: b.txt:1-12") && e.contains("… 4 more lines"),
        "{e}"
    );
    assert!(!e.contains("\n9\t"), "{e}");
}

#[test]
fn search_replace_blocks_inside_update() {
    let d = dir(&[(
        "a.rs",
        "fn one() {\n    a();\n    b();\n}\n\nfn two() {\n    a();\n}\n",
    )]);
    let c = run(
        d.path(),
        "*** Update File: a.rs\n<<<<<<< SEARCH\nfn two() {\n    a();\n=======\nfn two() {\n    z();\n>>>>>>> REPLACE\n<<<<<<<\n    a();\n    b();\n=======\n    b();\n>>>>>>>\n",
    )
    .unwrap();
    assert_eq!(
        read(d.path(), "a.rs"),
        "fn one() {\n    b();\n}\n\nfn two() {\n    z();\n}\n"
    );
    assert_eq!(c[0].to_string(), "M a.rs +1 -2");
}

#[test]
fn search_replaces_text_inside_lines_and_across_lines() {
    let d = dir(&[("a.rs", "let x = old_name(1);\n")]);
    run(
        d.path(),
        "*** Update File: a.rs\n<<<<<<< SEARCH\nold_name\n=======\nnew_name\n>>>>>>> REPLACE\n",
    )
    .unwrap();
    assert_eq!(read(d.path(), "a.rs"), "let x = new_name(1);\n");

    // The text starts and ends in the middle of lines.
    let d = dir(&[("b.rs", "x = 1; y = 2;\nz = 3;\nw\n")]);
    let c = run(
        d.path(),
        "*** Update File: b.rs\n<<<<<<< SEARCH\ny = 2;\nz\n=======\nq\n>>>>>>> REPLACE\n",
    )
    .unwrap();
    assert_eq!(read(d.path(), "b.rs"), "x = 1; q = 3;\nw\n");
    assert_eq!(c[0].to_string(), "M b.rs +1 -2");
}

#[test]
fn search_must_match_exactly_once() {
    let d = dir(&[("a.txt", "x\nfoo\nx\nfoo\nfoo\n")]);
    let e = run(
        d.path(),
        "*** Update File: a.txt\n<<<<<<< SEARCH\nfoo\n=======\nbar\n>>>>>>> REPLACE\n",
    )
    .unwrap_err()
    .shown();
    assert!(
        e.contains("SEARCH matches 3 times (lines 2, 4, 5); add surrounding lines to make it unique, or use *** Replace All:"),
        "{e}"
    );
    assert_eq!(read(d.path(), "a.txt"), "x\nfoo\nx\nfoo\nfoo\n");

    let d = dir(&[("a.rs", "fn main() {\n    run(1);\n}\n")]);
    let e = run(
        d.path(),
        "*** Update File: a.rs\n<<<<<<< SEARCH\n    run(2);\n=======\n    run(3);\n>>>>>>> REPLACE\n",
    )
    .unwrap_err().shown();
    assert!(
        e.contains("block 1: SEARCH not found, first line `run(2);`"),
        "{e}"
    );
    assert!(e.contains("nearest: a.rs:2\n2\t    run(1);"), "{e}");
}

#[test]
fn search_falls_back_to_line_number_prefixes() {
    let d = dir(&[("a.rs", "fn a() {\n    x();\n}\n")]);
    let a = apply(
        &parse("*** Update File: a.rs\n<<<<<<< SEARCH\n2\t    x();\n=======\n2\t    y();\n>>>>>>> REPLACE\n").unwrap(),
        d.path(),
        false,
    )
    .unwrap();
    assert_eq!(read(d.path(), "a.rs"), "fn a() {\n    y();\n}\n");
    assert_eq!(a.numbered, ["a.rs block 1"]);
}

#[test]
fn search_must_not_be_empty_or_equal_to_replace() {
    let e = |p: &str| parse(p).unwrap_err().shown();
    let m = e("*** Update File: a.rs\n<<<<<<< SEARCH\n=======\nx\n>>>>>>> REPLACE\n");
    assert!(
        m.contains("empty SEARCH")
            && m.contains("*** Insert After: PATH#Symbol")
            && m.contains("*** Add File: PATH"),
        "{m}"
    );
    let m = e("*** Update File: a.rs\n<<<<<<< SEARCH\nx\n=======\nx\n>>>>>>> REPLACE\n");
    // A patch whose blocks are all identical has nothing to write.
    assert!(m.contains("SEARCH is identical to REPLACE"), "{m}");
}

#[test]
fn identical_block_is_skipped_with_a_note() {
    let d = dir(&[("a.rs", "let x = 1;\nlet y = 2;\n")]);
    let patch = "*** Update File: a.rs\n<<<<<<< SEARCH\nlet x = 1;\n=======\nlet x = 1;\n>>>>>>> REPLACE\n<<<<<<< SEARCH\nlet y = 2;\n=======\nlet y = 3;\n>>>>>>> REPLACE\n";
    let applied = apply(&parse(patch).unwrap(), d.path(), false).unwrap();
    assert_eq!(applied.skipped, vec![2]);
    assert_eq!(read(d.path(), "a.rs"), "let x = 1;\nlet y = 3;\n");
}

#[test]
fn identical_block_must_still_match_once() {
    let d = dir(&[("a.rs", "let x = 1;\n")]);
    let patch = "*** Update File: a.rs\n<<<<<<< SEARCH\nlet z = 1;\n=======\nlet z = 1;\n>>>>>>> REPLACE\n<<<<<<< SEARCH\nlet x = 1;\n=======\nlet x = 2;\n>>>>>>> REPLACE\n";
    let m = apply(&parse(patch).unwrap(), d.path(), false)
        .err()
        .unwrap()
        .to_string();
    assert!(m.contains("block 1: SEARCH not found"), "{m}");
    assert_eq!(read(d.path(), "a.rs"), "let x = 1;\n");
}

#[test]
fn identical_blocks_only_is_an_error() {
    let m = parse("*** Update File: a.rs\n<<<<<<< SEARCH\nx\n=======\nx\n>>>>>>> REPLACE\n<<<<<<< SEARCH\ny\n=======\ny\n>>>>>>> REPLACE\n")
        .unwrap_err().shown();
    assert!(m.contains("SEARCH is identical to REPLACE"), "{m}");
    assert!(parse("*** Update File: a.rs\n<<<<<<< SEARCH\nx\n=======\nx\n>>>>>>> REPLACE\n<<<<<<< SEARCH\ny\n=======\nz\n>>>>>>> REPLACE\n").is_ok());
}

#[test]
fn update_file_rejects_hunk_lines() {
    let d = dir(&[("a.txt", "a\n")]);
    for hunk in ["@@ a\n-a\n+b\n", " a\n+b\n", "-a\n+b\n"] {
        let e = run(d.path(), &format!("*** Update File: a.txt\n{hunk}"))
            .unwrap_err()
            .shown();
        assert!(
            e.contains(
                "patch line 2: *** Update File: takes SEARCH/REPLACE blocks, not @@/-/+ hunk lines"
            ),
            "{e}"
        );
        assert!(
            e.contains(
                "`<<<<<<< SEARCH` / `old text` / `=======` / `new text` / `>>>>>>> REPLACE`"
            ),
            "{e}"
        );
    }
    assert_eq!(read(d.path(), "a.txt"), "a\n");
}

#[test]
fn search_refuses_ambiguous_text_and_writes_nothing() {
    let src = "[x] foo\nfn a\n[y] foo\n";
    let d = dir(&[("a.txt", src)]);
    let e = run(
        d.path(),
        "*** Update File: a.txt\n<<<<<<< SEARCH\nfoo\n=======\nbar\n>>>>>>> REPLACE\n",
    )
    .unwrap_err()
    .shown();
    assert!(e.contains("SEARCH matches 2 times (lines 1, 3)"), "{e}");
    assert_eq!(read(d.path(), "a.txt"), src);
    // Uniqueness is counted over the whole file, so a later line can make the text unique.
    run(
        d.path(),
        "*** Update File: a.txt\n<<<<<<< SEARCH\n[y] foo\n=======\n[y] bar\n>>>>>>> REPLACE\n",
    )
    .unwrap();
    assert_eq!(read(d.path(), "a.txt"), "[x] foo\nfn a\n[y] bar\n");
    // Text inside one line that occurs twice is ambiguous too.
    let d = dir(&[("b.txt", "foo foo\n")]);
    let e = run(
        d.path(),
        "*** Update File: b.txt\n<<<<<<< SEARCH\nfoo\n=======\nbar\n>>>>>>> REPLACE\n",
    )
    .unwrap_err()
    .shown();
    assert!(e.contains("SEARCH matches 2 times (lines 1, 1)"), "{e}");
    assert_eq!(read(d.path(), "b.txt"), "foo foo\n");
}

#[test]
fn search_ignoring_indentation_writes_replace_in_the_file_indent() {
    // A tab-indented Go file, SEARCH written with spaces: REPLACE comes out with tabs.
    let d = dir(&[("a.go", "func f() {\n\tif x {\n\t\treturn 1\n\t}\n}\n")]);
    let a = apply(
        &parse("*** Update File: a.go\n<<<<<<< SEARCH\n    if x {\n        return 1\n    }\n=======\n    if x {\n        return 2\n    }\n>>>>>>> REPLACE\n").unwrap(),
        d.path(),
        false,
    )
    .unwrap();
    assert_eq!(
        read(d.path(), "a.go"),
        "func f() {\n\tif x {\n\t\treturn 2\n\t}\n}\n"
    );
    assert_eq!(a.indented, ["a.go block 1"]);
}

#[test]
fn search_ignoring_indentation_keeps_the_file_indent_unit() {
    // A 2-space file: SEARCH's 4-space levels match, and REPLACE gets 2-space levels.
    let d = dir(&[("a.py", "class A:\n  def f(self):\n    return 1\n")]);
    let a = apply(
        &parse("*** Update File: a.py\n<<<<<<< SEARCH\n    def f(self):\n        return 1\n=======\n    def f(self):\n        return 2\n        pass\n>>>>>>> REPLACE\n").unwrap(),
        d.path(),
        false,
    )
    .unwrap();
    assert_eq!(
        read(d.path(), "a.py"),
        "class A:\n  def f(self):\n    return 2\n    pass\n"
    );
    assert_eq!(a.indented, ["a.py block 1"]);
}

#[test]
fn search_ignoring_indentation_must_match_once() {
    // Two windows equal modulo indentation are ambiguous: an error, and nothing is written.
    let d = dir(&[("a.py", "def a():\n\treturn 1\ndef b():\n\treturn 1\n")]);
    let err = run(
        d.path(),
        "*** Update File: a.py\n<<<<<<< SEARCH\n    return 1\n=======\n    return 2\n>>>>>>> REPLACE\n",
    )
    .unwrap_err().shown();
    assert!(err.contains("SEARCH not found"), "{err}");
    assert_eq!(
        read(d.path(), "a.py"),
        "def a():\n\treturn 1\ndef b():\n\treturn 1\n"
    );
}

#[test]
fn search_ignoring_indentation_needs_the_same_nesting() {
    // SEARCH nests `z()` under `x = 1`; the file has it beside it, so nothing matches.
    let d = dir(&[("a.py", "def f():\n\tx = 1\n\tz()\n")]);
    let err = run(
        d.path(),
        "*** Update File: a.py\n<<<<<<< SEARCH\n    x = 1\n        z()\n=======\n    x = 2\n        z()\n>>>>>>> REPLACE\n",
    )
    .unwrap_err().shown();
    assert!(err.contains("SEARCH not found"), "{err}");
    assert_eq!(read(d.path(), "a.py"), "def f():\n\tx = 1\n\tz()\n");
}

#[test]
fn exact_search_wins_over_an_indentation_match() {
    let d = dir(&[("a.go", "\tx();\n    x();\n")]);
    let a = apply(
        &parse(
            "*** Update File: a.go\n<<<<<<< SEARCH\n    x();\n=======\n    y();\n>>>>>>> REPLACE\n",
        )
        .unwrap(),
        d.path(),
        false,
    )
    .unwrap();
    assert_eq!(read(d.path(), "a.go"), "\tx();\n    y();\n");
    assert!(a.indented.is_empty());
}

#[test]
fn search_matches_across_crlf_lines_and_keeps_crlf() {
    let d = dir(&[("w.txt", "a\r\nb\r\nc\r\n")]);
    let a = apply(
        &parse("*** Update File: w.txt\n<<<<<<< SEARCH\na\nb\n=======\nA\nB\n>>>>>>> REPLACE\n")
            .unwrap(),
        d.path(),
        false,
    )
    .unwrap();
    assert_eq!(read(d.path(), "w.txt"), "A\r\nB\r\nc\r\n");
    assert_eq!(a.crlf, ["w.txt block 1"]);
    // A one-line SEARCH matches the file as written, so there is nothing to note.
    let a = apply(
        &parse("*** Update File: w.txt\n<<<<<<< SEARCH\nc\n=======\nC\n>>>>>>> REPLACE\n").unwrap(),
        d.path(),
        false,
    )
    .unwrap();
    assert_eq!(read(d.path(), "w.txt"), "A\r\nB\r\nC\r\n");
    assert!(a.crlf.is_empty());
}

#[test]
fn every_failing_block_is_reported_and_nothing_is_written() {
    let d = dir(&[("a.txt", "alpha\nbeta\n"), ("b.txt", "gamma\n")]);
    let e = run(
        d.path(),
        "*** Update File: a.txt\n<<<<<<< SEARCH\nmissing one\n=======\nx\n>>>>>>> REPLACE\n<<<<<<< SEARCH\nmissing two\n=======\ny\n>>>>>>> REPLACE\n*** Update File: b.txt\n<<<<<<< SEARCH\nmissing three\n=======\nz\n>>>>>>> REPLACE\n",
    )
    .unwrap_err()
    .shown();
    assert!(e.starts_with("3 blocks failed; nothing was written"), "{e}");
    for line in [
        "a.txt: block 1: SEARCH not found, first line `missing one`",
        "a.txt: block 2: SEARCH not found, first line `missing two`",
        "b.txt: block 1: SEARCH not found, first line `missing three`",
        "note: a block after a failed one in the same file was checked against the file without it",
        "hint: fix each failing block",
    ] {
        assert!(e.contains(line), "{e}");
    }
    assert_eq!(read(d.path(), "a.txt"), "alpha\nbeta\n");
    assert_eq!(read(d.path(), "b.txt"), "gamma\n");
}

#[test]
fn a_block_after_a_failed_one_sees_the_file_without_it() {
    // Block 1 would create `x` but fails, so block 2, which searches `x`, fails too.
    let d = dir(&[("a.txt", "a\n")]);
    let e = run(
        d.path(),
        "*** Update File: a.txt\n<<<<<<< SEARCH\nmissing\n=======\nx\n>>>>>>> REPLACE\n<<<<<<< SEARCH\nx\n=======\ny\n>>>>>>> REPLACE\n",
    )
    .unwrap_err()
    .shown();
    assert!(e.starts_with("2 blocks failed; nothing was written"), "{e}");
    assert!(e.contains("a.txt: block 1: SEARCH not found"), "{e}");
    assert!(e.contains("a.txt: block 2: SEARCH not found"), "{e}");
    assert_eq!(read(d.path(), "a.txt"), "a\n");
}

#[test]
fn blocks_apply_in_order_so_later_ones_see_earlier_edits() {
    let d = dir(&[("a.txt", "a\n")]);
    run(
        d.path(),
        "*** Update File: a.txt\n<<<<<<< SEARCH\na\n=======\nb\n>>>>>>> REPLACE\n<<<<<<< SEARCH\nb\n=======\nc\n>>>>>>> REPLACE\n",
    )
    .unwrap();
    assert_eq!(read(d.path(), "a.txt"), "c\n");
}

#[test]
fn crlf_patch_text_matches_an_lf_file() {
    let d = dir(&[("w.txt", "a\nb\nc\n")]);
    let a = apply(
        &parse("*** Update File: w.txt\r\n<<<<<<< SEARCH\r\na\r\nb\r\n=======\r\nA\r\nB\r\n>>>>>>> REPLACE\r\n").unwrap(),
        d.path(),
        false,
    )
    .unwrap();
    assert_eq!(read(d.path(), "w.txt"), "A\nB\nc\n");
    assert!(a.crlf.is_empty());
}
