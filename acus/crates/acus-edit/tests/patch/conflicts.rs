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
fn a_file_without_markers_fails_and_writes_nothing() {
    let d = dir(&[("a.txt", "keep\n")]);
    let e = run(d.path(), "*** Resolve Conflict: a.txt --ours\n")
        .unwrap_err()
        .shown();
    assert!(e.contains("a.txt: no conflict markers"), "{e}");
    assert_eq!(read(d.path(), "a.txt"), "keep\n");
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
