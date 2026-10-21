// SPDX-License-Identifier: MPL-2.0
use super::*;

#[test]
fn replace_all_literal_and_regex_across_files() {
    let d = dir(&[
        ("src/a.rs", "use old::X;\nfn f() { old::y(); }\n"),
        ("src/b.rs", "let v = old::z;\n"),
        ("c.txt", "old::keep\n"),
    ]);
    let c = run(
        d.path(),
        "*** Replace All: src\n<<<<<<< SEARCH\nold::\n=======\nnew::\n>>>>>>> REPLACE\n<<<<<<< REGEX\nfn (\\w+)\\(\\)\n=======\nfn ${1}_v2()\n>>>>>>> REPLACE\n",
    )
    .unwrap();
    assert_eq!(
        read(d.path(), "src/a.rs"),
        "use new::X;\nfn f_v2() { new::y(); }\n"
    );
    assert_eq!(read(d.path(), "src/b.rs"), "let v = new::z;\n");
    assert_eq!(read(d.path(), "c.txt"), "old::keep\n");
    let s: Vec<_> = c.iter().map(ToString::to_string).collect();
    assert_eq!(s, ["M src/a.rs +2 -2", "M src/b.rs +1 -1"]);

    let e = run(
        d.path(),
        "*** Replace All: src/a.rs\n<<<<<<< SEARCH\nmissing\n=======\nx\n>>>>>>> REPLACE\n",
    )
    .unwrap_err()
    .shown();
    assert!(e.contains("`missing` matches nothing in src/a.rs"), "{e}");
}

#[test]
fn replace_all_multiline_and_globs() {
    let d = dir(&[
        ("a.py", "x = 1\nif a:\n    go()\n\nif a:\n    go()\n"),
        ("b.rs", "if a:\n    go()\n"),
    ]);
    run(
        d.path(),
        "*** Replace All: *.py\n<<<<<<< SEARCH\nif a:\n    go()\n=======\ngo_if(a)\n>>>>>>> REPLACE\n",
    )
    .unwrap();
    assert_eq!(read(d.path(), "a.py"), "x = 1\ngo_if(a)\n\ngo_if(a)\n");
    assert_eq!(read(d.path(), "b.rs"), "if a:\n    go()\n");
}

#[test]
fn replace_all_takes_dirs_whole_and_globs_anywhere() {
    let d = dir(&[
        ("web/a.ts", "old\n"),
        ("src/b.rs", "old\n"),
        ("docs/c.md", "old\n"),
        ("docs/d.txt", "old\n"),
    ]);
    run(
        d.path(),
        "*** Replace All: web *.md\n<<<<<<< SEARCH\nold\n=======\nnew\n>>>>>>> REPLACE\n",
    )
    .unwrap();
    let got = ["web/a.ts", "src/b.rs", "docs/c.md", "docs/d.txt"].map(|p| read(d.path(), p));
    assert_eq!(got, ["new\n", "old\n", "new\n", "old\n"]);
}
