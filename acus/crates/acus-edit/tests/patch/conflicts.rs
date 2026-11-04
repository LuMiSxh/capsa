// SPDX-License-Identifier: MPL-2.0
use super::*;

const TWO_SECTIONS: &str = concat!(
    "before\n",
    "<<<<<<< HEAD\n",
    "ours one\n",
    "||||||| base\n",
    "base one\n",
    "=======\n",
    "theirs one\n",
    ">>>>>>> branch\n",
    "middle\n",
    "<<<<<<< HEAD\n",
    "ours two\n",
    "||||||| base\n",
    "base two\n",
    "=======\n",
    "theirs two\n",
    ">>>>>>> branch\n",
    "end\n",
);

const SECTION_THEN_LINES: &str = concat!(
    "top\n",
    "<<<<<<< HEAD\n",
    "ours\n",
    "||||||| base\n",
    "base\n",
    "=======\n",
    "theirs\n",
    ">>>>>>> branch\n",
    "middle\n",
    "cut one\n",
    "cut two\n",
    "bottom\n",
);

fn resolved_with(mode: &str) -> String {
    let d = dir(&[("a.txt", TWO_SECTIONS)]);
    run(d.path(), &format!("*** Resolve Conflict: a.txt {mode}\n")).unwrap();
    read(d.path(), "a.txt")
}

#[test]
fn ours_theirs_both_and_base_resolve_every_section() {
    assert_eq!(
        resolved_with("--ours"),
        "before\nours one\nmiddle\nours two\nend\n"
    );
    assert_eq!(
        resolved_with("--theirs"),
        "before\ntheirs one\nmiddle\ntheirs two\nend\n"
    );
    assert_eq!(
        resolved_with("--both"),
        "before\nours one\ntheirs one\nmiddle\nours two\ntheirs two\nend\n"
    );
    assert_eq!(
        resolved_with("--base"),
        "before\nbase one\nmiddle\nbase two\nend\n"
    );
}

#[test]
fn line_targets_resolve_only_their_section_in_one_patch() {
    let d = dir(&[("a.txt", TWO_SECTIONS)]);
    run(
        d.path(),
        "*** Resolve Conflict: a.txt:3 --theirs\n*** Resolve Conflict: a.txt:14 --ours\n",
    )
    .unwrap();
    assert_eq!(
        read(d.path(), "a.txt"),
        "before\ntheirs one\nmiddle\nours two\nend\n"
    );
}

#[test]
fn delete_lines_below_a_resolved_section_counts_from_the_file_before_the_patch() {
    let d = dir(&[("a.txt", SECTION_THEN_LINES)]);
    run(
        d.path(),
        "*** Resolve Conflict: a.txt --ours\n\
         *** Delete Lines: a.txt:10-11\n*** Expect: cut one\n",
    )
    .unwrap();
    assert_eq!(read(d.path(), "a.txt"), "top\nours\nmiddle\nbottom\n");
}

#[test]
fn delete_lines_above_a_resolved_section_counts_from_the_file_before_the_patch() {
    let d = dir(&[("a.txt", SECTION_THEN_LINES)]);
    run(
        d.path(),
        "*** Resolve Conflict: a.txt --ours\n\
         *** Delete Lines: a.txt:1-1\n*** Expect: top\n",
    )
    .unwrap();
    assert_eq!(
        read(d.path(), "a.txt"),
        "ours\nmiddle\ncut one\ncut two\nbottom\n"
    );
}

#[test]
fn a_line_directive_over_a_resolved_section_is_refused() {
    let d = dir(&[("a.txt", SECTION_THEN_LINES)]);
    let e = run(
        d.path(),
        "*** Resolve Conflict: a.txt --ours\n\
         *** Delete Lines: a.txt:3-3\n*** Expect: ours\n",
    )
    .unwrap_err()
    .shown();
    assert!(e.contains("`*** Resolve Conflict:`"), "{e}");
    assert!(e.contains("`*** Delete Lines:`"), "{e}");
    assert!(e.contains("lines 3-3"), "{e}");
    assert_eq!(read(d.path(), "a.txt"), SECTION_THEN_LINES);
}

#[test]
fn move_lines_after_a_resolved_section_counts_from_the_file_before_the_patch() {
    let d = dir(&[("a.txt", SECTION_THEN_LINES)]);
    run(
        d.path(),
        "*** Resolve Conflict: a.txt --ours\n\
         *** Move Lines: a.txt:10-11\n*** Expect: cut one\n*** After: a.txt:0\n",
    )
    .unwrap();
    assert_eq!(
        read(d.path(), "a.txt"),
        "cut one\ncut two\ntop\nours\nmiddle\nbottom\n"
    );
}

#[test]
fn update_file_after_a_resolve_sees_the_resolved_text() {
    let d = dir(&[("a.txt", SECTION_THEN_LINES)]);
    run(
        d.path(),
        "*** Resolve Conflict: a.txt --ours\n\
         *** Update File: a.txt\n\
         <<<<<<< SEARCH\nours\nmiddle\n=======\nours\nmiddle changed\n>>>>>>> REPLACE\n",
    )
    .unwrap();
    assert_eq!(
        read(d.path(), "a.txt"),
        "top\nours\nmiddle changed\ncut one\ncut two\nbottom\n"
    );
}

#[test]
fn a_file_without_markers_fails_and_writes_nothing() {
    let d = dir(&[("a.txt", "keep\n")]);
    let e = run(d.path(), "*** Resolve Conflict: a.txt --ours\n")
        .unwrap_err()
        .shown();
    assert!(e.contains("a.txt: no conflict markers"), "{e}");
    assert_eq!(read(d.path(), "a.txt"), "keep\n");
}

#[test]
fn search_replace_examples_are_not_conflict_markers() {
    let before = "<<<<<<< SEARCH\nold\n=======\nnew\n>>>>>>> REPLACE\n";
    let d = dir(&[("a.md", before)]);
    let applied = apply(
        &parse("*** Update File: a.md\n<<<<<<< SEARCH\nold\n=======\nnew\n>>>>>>> REPLACE\n")
            .unwrap(),
        d.path(),
        false,
    )
    .unwrap();
    assert!(applied.unchecked.is_empty(), "{:?}", applied.unchecked);
}

#[test]
fn rust_with_a_quoted_patch_example_is_still_syntax_checked() {
    let before = concat!(
        "const DOC: &str = r#\"\n",
        "<<<<<<< SEARCH\nold\n=======\nnew\n>>>>>>> REPLACE\n",
        "\"#;\nfn f() {}\n",
    );
    let d = dir(&[("a.rs", before)]);
    let e = run(
        d.path(),
        "*** Update File: a.rs\n<<<<<<< SEARCH\nfn f() {}\n=======\nfn f() {\n>>>>>>> REPLACE\n",
    )
    .unwrap_err()
    .shown();
    assert!(e.contains("patch would break the syntax of a.rs"), "{e}");
    assert_eq!(read(d.path(), "a.rs"), before);
}

#[test]
fn a_real_conflict_still_skips_the_syntax_check() {
    let before = "fn f() {\n<<<<<<< HEAD\n    1\n=======\n    2\n>>>>>>> branch\n}\nfn g() {\n<<<<<<< HEAD\n    3\n=======\n    4\n>>>>>>> branch\n}\n";
    let d = dir(&[("a.rs", before)]);
    let applied = apply(
        &parse("*** Resolve Conflict: a.rs:2 --ours\n").unwrap(),
        d.path(),
        false,
    )
    .unwrap();
    assert_eq!(applied.unchecked, ["a.rs"]);
    assert_eq!(
        read(d.path(), "a.rs"),
        "fn f() {\n    1\n}\nfn g() {\n<<<<<<< HEAD\n    3\n=======\n    4\n>>>>>>> branch\n}\n"
    );
}

#[test]
fn resolve_conflict_refuses_a_file_with_only_search_replace_blocks() {
    let before = "<<<<<<< SEARCH\nold\n=======\nnew\n>>>>>>> REPLACE\n";
    let d = dir(&[("a.md", before)]);
    let e = run(d.path(), "*** Resolve Conflict: a.md --ours\n")
        .unwrap_err()
        .shown();
    assert!(e.contains("a.md: no conflict markers"), "{e}");
    assert_eq!(read(d.path(), "a.md"), before);
}

#[test]
fn an_unterminated_section_fails_and_writes_nothing() {
    let before = "a\n<<<<<<< HEAD\nours\n=======\ntheirs\n";
    let d = dir(&[("a.txt", before)]);
    let e = run(d.path(), "*** Resolve Conflict: a.txt --ours\n")
        .unwrap_err()
        .shown();
    assert!(e.contains("a.txt:2: unterminated conflict section"), "{e}");
    assert_eq!(read(d.path(), "a.txt"), before);
}

#[test]
fn a_line_outside_every_section_fails() {
    let d = dir(&[("a.txt", TWO_SECTIONS)]);
    let e = run(d.path(), "*** Resolve Conflict: a.txt:9 --ours\n")
        .unwrap_err()
        .shown();
    assert!(
        e.contains("a.txt:9: no conflict section contains that line"),
        "{e}"
    );
    assert_eq!(read(d.path(), "a.txt"), TWO_SECTIONS);
}

#[test]
fn base_needs_the_diff3_section() {
    let before = "a\n<<<<<<< HEAD\nours\n=======\ntheirs\n>>>>>>> branch\n";
    let d = dir(&[("a.txt", before)]);
    let e = run(d.path(), "*** Resolve Conflict: a.txt --base\n")
        .unwrap_err()
        .shown();
    assert!(
        e.contains("a.txt:2: conflict section has no `|||||||` base"),
        "{e}"
    );
    assert_eq!(read(d.path(), "a.txt"), before);
}

#[test]
fn a_resolved_file_must_still_parse() {
    let before = "fn f() -> u8 {\n<<<<<<< HEAD\n    1\n=======\n    2\n>>>>>>> branch\n";
    let d = dir(&[("a.rs", before)]);
    let e = run(d.path(), "*** Resolve Conflict: a.rs --ours\n")
        .unwrap_err()
        .shown();
    assert!(e.contains("patch would break the syntax of a.rs"), "{e}");
    assert_eq!(read(d.path(), "a.rs"), before);
}

#[test]
fn a_missing_mode_names_the_expected_forms() {
    let m = parse("*** Resolve Conflict: a.txt\n").unwrap_err().shown();
    assert!(
        m.contains("expected `*** Resolve Conflict: PATH --ours`"),
        "{m}"
    );
}

#[test]
fn resolves_conflicts_in_a_gated_language() {
    let d = dir(&[(
        "a.rs",
        "fn f() -> u8 {\n<<<<<<< HEAD\n    1\n=======\n    2\n>>>>>>> branch\n}\n",
    )]);
    run(d.path(), "*** Resolve Conflict: a.rs --ours\n").unwrap();
    assert_eq!(read(d.path(), "a.rs"), "fn f() -> u8 {\n    1\n}\n");
}
