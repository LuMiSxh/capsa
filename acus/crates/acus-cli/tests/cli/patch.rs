// SPDX-License-Identifier: MPL-2.0
use crate::common::*;

#[test]
fn patch_help_documents_replace_all() {
    let (code, out, _) = acus(&["patch", "--help"]);
    assert_eq!(code, 0);
    let _dropguard = snapshot_settings().bind_to_scope();
    insta::assert_snapshot!(
        "cli__patch_help_documents_replace_all",
        &out[out.find("PATCH FORMAT").unwrap()..]
    );
}

#[test]
fn patch_check_counts_only_the_changed_lines() {
    let d = tempfile::tempdir().unwrap();
    let original = "fn f() {\n    let x = 1;\n    let y = 2;\n    let z = 3;\n}\n";
    std::fs::write(d.path().join("a.rs"), original).unwrap();
    let p = "*** Begin Patch\n*** Update File: a.rs\n<<<<<<< SEARCH\n    let x = 1;\n    let y = 2;\n    let z = 3;\n=======\n    let x = 1;\n    let y = 2;\n    let z = 3;\n    let w = 4;\n    let v = 5;\n>>>>>>> REPLACE\n*** End Patch\n";
    let (code, out, _) = acus_in(d.path(), &["patch", "--check"], p);
    assert_eq!(code, 0);
    assert!(out.starts_with("would update a.rs +2 -0\n"), "{out}");
    assert_eq!(
        std::fs::read_to_string(d.path().join("a.rs")).unwrap(),
        original
    );
}

#[test]
fn patch_add_file_prints_only_the_summary() {
    let d = tempfile::tempdir().unwrap();
    let body: String = (1..=30).map(|i| format!("+fn g{i}() {{}}\n")).collect();
    let p = format!("*** Begin Patch\n*** Add File: sub/big.rs\n{body}*** End Patch\n");
    assert_eq!(
        acus_in(d.path(), &["patch"], &p),
        (0, "A sub/big.rs +30\n".into(), String::new())
    );
}

#[test]
fn patch_replace_symbol_echoes_the_edges_of_a_long_body() {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join("f.rs"), "fn a() {\n    1\n}\n").unwrap();
    let body: String = (1..=45)
        .map(|i| format!("+    let y{i} = {i};\n"))
        .collect();
    let p = format!(
        "*** Begin Patch\n*** Replace Symbol: f.rs#a\n+fn a() {{\n{body}+}}\n*** End Patch\n"
    );
    let (code, out, err) = acus_in(d.path(), &["patch"], &p);
    assert_eq!(code, 0, "{err}");
    assert_eq!(
        out,
        "M f.rs +45 -1\n== f.rs 1-47\n\
         1\tfn a() {\n2\t    let y1 = 1;\n3\t    let y2 = 2;\n4\t    let y3 = 3;\n5\t    let y4 = 4;\n\
         … 37 more lines (acus show f.rs:6-42)\n\
         43\t    let y42 = 42;\n44\t    let y43 = 43;\n45\t    let y44 = 44;\n46\t    let y45 = 45;\n47\t}\n"
    );
}

#[test]
fn patch_from_stdin() {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join("a.rs"), "fn f() {\n    1\n}\n").unwrap();
    let p = "*** Begin Patch\n*** Update File: a.rs\n<<<<<<< SEARCH\n    1\n=======\n    2\n>>>>>>> REPLACE\n*** End Patch\n";
    assert_eq!(
        acus_in(d.path(), &["patch", "--check"], p),
        (
            0,
            "would update a.rs +1 -1\n== a.rs 2-2\n2\t    2\n(check only, nothing written)\n"
                .into(),
            String::new()
        )
    );
    assert_eq!(acus_in(d.path(), &["patch", "-q"], p).1, "M a.rs +1 -1\n");
    assert_eq!(
        std::fs::read_to_string(d.path().join("a.rs")).unwrap(),
        "fn f() {\n    2\n}\n"
    );
    let (code, _, err) = acus_in(d.path(), &["patch"], p);
    assert_eq!(code, 2);
    assert!(
        err.starts_with("error: a.rs: block 1: SEARCH not found"),
        "{err}"
    );
}

#[test]
fn patch_refuses_to_break_syntax_unless_allowed() {
    let d = tempfile::tempdir().unwrap();
    let file = d.path().join("a.rs");
    std::fs::write(&file, "fn f() {\n    1\n}\n").unwrap();
    let p = "*** Begin Patch\n*** Update File: a.rs\n<<<<<<< SEARCH\n    1\n=======\n    1(\n>>>>>>> REPLACE\n*** End Patch\n";
    let (code, _, err) = acus_in(d.path(), &["patch"], p);
    assert_eq!(code, 2);
    assert!(
        err.starts_with("error: patch would break the syntax of a.rs near line 2"),
        "{err}"
    );
    assert!(
        err.contains("hint: fix the block, or pass --allow-broken"),
        "{err}"
    );
    assert_eq!(acus_in(d.path(), &["patch", "--check"], p).0, 2);
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        "fn f() {\n    1\n}\n"
    );
    let (code, out, _) = acus_in(d.path(), &["patch", "--allow-broken", "-q"], p);
    assert_eq!(code, 0);
    assert!(
        out.contains("note: --allow-broken: wrote a.rs with more syntax errors than before"),
        "{out}"
    );
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        "fn f() {\n    1(\n}\n"
    );
}

#[test]
fn patch_accepts_a_c_attribute_declaration_added_beside_one() {
    let d = tempfile::tempdir().unwrap();
    let file = d.path().join("main.c");
    let before = "#ifdef __APPLE__\n__asm__(\".zerofill WINE_RESERVE,WINE_RESERVE\");\nstatic char __wine_reserve[0x1fffff000] __attribute__((section(\"WINE_RESERVE, WINE_RESERVE\")));\n#endif\n\nstatic const int preload_info[] = {\n    0\n};\n";
    std::fs::write(&file, before).unwrap();
    let p = "*** Begin Patch\n*** Update File: main.c\n<<<<<<< SEARCH\nstatic const int preload_info[] = {\n=======\n__asm__(\".zerofill WINE_TOP_DOWN,WINE_TOP_DOWN\");\nstatic char __wine_top_down[0x001ff0000] __attribute__((section(\"WINE_TOP_DOWN, WINE_TOP_DOWN\")));\n\nstatic const int preload_info[] = {\n>>>>>>> REPLACE\n*** End Patch\n";
    let (code, _, err) = acus_in(d.path(), &["patch", "-q"], p);
    assert_eq!(code, 0, "{err}");
    assert!(
        std::fs::read_to_string(&file)
            .unwrap()
            .contains("__wine_top_down[0x001ff0000] __attribute__"),
    );
}

#[test]
fn patch_accepts_an_entry_added_to_a_zed_style_jsonc_file() {
    let d = tempfile::tempdir().unwrap();
    let file = d.path().join("settings.json");
    let before =
        "// Zed settings\n{\n  \"theme\": \"One Dark\", // inline\n  \"tab_size\": 4,\n}\n";
    std::fs::write(&file, before).unwrap();
    let p = "*** Begin Patch\n*** Update File: settings.json\n<<<<<<< SEARCH\n  \"tab_size\": 4,\n=======\n  \"tab_size\": 4,\n  \"format_on_save\": \"on\",\n>>>>>>> REPLACE\n*** End Patch\n";
    let (code, _, err) = acus_in(d.path(), &["patch", "-q"], p);
    assert_eq!(code, 0, "{err}");
    assert!(
        std::fs::read_to_string(&file)
            .unwrap()
            .contains("\"format_on_save\": \"on\",\n}")
    );
}

#[test]
fn patch_refuses_a_broken_brace_in_a_jsonc_file() {
    let d = tempfile::tempdir().unwrap();
    let file = d.path().join("settings.json");
    let before = "// Zed settings\n{\n  \"tab_size\": 4,\n}\n";
    std::fs::write(&file, before).unwrap();
    let p = "*** Begin Patch\n*** Update File: settings.json\n<<<<<<< SEARCH\n  \"tab_size\": 4,\n}\n=======\n  \"tab_size\": 4,\n>>>>>>> REPLACE\n*** End Patch\n";
    let (code, _, err) = acus_in(d.path(), &["patch"], p);
    assert_eq!(code, 2);
    assert!(
        err.starts_with("error: patch would break the syntax of settings.json near line"),
        "{err}"
    );
    assert_eq!(std::fs::read_to_string(&file).unwrap(), before);
}

#[test]
fn patch_still_refuses_a_trailing_comma_added_to_strict_json() {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join("a.json"), "{\n  \"a\": 1\n}\n").unwrap();
    let p = "*** Begin Patch\n*** Update File: a.json\n<<<<<<< SEARCH\n  \"a\": 1\n=======\n  \"a\": 1,\n>>>>>>> REPLACE\n*** End Patch\n";
    let (code, _, err) = acus_in(d.path(), &["patch"], p);
    assert_eq!(code, 2, "{err}");
    assert!(
        err.starts_with("error: patch would break the syntax of a.json near line 2"),
        "{err}"
    );
}

#[test]
fn patch_still_refuses_swift_import_fused_to_an_identifier() {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join("a.swift"), "import Foo\n\nfunc f() {}\n").unwrap();
    let p = "*** Begin Patch\n*** Update File: a.swift\n<<<<<<< SEARCH\nimport Foo\n=======\nimport Aimport B\n>>>>>>> REPLACE\n*** End Patch\n";
    let (code, _, err) = acus_in(d.path(), &["patch"], p);
    assert_eq!(code, 2);
    assert!(
        err.starts_with("error: patch would break the syntax of a.swift near line 1"),
        "{err}"
    );
}

#[test]
fn patch_still_refuses_an_unclosed_rust_block() {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join("a.rs"), "fn a() {\n    x();\n}\n").unwrap();
    let p = "*** Begin Patch\n*** Update File: a.rs\n<<<<<<< SEARCH\n    x();\n}\n=======\n    x();\n>>>>>>> REPLACE\n*** End Patch\n";
    let (code, _, err) = acus_in(d.path(), &["patch"], p);
    assert_eq!(code, 2);
    assert!(
        err.starts_with("error: patch would break the syntax of a.rs"),
        "{err}"
    );
}

#[test]
fn patch_notes_a_search_matched_with_crlf_line_endings() {
    let d = tempfile::tempdir().unwrap();
    let file = d.path().join("w.txt");
    std::fs::write(&file, "a\r\nb\r\nc\r\n").unwrap();
    let p = "*** Begin Patch\n*** Update File: w.txt\n<<<<<<< SEARCH\na\nb\n=======\nA\nB\n>>>>>>> REPLACE\n*** End Patch\n";
    let (code, out, _) = acus_in(d.path(), &["patch", "-q"], p);
    assert_eq!(code, 0);
    assert_eq!(
        out,
        "M w.txt +2 -2\nnote: matched with CRLF line endings in w.txt block 1\n"
    );
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "A\r\nB\r\nc\r\n");
}

#[test]
fn patch_reports_tolerant_matches_and_new_directives() {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join("a.py"), "def f():\n\treturn 1  \n\tpass\n").unwrap();
    let p = "*** Update File: a.py\n<<<<<<< SEARCH\ndef f():\n\treturn 1\n\tpass\n=======\ndef f():\n\treturn 2\n\tpass\n>>>>>>> REPLACE\n*** Copy File: a.py\n*** To: b.py\n*** Insert After: b.py:1\n+# top\n";
    let (code, out, err) = acus_in(d.path(), &["patch", "-q"], p);
    assert_eq!((code, err.as_str()), (0, ""), "{out}");
    assert_eq!(
        out,
        "M a.py +1 -1\nA b.py +4\nnote: matched ignoring trailing whitespace in a.py block 1\n"
    );
    assert_eq!(
        std::fs::read_to_string(d.path().join("b.py")).unwrap(),
        "def f():\n# top\n\treturn 2\n\tpass\n"
    );
    let (code, _, err) = acus_in(
        d.path(),
        &["patch"],
        "*** Update File: a.py\n<<<<<<< SEARCH\nnothing like\n=======\nx\n>>>>>>> REPLACE\n",
    );
    assert_eq!(code, 2);
    assert!(err.contains("block 1: SEARCH not found"), "{err}");
}

#[test]
fn patch_notes_a_search_matched_ignoring_indentation() {
    let d = tempfile::tempdir().unwrap();
    let file = d.path().join("a.go");
    std::fs::write(&file, "func f() {\n\tif x {\n\t\treturn 1\n\t}\n}\n").unwrap();
    let p = "*** Update File: a.go\n<<<<<<< SEARCH\n    if x {\n        return 1\n    }\n=======\n    if x {\n        return 2\n    }\n>>>>>>> REPLACE\n";
    let (code, out, err) = acus_in(d.path(), &["patch", "-q"], p);
    assert_eq!((code, err.as_str()), (0, ""), "{out}");
    let _dropguard = snapshot_settings().bind_to_scope();
    insta::assert_snapshot!("cli__patch_notes_indentation", out);
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        "func f() {\n\tif x {\n\t\treturn 2\n\t}\n}\n"
    );
}

#[test]
fn patch_check_writes_nothing_and_exits_0() {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join("a.rs"), "fn f() {\n    1\n}\n").unwrap();
    let p = "*** Begin Patch\n*** Update File: a.rs\n<<<<<<< SEARCH\n    1\n=======\n    2\n>>>>>>> REPLACE\n*** End Patch\n";
    let (code, _, _) = acus_in(d.path(), &["patch", "--check"], p);
    assert_eq!(code, 0);
    assert_eq!(
        std::fs::read_to_string(d.path().join("a.rs")).unwrap(),
        "fn f() {\n    1\n}\n"
    );
}

#[test]
fn patch_check_failing_search_exits_2_and_changes_nothing() {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join("a.rs"), "fn f() {\n    1\n}\n").unwrap();
    std::fs::write(d.path().join("b.rs"), "fn g() {}\n").unwrap();
    let p = "*** Begin Patch\n*** Update File: a.rs\n<<<<<<< SEARCH\n    1\n=======\n    2\n>>>>>>> REPLACE\n*** Update File: b.rs\n<<<<<<< SEARCH\nfn missing() {}\n=======\nfn h() {}\n>>>>>>> REPLACE\n*** End Patch\n";
    let (code, _, err) = acus_in(d.path(), &["patch", "--check"], p);
    assert_eq!(code, 2);
    assert!(
        err.starts_with("error: b.rs: block 1: SEARCH not found"),
        "{err}"
    );
    assert_eq!(
        std::fs::read_to_string(d.path().join("a.rs")).unwrap(),
        "fn f() {\n    1\n}\n"
    );
    assert_eq!(
        std::fs::read_to_string(d.path().join("b.rs")).unwrap(),
        "fn g() {}\n"
    );
}

#[test]
fn patch_check_add_and_delete_change_nothing_on_disk() {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join("old.rs"), "gone\n").unwrap();
    let p = "*** Begin Patch\n*** Add File: new.rs\n+fn hi() {}\n*** Delete File: old.rs\n*** End Patch\n";
    let (code, _, _) = acus_in(d.path(), &["patch", "--check"], p);
    assert_eq!(code, 0);
    assert!(!d.path().join("new.rs").exists());
    assert!(d.path().join("old.rs").exists());
}

#[test]
fn patch_diff_prints_a_unified_diff_and_applies() {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join("a.rs"), "fn f() {\n    1\n}\n").unwrap();
    std::fs::write(d.path().join("old.rs"), "gone\n").unwrap();
    let p = "*** Begin Patch\n*** Update File: a.rs\n<<<<<<< SEARCH\n    1\n=======\n    2\n>>>>>>> REPLACE\n*** Add File: new.rs\n+fn hi() {}\n*** Delete File: old.rs\n*** End Patch\n";
    let (code, preview, _) = acus_in(d.path(), &["patch", "--diff", "--check"], p);
    assert_eq!(code, 0);
    assert!(d.path().join("old.rs").exists());
    assert!(!d.path().join("new.rs").exists());
    let (code, out, _) = acus_in(d.path(), &["patch", "--diff"], p);
    assert_eq!(code, 0);
    assert_eq!(out, preview);
    let _dropguard = snapshot_settings().bind_to_scope();
    insta::assert_snapshot!("cli__patch_diff", out);
    assert_eq!(
        std::fs::read_to_string(d.path().join("a.rs")).unwrap(),
        "fn f() {\n    2\n}\n"
    );
}

#[test]
fn patch_check_refuses_fmt() {
    let d = tempfile::tempdir().unwrap();
    let (code, _, err) = acus_in(
        d.path(),
        &["patch", "--check", "--fmt"],
        "*** Begin Patch\n*** End Patch\n",
    );
    assert_eq!(code, 2);
    assert!(
        err.starts_with("error: --fmt cannot be combined with --check"),
        "{err}"
    );
}

// Edits a.txt, then moves it, in one patch. `git diff --numstat -M` on the same change:
// `2 2 a.txt => m.txt`.
const EDIT_THEN_MOVE: &str = "*** Begin Patch\n*** Update File: a.txt\n<<<<<<< SEARCH\nb\n=======\nx\n>>>>>>> REPLACE\n*** Update File: a.txt\n*** Move to: m.txt\n<<<<<<< SEARCH\nc\n=======\ny\n>>>>>>> REPLACE\n*** End Patch\n";

#[test]
fn patch_edit_then_move_prints_one_line_with_net_counts() {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join("a.txt"), "a\nb\nc\nd\n").unwrap();
    let (code, out, _) = acus_in(d.path(), &["patch"], EDIT_THEN_MOVE);
    assert_eq!(code, 0);
    assert_eq!(out, "R a.txt -> m.txt +2 -2\n");
}

#[test]
fn patch_json_edit_then_move_has_one_change() {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join("a.txt"), "a\nb\nc\nd\n").unwrap();
    let (code, out, err) = acus_in(d.path(), &["patch", "--json"], EDIT_THEN_MOVE);
    assert_eq!(code, 0, "{err}");
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["changes"], serde_json::json!(["R a.txt -> m.txt +2 -2"]));
    assert_eq!(v["written"], true);
    assert!(!d.path().join("a.txt").exists());
    assert_eq!(
        std::fs::read_to_string(d.path().join("m.txt")).unwrap(),
        "a\nx\ny\nd\n"
    );
}

#[test]
fn patch_echo_stops_after_120_region_lines() {
    let d = tempfile::tempdir().unwrap();
    let body: String = (1..=400).map(|i| format!("line {i}\n")).collect();
    std::fs::write(d.path().join("a.txt"), &body).unwrap();
    let mut p = String::from("*** Begin Patch\n*** Update File: a.txt\n");
    for k in 0..12 {
        let n = 10 + k * 30;
        let new: String = (0..11).map(|j| format!("new {n}.{j}\n")).collect();
        let next = n + 1;
        p += &format!("<<<<<<< SEARCH\nline {n}\nline {next}\n=======\n{new}>>>>>>> REPLACE\n");
    }
    p += "*** End Patch\n";
    let (code, out, err) = acus_in(d.path(), &["patch"], &p);
    assert_eq!(code, 0, "{err}");
    assert!(
        out.contains("more written regions: acus show a.txt:"),
        "{out}"
    );
    assert!(out.lines().count() < 160, "{out}");
    assert!(out.matches("new ").count() <= 120, "{out}");
}

#[cfg(unix)]
#[test]
fn patch_through_a_symlink_edits_the_target_and_keeps_the_link() {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join("AGENTS.md"), "a\nb\n").unwrap();
    std::os::unix::fs::symlink("AGENTS.md", d.path().join("CLAUDE.md")).unwrap();
    let p = "*** Begin Patch\n*** Update File: CLAUDE.md\n<<<<<<< SEARCH\nb\n=======\nc\n>>>>>>> REPLACE\n*** End Patch\n";
    let (code, _, err) = acus_in(d.path(), &["patch"], p);
    assert_eq!(code, 0, "{err}");
    assert!(
        std::fs::symlink_metadata(d.path().join("CLAUDE.md"))
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(
        std::fs::read_to_string(d.path().join("AGENTS.md")).unwrap(),
        "a\nc\n"
    );
}
