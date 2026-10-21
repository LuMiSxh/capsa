// SPDX-License-Identifier: MPL-2.0
#![expect(clippy::unwrap_used, reason = "test code")]

use acus_search::{FileHits, FindOpts, Row, attach_outlines, find, find_range, find_stdin};
use acus_walk::WalkOpts;
use std::fs;

fn project() -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    fs::write(
        d.path().join("a.rs"),
        "fn alpha() {\n    let needle = 1;\n}\n// needle at top\n",
    )
    .unwrap();
    fs::write(d.path().join("b.txt"), "no match\nNEEDLE upper\n").unwrap();
    fs::write(d.path().join("bin.dat"), b"needle\0binary").unwrap();
    d
}

fn opts(d: &tempfile::TempDir, pats: &[&str]) -> FindOpts {
    FindOpts {
        patterns: pats.iter().map(|s| s.to_string()).collect(),
        walk: WalkOpts {
            roots: vec![d.path().into()],
            ..Default::default()
        },
        ignore_case: false,
        fixed: false,
        syntax: true,
        word: false,
        line: false,
        invert: false,
        max_per_file: None,
        multiline: false,
    }
}

#[test]
fn find_range_keeps_file_absolute_lines_and_counts_only_its_range() {
    let d = tempfile::tempdir().unwrap();
    fs::write(d.path().join("f.txt"), "hit 1\nhit 2\nmiss\nhit 4\nhit 5\n").unwrap();
    let path = d.path().join("f.txt");
    let o = opts(&d, &["hit"]);
    let f = find_range(&o, &path, (2, 4)).unwrap().unwrap();
    assert_eq!(
        f.hits.iter().map(|h| h.line).collect::<Vec<_>>(),
        vec![2, 4]
    );
    assert_eq!(f.lines, Some((2, 4)));
    // -m applies to the hits inside the range, not to the first matches of the file.
    let mut capped = opts(&d, &["hit"]);
    capped.max_per_file = Some(1);
    let f = find_range(&capped, &path, (4, 5)).unwrap().unwrap();
    assert_eq!(f.hits.iter().map(|h| h.line).collect::<Vec<_>>(), vec![4]);
    assert!(find_range(&o, &path, (3, 3)).unwrap().is_none());
}

#[test]
fn find_range_context_stays_inside_the_range() {
    let d = tempfile::tempdir().unwrap();
    fs::write(d.path().join("f.txt"), "hit 1\nhit 2\nmiss\nhit 4\nhit 5\n").unwrap();
    let f = find_range(&opts(&d, &["hit"]), &d.path().join("f.txt"), (2, 3))
        .unwrap()
        .unwrap();
    let rows = f.context_rows(f.hits.len(), 1, 1);
    assert_eq!(rows.len(), 2, "{rows:?}");
    assert!(matches!(rows[0], Row::Hit(h) if h.line == 2));
    assert!(matches!(rows[1], Row::Context { line: 3, .. }));
}

#[test]
fn line_regexp_matches_whole_lines_only() {
    let d = project();
    let mut o = opts(&d, &["needle"]);
    o.line = true;
    assert!(find(&o).unwrap().is_empty());
    o.patterns = vec![r"\s*let needle = 1;".into()];
    let files = find(&o).unwrap();
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].hits[0].line, 2);
    o.fixed = true;
    o.patterns = vec!["let needle = 1;".into()];
    assert!(find(&o).unwrap().is_empty());
    o.patterns = vec!["    let needle = 1;".into()];
    assert_eq!(find(&o).unwrap()[0].hits[0].line, 2);
}

#[test]
fn multiline_anchors_match_at_line_boundaries() {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(
        d.path().join("a.txt"),
        "intro\nfn foo\nbar\nfn foo\nbaz\ntail end\n",
    )
    .unwrap();
    let mut o = opts(&d, &[r"^fn foo\nbar$"]);
    o.multiline = true;
    let files = find(&o).unwrap();
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].hits.len(), 1);
    assert_eq!(files[0].hits[0].line, 2);
    assert_eq!(files[0].hits[0].text, "fn foo\nbar");
    // `$` before the final newline, and `^` after the first line.
    let mut o = opts(&d, &[r"^tail end$"]);
    o.multiline = true;
    assert_eq!(find(&o).unwrap().len(), 1);
}

#[test]
fn multiline_hits_span_the_lines_of_the_match() {
    let d = project();
    let mut o = opts(&d, &[r"fn alpha\(\) \{\n    let needle"]);
    assert!(find(&o).unwrap().is_empty(), "lines never contain \\n");
    o.multiline = true;
    let files = find(&o).unwrap();
    assert_eq!(files.len(), 1);
    let h = &files[0].hits[0];
    assert_eq!(
        (h.line, h.text.as_str(), &h.spans[..]),
        (1, "fn alpha() {\n    let needle = 1;", &[(0, 27)][..])
    );
}

#[test]
fn attach_outlines_matches_syntax_true() {
    let d = project();
    fs::write(
        d.path().join("c.rs"),
        "struct S;\nimpl S {\n    fn needle(&self) {}\n}\n",
    )
    .unwrap();
    let want = find(&opts(&d, &["needle"])).unwrap();
    let mut o = opts(&d, &["needle"]);
    o.syntax = false;
    let mut got = find(&o).unwrap();
    assert!(got.iter().all(|f| f.outline.is_none()));
    attach_outlines(&mut got);
    assert_eq!(got.len(), want.len());
    assert!(got.len() >= 2);
    for (g, w) in got.iter().zip(&want) {
        assert!(w.outline.is_some());
        assert_eq!(format!("{:?}", g.outline), format!("{:?}", w.outline));
    }
}

#[test]
fn finds_with_enclosing_symbols_sorted() {
    let d = project();
    let r = find(&opts(&d, &["needle"])).unwrap();
    assert_eq!(r.len(), 1, "binary files are skipped, case matters");
    let f = &r[0];
    assert!(f.path.ends_with("a.rs"));
    assert_eq!(f.hits.iter().map(|h| h.line).collect::<Vec<_>>(), [2, 4]);
    assert_eq!(f.hits[0].text, "    let needle = 1;");
    let o = f.outline.as_ref().unwrap();
    assert_eq!(o.enclosing(2).unwrap().qual, "alpha");
    assert!(o.enclosing(4).is_none());
}

#[test]
fn ignore_case_and_multiple_patterns() {
    let d = project();
    let mut o = opts(&d, &["upper", "alpha"]);
    o.ignore_case = true;
    let r = find(&o).unwrap();
    assert_eq!(r.len(), 2);
    assert!(r[0].path.ends_with("a.rs") && r[1].path.ends_with("b.txt"));
}

#[test]
fn fixed_strings_escape_regex() {
    let d = project();
    fs::write(d.path().join("c.rs"), "a.b()\naxb()\n").unwrap();
    let mut o = opts(&d, &["a.b("]);
    o.fixed = true;
    let r = find(&o).unwrap();
    assert_eq!(r.len(), 1);
    assert_eq!(r[0].hits.len(), 1);
}

#[test]
fn invalid_regex_is_an_error() {
    let d = project();
    assert!(find(&opts(&d, &["("])).is_err());
}

/// Each row as `h<line>` (hit), `c<line>` (context) or `--`.
fn rows(f: &FileHits, n: usize, before: usize, after: usize) -> Vec<String> {
    f.context_rows(n, before, after)
        .iter()
        .map(|r| match r {
            Row::Hit(h) => format!("h{}", h.line),
            Row::Context { line, .. } => format!("c{line}"),
            Row::Break => "--".to_owned(),
        })
        .collect()
}

#[test]
fn context_rows_merge_and_split_groups_like_grep() {
    let d = project();
    let f = find_stdin(&opts(&d, &["hit"]), b"hit\na\nhit\nb\nc\nd\ne\nhit\n")
        .unwrap()
        .unwrap();
    assert_eq!(f.path, "<stdin>");
    // Without context, non-adjacent hits still split like grep's -A0.
    assert_eq!(rows(&f, 3, 0, 0), ["h1", "--", "h3", "--", "h8"]);
    // Only the first n hits count (the per-file budget).
    assert_eq!(rows(&f, 2, 0, 0), ["h1", "--", "h3"]);
    // Hit 1 and 3 share context; the group ends before hit 8's.
    assert_eq!(
        rows(&f, 3, 1, 1),
        ["h1", "c2", "h3", "c4", "--", "c7", "h8"]
    );
    // Ranges stop at the end of the file.
    assert_eq!(
        rows(&f, 3, 0, 2),
        ["h1", "c2", "h3", "c4", "c5", "--", "h8"]
    );
}
