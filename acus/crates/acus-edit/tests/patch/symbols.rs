// SPDX-License-Identifier: MPL-2.0
use super::*;

#[test]
fn insert_before_and_after_symbols_and_lines() {
    let d = dir(&[(
        "a.rs",
        "fn one() {}\n\n/// Two.\n#[inline]\nfn two() {}\n\nfn three() {}\n",
    )]);
    let c = run(
        d.path(),
        "*** Insert Before: a.rs#two\n+/// New.\n+fn new() {}\n+\n*** Insert After: a.rs#two\n+\n+fn after() {}\n",
    )
    .unwrap();
    assert_eq!(
        read(d.path(), "a.rs"),
        "fn one() {}\n\n/// New.\nfn new() {}\n\n/// Two.\n#[inline]\nfn two() {}\n\nfn after() {}\n\nfn three() {}\n"
    );
    let s: Vec<_> = c.iter().map(ToString::to_string).collect();
    assert_eq!(s, ["M a.rs +5 -0"]);
    run(
        d.path(),
        "*** Insert Before: a.rs:1\n+// top\n*** Insert After: a.rs:13\n+// bottom\n",
    )
    .unwrap();
    let got = read(d.path(), "a.rs");
    assert!(got.starts_with("// top\nfn one() {}\n"), "{got}");
    assert!(got.ends_with("fn three() {}\n// bottom\n"), "{got}");
    let e = run(d.path(), "*** Insert After: a.rs:99\n+x\n")
        .unwrap_err()
        .shown();
    assert!(e.contains("a.rs:99"), "{e}");
    assert!(parse("*** Insert After: a.rs:0\n+x\n").is_err());
    assert!(parse("*** Insert After: a.rs\n+x\n").is_err());
    assert!(
        parse("*** Insert Before: a.rs#two\n")
            .unwrap_err()
            .shown()
            .contains("needs `+` lines")
    );
}

#[test]
fn replace_symbol() {
    let d = dir(&[(
        "a.rs",
        "struct A;\n\nimpl A {\n    fn f() -> u8 {\n        1\n    }\n}\n",
    )]);
    let c = run(
        d.path(),
        "*** Replace Symbol: a.rs#A::f\n+    fn f() -> u8 {\n+        2\n+    }\n",
    )
    .unwrap();
    assert_eq!(
        read(d.path(), "a.rs"),
        "struct A;\n\nimpl A {\n    fn f() -> u8 {\n        2\n    }\n}\n"
    );
    assert_eq!(c[0].to_string(), "M a.rs +1 -1");
}

#[test]
fn symbols_stay_in_their_language() {
    let d = dir(&[("a.rs", "fn one() {}\n"), ("b.py", "x = 1\n")]);
    let e = run(d.path(), "*** Move Symbol: a.rs#one\n*** To: b.py\n").unwrap_err();
    assert!(e.to_string().contains("cannot move rust code"), "{e:#}");
    let e = run(d.path(), "*** Replace Symbol: a.rs#one\n").unwrap_err();
    assert!(e.shown().contains("Delete Symbol"), "{e:#}");
}

#[test]
fn moving_a_symbol_next_to_itself_is_refused() {
    let d = dir(&[("a.rs", "fn one() {}\n\nfn two() {}\n")]);
    let e = run(d.path(), "*** Move Symbol: a.rs#one\n*** After: a.rs#one\n").unwrap_err();
    assert!(e.to_string().contains("next to itself"), "{e:#}");
}

#[test]
fn delete_symbol_takes_docs_and_one_blank() {
    let d = dir(&[(
        "a.rs",
        "fn one() {}\n\n/// Two.\n#[inline]\nfn two() {\n    x();\n}\n\nfn three() {}\n",
    )]);
    let c = run(d.path(), "*** Delete Symbol: a.rs#two\n").unwrap();
    assert_eq!(read(d.path(), "a.rs"), "fn one() {}\n\nfn three() {}\n");
    assert_eq!(c[0].to_string(), "M a.rs +0 -6");
}

#[test]
fn move_symbol_within_and_across_files() {
    let d = dir(&[
        (
            "a.rs",
            "fn one() {}\n\n/// Two.\nfn two() {}\n\nfn three() {}\n",
        ),
        ("b.rs", "fn b() {}\n"),
    ]);
    run(
        d.path(),
        "*** Move Symbol: a.rs#three\n*** Before: a.rs#one\n",
    )
    .unwrap();
    assert_eq!(
        read(d.path(), "a.rs"),
        "fn three() {}\n\nfn one() {}\n\n/// Two.\nfn two() {}\n"
    );
    let c = run(
        d.path(),
        "*** Move Symbol: a.rs#two\n*** To: b.rs\n*** Move Symbol: a.rs#one\n*** To: c.rs\n",
    )
    .unwrap();
    assert_eq!(read(d.path(), "a.rs"), "fn three() {}\n");
    assert_eq!(
        read(d.path(), "b.rs"),
        "fn b() {}\n\n/// Two.\nfn two() {}\n"
    );
    assert_eq!(read(d.path(), "c.rs"), "fn one() {}\n");
    let s: Vec<_> = c.iter().map(ToString::to_string).collect();
    assert_eq!(s, ["M a.rs +0 -5", "M b.rs +3 -0", "A c.rs +1"]);
    run(d.path(), "*** Move Symbol: b.rs#b\n*** After: b.rs#two\n").unwrap();
    assert_eq!(
        read(d.path(), "b.rs"),
        "/// Two.\nfn two() {}\n\nfn b() {}\n"
    );
    assert!(
        parse("*** Move Symbol: a.rs#x\n")
            .unwrap_err()
            .shown()
            .contains("needs a destination")
    );
}

#[test]
fn replacement_may_contain_example_blocks() {
    let d = dir(&[("a.md", "# Docs\nTODO\n")]);
    run(
        d.path(),
        "*** Update File: a.md\n<<<<<<< SEARCH\nTODO\n=======\n<<<<<<< SEARCH\nold\n=======\nnew\n>>>>>>> REPLACE\n>>>>>>> REPLACE\n",
    )
    .unwrap();
    assert_eq!(
        read(d.path(), "a.md"),
        "# Docs\n<<<<<<< SEARCH\nold\n=======\nnew\n>>>>>>> REPLACE\n"
    );
}

#[test]
fn moved_symbols_take_the_indent_of_their_new_place() {
    let d = dir(&[(
        "a.py",
        "class A:\n    def m(self):\n        return 1\n\n    def n(self):\n        pass\n\n\ndef top():\n    pass\n",
    )]);
    run(d.path(), "*** Move Symbol: a.py#A.m\n*** After: a.py#top\n").unwrap();
    assert_eq!(
        read(d.path(), "a.py"),
        "class A:\n    def n(self):\n        pass\n\n\ndef top():\n    pass\n\ndef m(self):\n    return 1\n"
    );
    run(d.path(), "*** Move Symbol: a.py#m\n*** Before: a.py#A.n\n").unwrap();
    assert_eq!(
        read(d.path(), "a.py"),
        "class A:\n    def m(self):\n        return 1\n\n    def n(self):\n        pass\n\n\ndef top():\n    pass\n"
    );
}

#[test]
fn a_type_and_its_impl_are_refused_by_move_and_delete() {
    let src = "enum Mode {\n    A,\n}\n\nimpl Mode {\n    fn f() {}\n}\n\nfn main() {}\n";
    let d = dir(&[("mode.rs", src), ("b.rs", "fn b() {}\n")]);
    let msg = "mode.rs#Mode is ambiguous: enum Mode (1-3), impl Mode (5-7)";
    let e = run(d.path(), "*** Move Symbol: mode.rs#Mode\n*** To: b.rs\n").unwrap_err();
    assert!(e.to_string().contains(msg), "{e:#}");
    let e = run(d.path(), "*** Delete Symbol: mode.rs#Mode\n").unwrap_err();
    assert!(e.to_string().contains(msg), "{e:#}");
    let e = run(
        d.path(),
        "*** Move Symbol: mode.rs#main\n*** After: mode.rs#Mode\n",
    )
    .unwrap_err();
    assert!(e.to_string().contains(msg), "{e:#}");
    assert_eq!(read(d.path(), "mode.rs"), src);
    assert_eq!(read(d.path(), "b.rs"), "fn b() {}\n");
}

#[test]
fn methods_and_unique_names_still_move_and_delete() {
    let d = dir(&[
        (
            "mode.rs",
            "enum Mode {\n    A,\n}\n\nimpl Mode {\n    fn f() {}\n}\n\nfn main() {}\n",
        ),
        ("b.rs", "fn b() {}\n"),
    ]);
    run(d.path(), "*** Delete Symbol: mode.rs#Mode::f\n").unwrap();
    assert!(!read(d.path(), "mode.rs").contains("fn f"));
    run(d.path(), "*** Move Symbol: mode.rs#main\n*** To: b.rs\n").unwrap();
    assert_eq!(read(d.path(), "b.rs"), "fn b() {}\n\nfn main() {}\n");
    assert!(!read(d.path(), "mode.rs").contains("fn main"));
}

#[test]
fn replace_symbol_refuses_a_type_and_its_impl() {
    let src = "enum Mode {\n    A,\n}\n\nimpl Mode {\n    fn f() {}\n}\n";
    let d = dir(&[("mode.rs", src)]);
    let e = run(
        d.path(),
        "*** Replace Symbol: mode.rs#Mode\n+struct Mode;\n",
    )
    .unwrap_err();
    let msg = "mode.rs#Mode is ambiguous: enum Mode (1-3), impl Mode (5-7)";
    assert!(e.to_string().contains(msg), "{e:#}");
    assert_eq!(read(d.path(), "mode.rs"), src);
}

#[test]
fn insert_refuses_a_type_and_its_impl() {
    let src = "enum Mode {\n    A,\n}\n\nimpl Mode {\n    fn f() {}\n}\n";
    let d = dir(&[("mode.rs", src)]);
    let e = run(d.path(), "*** Insert After: mode.rs#Mode\n+fn g() {}\n").unwrap_err();
    let msg = "mode.rs#Mode is ambiguous: enum Mode (1-3), impl Mode (5-7)";
    assert!(e.to_string().contains(msg), "{e:#}");
    assert_eq!(read(d.path(), "mode.rs"), src);
}

#[test]
fn syntax_check_refuses_a_patch_that_breaks_the_file() {
    let d = dir(&[("a.rs", "fn a() {\n    x();\n}\n"), ("b.md", "# T\n")]);
    let p = "*** Begin Patch\n*** Update File: a.rs\n<<<<<<< SEARCH\n    x();\n=======\n    x(;\n>>>>>>> REPLACE\n*** Update File: b.md\n<<<<<<< SEARCH\n# T\n=======\n# U\n>>>>>>> REPLACE\n*** End Patch\n";
    let e = apply(&parse(p).unwrap(), d.path(), &ApplyOptions::default())
        .unwrap_err()
        .shown();
    assert!(
        e.starts_with("patch would break the syntax of a.rs near line 2"),
        "{e}"
    );
    assert!(
        e.contains("hint: fix the block, or pass --allow-broken to write anyway"),
        "{e}"
    );
    assert_eq!(read(d.path(), "a.rs"), "fn a() {\n    x();\n}\n");
    assert_eq!(read(d.path(), "b.md"), "# T\n", "all or nothing");
    let broken = ApplyOptions {
        allow_broken: true,
        ..ApplyOptions::default()
    };
    let a = apply(&parse(p).unwrap(), d.path(), &broken).unwrap();
    assert_eq!(a.broken, ["a.rs"]);
    assert_eq!(read(d.path(), "a.rs"), "fn a() {\n    x(;\n}\n");
    assert_eq!(read(d.path(), "b.md"), "# U\n");
}

#[test]
fn syntax_check_counts_only_new_errors() {
    // `let x` at the top level is an error before and after the edit: not made worse, so it passes.
    let d = dir(&[("a.rs", "let x = 1;\nfn a() {}\n")]);
    let block = |s: &str, r: &str| {
        format!("*** Update File: a.rs\n<<<<<<< SEARCH\n{s}\n=======\n{r}\n>>>>>>> REPLACE\n")
    };
    apply(
        &parse(&block("let x = 1;", "let x = 2;")).unwrap(),
        d.path(),
        &ApplyOptions::default(),
    )
    .unwrap();
    assert_eq!(read(d.path(), "a.rs"), "let x = 2;\nfn a() {}\n");
    let e = run(d.path(), &block("fn a() {}", "fn a() { x(; }"))
        .unwrap_err()
        .shown();
    assert!(e.starts_with("patch would break the syntax of a.rs"), "{e}");
    assert_eq!(read(d.path(), "a.rs"), "let x = 2;\nfn a() {}\n");
}

#[test]
fn syntax_check_skips_unsupported_languages() {
    let d = dir(&[("a.txt", "a\n")]);
    run(
        d.path(),
        "*** Update File: a.txt\n<<<<<<< SEARCH\na\n=======\nfn ( {\n>>>>>>> REPLACE\n",
    )
    .unwrap();
    assert_eq!(read(d.path(), "a.txt"), "fn ( {\n");
}
