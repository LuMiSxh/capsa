// SPDX-License-Identifier: MPL-2.0
use crate::common::*;

const NOTE: &str = "(no files; hidden and .gitignored files were skipped: --hidden, -u)\n";
const ZZZ_HINT: &str =
    "(searched -g '*.zzz' in .; a glob without / matches a file name at any depth)\n";

#[test]
fn files_lists_the_fixture_sorted_with_relative_paths() {
    assert_eq!(
        acus(&["files"]),
        (
            0,
            "README.md\nsrc/lib.rs\nweb/App.svelte\n".into(),
            String::new()
        )
    );
    assert_eq!(acus(&["files", "src"]).1, "src/lib.rs\n");
    assert_eq!(acus(&["files", "./src/"]).1, "src/lib.rs\n");
}

#[test]
fn files_takes_globs_and_types() {
    assert_eq!(acus(&["files", "-g", "*.rs"]).1, "src/lib.rs\n");
    assert_eq!(acus(&["files", "-g", "!web/"]).1, "README.md\nsrc/lib.rs\n");
    assert_eq!(acus(&["files", "-t", "rust"]).1, "src/lib.rs\n");
    assert_eq!(acus(&["files", "-t", "svelte"]).1, "web/App.svelte\n");
    assert_eq!(
        acus(&["files", "-g", "*.zzz"]),
        (1, String::new(), format!("{NOTE}{ZZZ_HINT}"))
    );
    assert_eq!(acus(&["files", "no/such/dir"]).0, 2);
}

#[test]
fn files_glob_without_slash_matches_the_name_at_any_depth() {
    let d = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(d.path().join("Tests/A/B")).unwrap();
    std::fs::write(d.path().join("Tests/A/B/FooTests.swift"), "").unwrap();
    std::fs::write(d.path().join("Tests/BarTests.swift"), "").unwrap();
    let files = |args: &[&str]| acus_in(d.path(), args, "").1;
    assert_eq!(
        files(&["files", "Tests", "-g", "FooTests.swift"]),
        "Tests/A/B/FooTests.swift\n"
    );
    assert_eq!(
        files(&["files", "-g", "FooTests.swift"]),
        "Tests/A/B/FooTests.swift\n"
    );
    // A glob with a `/` stays anchored at the search root.
    assert_eq!(
        acus_in(d.path(), &["files", "Tests", "-g", "B/FooTests.swift"], "").0,
        1
    );
}

#[test]
fn files_empty_glob_result_names_what_was_searched() {
    let d = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(d.path().join("Tests")).unwrap();
    let (code, out, err) = acus_in(d.path(), &["files", "Tests", "-g", "*.zzz"], "");
    assert_eq!((code, out.as_str()), (1, ""));
    assert!(
        err.ends_with(
            "(searched -g '*.zzz' in Tests; a glob without / matches a file name at any depth)\n"
        ),
        "{err}"
    );
}

#[test]
fn files_human_view_aligns_line_counts_in_a_table() {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join("a.txt"), "x\ny\n").unwrap();
    std::fs::write(d.path().join("bb.rs"), "fn f() {}\n").unwrap();
    std::fs::write(d.path().join("c.bin"), b"\0\x01").unwrap();
    assert_eq!(
        acus_in(d.path(), &["files", "--human", "--lines"], ""),
        (
            0,
            "  path   lines\n  a.txt      2\n  bb.rs      1\n  c.bin      -\n  total      3\n"
                .into(),
            String::new()
        )
    );
    assert_eq!(
        acus_in(d.path(), &["files", "--human"], ""),
        (
            0,
            "  path\n  a.txt\n  bb.rs\n  c.bin\n".into(),
            String::new()
        )
    );
}

#[test]
fn files_skips_hidden_and_ignored_files_unless_asked() {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join("a.txt"), "").unwrap();
    std::fs::write(d.path().join(".hidden"), "").unwrap();
    std::fs::write(d.path().join(".gitignore"), "ignored.txt\n").unwrap();
    std::fs::write(d.path().join("ignored.txt"), "").unwrap();
    assert_eq!(acus_in(d.path(), &["files"], "").1, "a.txt\n");
    assert_eq!(
        acus_in(d.path(), &["files", "-u"], "").1,
        "a.txt\nignored.txt\n"
    );
    assert_eq!(
        acus_in(d.path(), &["files", "--hidden", "-u"], "").1,
        ".gitignore\n.hidden\na.txt\nignored.txt\n"
    );
    let hidden_only = tempfile::tempdir().unwrap();
    std::fs::write(hidden_only.path().join(".only"), "").unwrap();
    assert_eq!(
        acus_in(hidden_only.path(), &["files"], ""),
        (1, String::new(), NOTE.into())
    );
    assert_eq!(
        acus_in(hidden_only.path(), &["files", "--hidden"], "").1,
        ".only\n"
    );
}

#[cfg(unix)]
#[test]
fn files_follow_symlinked_directories_with_l() {
    let d = tempfile::tempdir().unwrap();
    let target = tempfile::tempdir().unwrap();
    std::fs::write(target.path().join("t.rs"), "x").unwrap();
    std::os::unix::fs::symlink(target.path(), d.path().join("link")).unwrap();
    assert_eq!(acus_in(d.path(), &["files"], "").0, 1);
    assert_eq!(acus_in(d.path(), &["files", "-L"], "").1, "link/t.rs\n");
}

#[test]
fn files_caps_the_listing_and_names_the_narrowing() {
    let d = tempfile::tempdir().unwrap();
    for i in 0..201 {
        std::fs::write(d.path().join(format!("f{i:03}.txt")), "").unwrap();
    }
    let (code, out, _) = acus_in(d.path(), &["files"], "");
    assert_eq!(code, 0);
    assert_eq!(out.lines().count(), 201);
    assert_eq!(out.lines().nth(199), Some("f199.txt"));
    assert_eq!(
        out.lines().last(),
        Some("… 1 more; narrow with -g or a path")
    );
}

/// Newlines in a fixture file, as `wc -l` counts them.
fn wc_l(path: &str) -> usize {
    let fixture = std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/proj"));
    std::fs::read(fixture.join(path))
        .unwrap()
        .iter()
        .filter(|&&b| b == b'\n')
        .count()
}

#[test]
fn files_lines_appends_counts_and_a_total() {
    let (code, out, _) = acus(&["files", "--lines"]);
    let total = wc_l("README.md") + wc_l("src/lib.rs") + wc_l("web/App.svelte");
    assert_eq!(code, 0);
    assert_eq!(
        out,
        format!(
            "README.md  {}\nsrc/lib.rs  {}\nweb/App.svelte  {}\ntotal  {total}\n",
            wc_l("README.md"),
            wc_l("src/lib.rs"),
            wc_l("web/App.svelte")
        )
    );
    assert_eq!(
        acus(&["files", "--lines", "-g", "*.rs"]).1,
        format!(
            "src/lib.rs  {}\ntotal  {}\n",
            wc_l("src/lib.rs"),
            wc_l("src/lib.rs")
        )
    );
    assert_eq!(
        acus(&["files", "src", "--lines"]).1,
        format!(
            "src/lib.rs  {}\ntotal  {}\n",
            wc_l("src/lib.rs"),
            wc_l("src/lib.rs")
        )
    );
}

#[test]
fn files_lines_marks_binary_files_with_a_dash_and_leaves_them_out_of_the_total() {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join("a.txt"), "x\ny\n").unwrap();
    std::fs::write(d.path().join("b.bin"), b"a\0\nb\n").unwrap();
    let (code, out, _) = acus_in(d.path(), &["files", "--lines"], "");
    assert_eq!((code, out.as_str()), (0, "a.txt  2\nb.bin  -\ntotal  2\n"));
}

#[test]
fn files_lines_json_has_counts_and_total() {
    let (code, out, _) = acus(&["files", "--lines", "--json"]);
    assert_eq!(code, 0);
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["files"][0]["path"], "README.md");
    assert_eq!(v["files"][0]["lines"], wc_l("README.md"));
    assert_eq!(v["truncated"], 0);
    assert_eq!(
        v["total"],
        wc_l("README.md") + wc_l("src/lib.rs") + wc_l("web/App.svelte")
    );
}

#[test]
fn files_json_lists_files_and_is_null_when_empty() {
    let (code, out, _) = acus(&["files", "-g", "*.rs", "--json"]);
    assert_eq!(code, 0);
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["files"], serde_json::json!(["src/lib.rs"]));
    assert_eq!(v["truncated"], 0);
    assert_eq!(
        acus(&["files", "-g", "*.zzz", "--json"]),
        (1, "null\n".into(), String::new())
    );
}
