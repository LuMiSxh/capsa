// SPDX-License-Identifier: MPL-2.0
use crate::common::*;

#[test]
fn show_keeps_going_past_missing_files() {
    let (code, out, err) = acus(&["show", "nope.rs", "src/lib.rs:1"]);
    assert_eq!(
        (code, err.as_str()),
        (1, "error: path not found: nope.rs\n")
    );
    assert!(out.starts_with("== src/lib.rs 1-1\n"));
    let (code, out, err) = acus(&["show", "nope.rs", "nope2.rs"]);
    assert_eq!(
        (code, out.as_str(), err.as_str()),
        (
            2,
            "",
            "error: path not found: nope.rs\nerror: path not found: nope2.rs\n"
        )
    );
}

#[test]
fn show_exit_code_reflects_failed_addresses() {
    // One address that works and one missing: the working one is printed, and the exit is 1.
    let (code, out, err) = acus(&["show", "src/lib.rs:1", "nope.rs"]);
    assert_eq!(
        (code, err.as_str()),
        (1, "error: path not found: nope.rs\n")
    );
    assert!(out.starts_with("== src/lib.rs 1-1\n"), "{out}");
    // A missing symbol fails its address alone; the others still print.
    let (code, out, err) = acus(&["show", "src/lib.rs#nope", "src/lib.rs:1"]);
    assert_eq!(code, 1, "{err}");
    assert!(out.starts_with("== src/lib.rs 1-1\n"), "{out}");
    assert!(
        err.contains("error: no symbol `nope` in src/lib.rs"),
        "{err}"
    );
    // Every address present: exit 0.
    let (code, _, err) = acus(&["show", "src/lib.rs:1", "src/lib.rs:2"]);
    assert_eq!((code, err.as_str()), (0, ""));
}

#[test]
fn outline_file_and_dir() {
    let _dropguard = snapshot_settings().bind_to_scope();
    insta::assert_snapshot!("cli__outline_file", acus(&["outline", "src/lib.rs"]).1);
    insta::assert_snapshot!(
        "cli__outline_dir",
        acus(&["outline", ".", "--depth", "1"]).1
    );
}

#[test]
fn show_symbol_suffix_range_and_markdown() {
    let _dropguard = snapshot_settings().bind_to_scope();
    insta::assert_snapshot!(
        "cli__show_symbol_suffix_range_and_markdown",
        acus(&[
            "show",
            "src/lib.rs#parse",
            "src/lib.rs:1-3",
            "README.md#Usage"
        ])
        .1
    );
}

#[test]
fn show_errors_have_hints() {
    let (code, _, err) = acus(&["show", "src/lib.rs#nope"]);
    assert_eq!(code, 2);
    assert_eq!(
        err,
        "error: no symbol `nope` in src/lib.rs\nhint: acus outline src/lib.rs\n"
    );
    let (code, _, err) = acus(&["show", "src/lib.rs:99"]);
    assert_eq!(
        (code, err.as_str()),
        (2, "error: src/lib.rs has 17 lines\n")
    );
}

#[test]
fn show_caps_whole_files() {
    let _dropguard = snapshot_settings().bind_to_scope();
    insta::assert_snapshot!(
        "cli__show_caps_whole_files",
        acus(&["show", "src/lib.rs", "--max-lines", "3"]).1
    );
}

/// Writes a 500-line Rust file to `dir` and returns its text.
fn long_rust(dir: &std::path::Path) -> String {
    let src: String = (1..=500)
        .map(|i| match i % 10 {
            1 => format!("fn f{i}() {{\n"),
            0 => "}\n".to_owned(),
            _ => format!("    let x{i} = {i};\n"),
        })
        .collect();
    std::fs::write(dir.join("big.rs"), &src).unwrap();
    src
}

#[test]
fn show_caps_a_long_whole_file_with_its_outline() {
    let d = tempfile::tempdir().unwrap();
    long_rust(d.path());
    let (code, out, err) = acus_in(d.path(), &["show", "big.rs"], "");
    assert_eq!((code, err.as_str()), (0, ""));
    assert!(out.starts_with("== big.rs 1-500\n"), "{out}");
    assert!(out.contains("big.rs rust 500\n"), "{out}");
    assert!(out.contains("fn f1 1-10\n"), "{out}");
    assert!(out.contains("\n60\t"), "{out}");
    assert!(!out.contains("\n61\t"), "{out}");
    assert!(
        out.contains("… 440 more lines: acus show big.rs:61-500"),
        "{out}"
    );
    assert!(out.contains("or acus show big.rs#Symbol"), "{out}");
}

#[test]
fn show_json_is_never_capped() {
    let d = tempfile::tempdir().unwrap();
    long_rust(d.path());
    let (code, out, err) = acus_in(d.path(), &["show", "big.rs", "--json"], "");
    assert_eq!((code, err.as_str()), (0, ""));
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v[0]["start"], 1);
    assert_eq!(v[0]["end"], 500);
    assert_eq!(v[0]["text"].as_str().unwrap().lines().count(), 500);
}

#[test]
fn show_full_prints_every_line_of_a_long_file() {
    let d = tempfile::tempdir().unwrap();
    long_rust(d.path());
    let (code, out, _) = acus_in(d.path(), &["show", "big.rs", "--full"], "");
    assert_eq!(code, 0);
    assert!(out.contains("\n500\t"), "{out}");
    assert!(!out.contains("more lines"), "{out}");
    assert!(!out.contains("big.rs rust"), "{out}");
}

#[test]
fn show_does_not_cap_an_explicit_range() {
    let d = tempfile::tempdir().unwrap();
    long_rust(d.path());
    let (code, out, _) = acus_in(d.path(), &["show", "big.rs:1-400"], "");
    assert_eq!(code, 0);
    assert!(out.contains("\n400\t"), "{out}");
    assert!(!out.contains("more lines"), "{out}");
    assert!(!out.contains("big.rs rust"), "{out}");
}

#[test]
fn show_cuts_a_multi_address_call_at_the_character_budget() {
    let d = tempfile::tempdir().unwrap();
    let text = format!("{}\n", "x".repeat(100)).repeat(600);
    std::fs::write(d.path().join("a.txt"), &text).unwrap();
    std::fs::write(d.path().join("b.txt"), &text).unwrap();
    let (code, out, err) = acus_in(d.path(), &["show", "a.txt:1-600", "b.txt:1-600"], "");
    assert_eq!((code, err.as_str()), (0, ""));
    assert!(
        out.contains("output budget spent (24000 chars); the rest: acus show a.txt:"),
        "{out}"
    );
    assert!(out.contains("-600 b.txt:1-600"), "{out}");
    assert!(out.chars().count() < 25_000, "{}", out.chars().count());
}

#[test]
fn show_json() {
    let _dropguard = snapshot_settings().bind_to_scope();
    insta::assert_snapshot!(
        "cli__show_json",
        acus(&["show", "src/lib.rs#Parser::new", "--json"]).1
    );
}

#[cfg(feature = "lang-shell")]
#[test]
fn shell_outline_and_comment_address() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("run.sh");
    std::fs::write(&file, "# greets\ngreet() { echo hi; }\n").unwrap();
    let path = file.to_str().unwrap();
    let (code, out, err) = acus(&["outline", path]);
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("fn greet 2-2"), "{out}");
    // The address takes the comment above the function with it.
    let (code, out, err) = acus(&["show", &format!("{path}#greet")]);
    assert_eq!(code, 0, "{err}");
    assert!(
        out.contains("1-2\n1\t# greets\n2\tgreet() { echo hi; }"),
        "{out}"
    );
}

#[cfg(feature = "lang-xml")]
#[test]
fn plist_outline_and_key_address() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("Info.plist");
    std::fs::write(
        &file,
        "<plist>\n<dict>\n  <key>CFBundleIdentifier</key>\n  <string>x</string>\n</dict>\n</plist>\n",
    )
    .unwrap();
    let path = file.to_str().unwrap();
    let (code, out, err) = acus(&["outline", path]);
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("key CFBundleIdentifier 3-3"), "{out}");
    // A plist key resolves by its text.
    let (code, out, err) = acus(&["show", &format!("{path}#CFBundleIdentifier")]);
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("3\t  <key>CFBundleIdentifier</key>"), "{out}");
}

#[test]
fn tilde_paths_expand_to_home() {
    let home = tempfile::tempdir().unwrap();
    let h = home.path();
    std::fs::write(h.join("w.rs"), "fn worker() {}\nfn other() {}\n").unwrap();
    // `std::env::home_dir` reads USERPROFILE on Windows and HOME elsewhere.
    let env = [
        ("HOME", h.to_str().unwrap()),
        ("USERPROFILE", h.to_str().unwrap()),
    ];
    // Command-line arguments: a symbol and a range, with the address suffix kept.
    let (code, out, err) = acus_env(&["show", "~/w.rs#worker", "~/w.rs:2-2"], &env);
    assert_eq!(code, 0, "{err}");
    assert!(
        out.contains("fn worker() {}") && out.contains("fn other() {}"),
        "{out}"
    );
    // Lines of `acus run` get no shell either.
    let (code, out, err) = acus_in_env(h, &["run"], "show ~/w.rs:1-1\n", &env);
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("fn worker() {}"), "{out}");
    // Paths named inside patch text, and the patch file itself.
    std::fs::write(
        h.join("p.patch"),
        "*** Begin Patch\n*** Update File: ~/w.rs\n<<<<<<< SEARCH\nfn other() {}\n=======\nfn other() { 1 }\n>>>>>>> REPLACE\n*** End Patch\n",
    )
    .unwrap();
    let (code, _, err) = acus_in_env(h, &["patch", "-q", "--file", "~/p.patch"], "", &env);
    assert_eq!(code, 0, "{err}");
    assert_eq!(
        std::fs::read_to_string(h.join("w.rs")).unwrap(),
        "fn worker() {}\nfn other() { 1 }\n"
    );
}

#[test]
fn show_from_line_addresses() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    let ten: String = (1..=10).map(|i| format!("l{i}\n")).collect();
    std::fs::write(d.join("t.txt"), &ten).unwrap();

    let run = |addr: &str| acus_in_env(d, &["show", addr], "", &[]);
    // Line N to the end of the file, numbered by the file's own lines.
    let from: String = (3..=10).map(|i| format!("{i}\tl{i}\n")).collect();
    assert_eq!(
        run("t.txt:3-"),
        (0, format!("== t.txt 3-10\n{from}"), String::new())
    );
    // Past the end, the same as an out-of-range `A-B` range.
    assert_eq!(run("t.txt:99-"), run("t.txt:99-100"));
    assert_ne!(run("t.txt:99-").0, 0);
}

#[test]
fn show_tail_addresses() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    let ten: String = (1..=10).map(|i| format!("l{i}\n")).collect();
    std::fs::write(d.join("t.txt"), &ten).unwrap();
    std::fs::write(d.join("e.txt"), "").unwrap();
    std::fs::write(d.join("n.txt"), "a\nb\nc").unwrap();

    let run = |addr: &str| acus_in_env(d, &["show", addr], "", &[]);
    // The last N lines, numbered by their place in the file.
    let (code, out, err) = run("t.txt:-2");
    assert_eq!((code, err.as_str()), (0, ""));
    assert_eq!(out, "== t.txt 9-10\n9\tl9\n10\tl10\n");
    // N past the top of the file shows all of it.
    let all: String = (1..=10).map(|i| format!("{i}\tl{i}\n")).collect();
    assert_eq!(run("t.txt:-50").1, format!("== t.txt 1-10\n{all}"));
    // An empty file has no lines, the same as its whole-file form.
    assert_eq!(
        run("e.txt:-3"),
        (0, "== e.txt 1-0\n".to_string(), String::new())
    );
    // A file without a trailing newline still counts its last line.
    assert_eq!(run("n.txt:-2").1, "== n.txt 2-3\n2\tb\n3\tc\n");
    // `-0` is not a line count.
    let (code, _, err) = run("t.txt:-0");
    assert_eq!(
        (code, err.as_str()),
        (
            2,
            "error: t.txt:-0: tail needs at least 1 line\nhint: acus show t.txt:-1\n"
        )
    );
    // A file whose own name ends in `:-N` is still shown whole. Windows forbids `:` in names.
    if cfg!(unix) {
        std::fs::write(d.join("x:-3"), "a\nb\n").unwrap();
        assert_eq!(run("x:-3").1, "== x:-3 1-2\n1\ta\n2\tb\n");
    }
}

#[test]
fn outline_kind_filter() {
    let _dropguard = snapshot_settings().bind_to_scope();
    insta::assert_snapshot!(
        "cli__outline_kind_fn",
        acus(&["outline", "src/lib.rs", "--kind", "fn"]).1
    );
    insta::assert_snapshot!(
        "cli__outline_kind_struct_enum",
        acus(&["outline", "src/lib.rs", "--kind", "struct,enum"]).1
    );
}

#[test]
fn outline_unknown_kind_errors() {
    let (code, out, err) = acus(&["outline", "src/lib.rs", "--kind", "fnn"]);
    assert_eq!((code, out.as_str()), (2, ""));
    assert!(
        err.starts_with("error: unknown kind `fnn`\nhint: valid kinds are actor, array, "),
        "{err}"
    );
}
