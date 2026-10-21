// SPDX-License-Identifier: MPL-2.0
use crate::common::*;

const HITS: &str = "a\nhit one\nb\nc\nd\nhit two\ne\nf\ng\nh\nhit three\n";

/// `acus find hit <flags> -` over HITS, which has hits on lines 2, 6 and 11.
fn find_context(flags: &[&str]) -> (i32, String, String) {
    let dir = std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/proj"));
    let mut args = vec!["find", "hit"];
    args.extend_from_slice(flags);
    args.push("-");
    acus_in(dir, &args, HITS)
}

#[test]
fn find_agent() {
    let (code, out, _) = acus(&["find", "needle"]);
    assert_eq!(code, 0);
    let _dropguard = snapshot_settings().bind_to_scope();
    insta::assert_snapshot!("cli__find_agent", out);
}

#[test]
fn find_label_is_left_out_when_the_hit_is_the_definition() {
    let d = tempfile::tempdir().unwrap();
    // The call sits in b's body: a call on b's first line would be b's definition line too.
    std::fs::write(
        d.path().join("lib.rs"),
        "fn a() {}\nfn b() {\n    a();\n}\n",
    )
    .unwrap();
    let (code, out, _) = acus_in(d.path(), &["find", "fn a|a\\(\\)"], "");
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("1\tfn a() {}"), "{out}");
    assert!(!out.contains("@a "), "{out}");
    assert!(out.contains("@b 2-4\n3\t"), "{out}");
}

#[test]
fn find_block() {
    let _dropguard = snapshot_settings().bind_to_scope();
    insta::assert_snapshot!(
        "cli__find_block",
        acus(&["find", "needle", "--block", "-g", "*.rs"]).1
    );
}

#[test]
fn grep_context_flags_print_lines_around_hits() {
    let (code, out, err) = find_context(&["-A1"]);
    assert_eq!((code, err.as_str()), (0, ""));
    assert_eq!(
        out,
        "<stdin>\n2\thit one\n3-\tb\n--\n6\thit two\n7-\te\n--\n11\thit three\n"
    );
    assert_eq!(
        find_context(&["-B1"]).1,
        "<stdin>\n1-\ta\n2\thit one\n--\n5-\td\n6\thit two\n--\n10-\th\n11\thit three\n"
    );
    assert_eq!(
        find_context(&["-C1"]).1,
        "<stdin>\n1-\ta\n2\thit one\n3-\tb\n--\n5-\td\n6\thit two\n7-\te\n--\n10-\th\n11\thit three\n"
    );
    // Overlapping and adjacent ranges merge into one group.
    assert_eq!(
        find_context(&["-C2"]).1,
        "<stdin>\n1-\ta\n2\thit one\n3-\tb\n4-\tc\n5-\td\n6\thit two\n7-\te\n8-\tf\n9-\tg\n10-\th\n11\thit three\n"
    );
    assert_eq!(
        find_context(&["-A0"]).1,
        "<stdin>\n2\thit one\n--\n6\thit two\n--\n11\thit three\n"
    );
}

#[test]
fn grep_context_explicit_flags_win_and_the_last_repeat_wins() {
    assert_eq!(
        find_context(&["-C2", "-A0"]).1,
        find_context(&["-B2", "-A0"]).1
    );
    assert_eq!(find_context(&["-A1", "-A0"]).1, find_context(&["-A0"]).1);
    assert_eq!(find_context(&["-C1", "-C2"]).1, find_context(&["-C2"]).1);
}

#[test]
fn grep_context_conflicts_with_block() {
    let dir = std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/proj"));
    let (code, out, err) = acus_in(dir, &["find", "hit", "--block", "-C1", "-"], HITS);
    assert_eq!((code, out.as_str()), (2, ""));
    assert!(err.contains("conflicts with --block"), "{err}");
}

#[test]
fn find_skips_missing_paths_when_another_exists() {
    let (code, out, err) = acus(&["find", "Parser", "src/lib.rs", "nope.rs"]);
    assert_eq!(
        (code, err.as_str()),
        (0, "warning: path not found: nope.rs\n")
    );
    assert!(out.contains("pub struct Parser"));
    let (code, out, err) = acus(&["find", "Parser", "nope.rs", "nope2.rs"]);
    assert_eq!(
        (code, out.as_str(), err.as_str()),
        (2, "", "error: path not found: nope.rs\n")
    );
}

#[test]
fn find_reads_standard_input_for_dash() {
    let dir = std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/proj"));
    let input = "a Parser line\nnothing\n";
    let (code, out, err) = acus_in(dir, &["find", "Parser", "-"], input);
    assert_eq!((code, err.as_str()), (0, ""));
    assert!(out.contains("<stdin>") && out.contains("Parser line"));
    assert!(!out.contains("nothing"));
    let (code, out, _) = acus_in(dir, &["find", "-v", "Parser", "-"], input);
    assert_eq!(code, 0);
    assert!(out.contains("nothing") && !out.contains("Parser line"));
    let (code, _, _) = acus_in(dir, &["find", "zzz_none", "-"], input);
    assert_eq!(code, 1);
}

#[test]
fn find_json() {
    let _dropguard = snapshot_settings().bind_to_scope();
    insta::assert_snapshot!(
        "cli__find_json",
        acus(&["find", "-e", "greet", "-e", "Usage", "--json"]).1
    );
}

#[test]
fn find_caps_hits() {
    let _dropguard = snapshot_settings().bind_to_scope();
    insta::assert_snapshot!(
        "cli__find_caps_hits",
        acus(&["find", "needle", "--max-hits", "1"]).1
    );
}

#[test]
fn find_rg_habits() {
    let (code, out, _) = acus(&["find", "needle", "-l", "-n"]);
    assert_eq!(code, 0);
    assert!(out.lines().all(|l| !l.contains('\t')), "{out}");
    assert_eq!(acus(&["find", "-w", "needl"]).0, 1);
    assert_eq!(
        acus(&["find", "needle", "-t", "rust", "-l"])
            .1
            .lines()
            .count(),
        1
    );
}

#[test]
fn find_accepts_recursive_grep_habits_and_explains_the_rest() {
    for flags in [["-rn"], ["-rnH"], ["-R"], ["-nH"]] {
        let (code, _, err) = acus(&["find", "needle", flags[0]]);
        assert_eq!(code, 0, "{flags:?}: {err}");
    }
    for (flag, near) in [
        ("--files-without-match", "acus find -l PAT"),
        ("--bogus", "that flag does not exist"),
    ] {
        let (code, out, err) = acus(&["find", "needle", flag]);
        assert_eq!((code, out.as_str()), (2, ""), "{flag}");
        assert!(err.contains("unexpected argument"), "{flag}: {err}");
        assert!(
            err.contains("hint: ") && err.contains(near),
            "{flag}: {err}"
        );
        assert!(err.contains("`acus find --help`"), "{flag}: {err}");
    }
    // Other commands keep clap's plain error, and `run` carries the hint into its entry.
    let (_, _, err) = acus(&["outline", "src", "-c"]);
    assert!(!err.contains("hint:"), "{err}");
    let (_, out, _) = acus_in(
        std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/proj")),
        &["run"],
        "[[\"find\", \"needle\", \"--files-without-match\"]]",
    );
    assert!(out.contains("hint: `acus find -l PAT`"), "{out}");
}

#[test]
fn find_files_points_to_acus_files_not_a_grep_flag() {
    let (code, out, err) = acus(&["find", "needle", "--files"]);
    assert_eq!((code, out.as_str()), (2, ""), "{err}");
    assert_eq!(
        err,
        "error: unexpected argument '--files' found\nhint: use acus files [-g GLOB] [PATH] to list files\n"
    );
}

#[test]
fn find_count_lists_files_and_total() {
    let (code, out, _) = acus(&["find", "needle", "-c"]);
    assert_eq!(code, 0);
    assert_eq!(
        out,
        "README.md 1\nsrc/lib.rs 3\nweb/App.svelte 1\ntotal 5 in 3 files\n"
    );
    assert_eq!(
        acus(&["find", "needle", "--count", "-m", "1", "-g", "*.rs"]).1,
        "src/lib.rs 1\ntotal 1 in 1 files\n"
    );
    let (_, json, _) = acus(&["find", "needle", "-c", "--json"]);
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(v["total"], 5);
    assert_eq!(v["files"][1]["path"], "src/lib.rs");
    assert_eq!(v["files"][1]["count"], 3);
    assert_eq!(acus(&["find", "zzz-none", "-c"]).0, 1);
}

#[test]
fn find_only_matching_prints_the_matched_text() {
    let (code, out, _) = acus(&["find", "-o", r"need\w+", "-g", "*.rs"]);
    assert_eq!(code, 0);
    assert!(out.contains("11\tneedle\n12\tneedle\n"), "{out}");
    assert!(!out.contains("let needle"), "{out}");
    let (_, json, _) = acus(&["find", "-o", "need", "--json", "-g", "*.rs"]);
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(v["files"][0]["hits"][0]["matches"][0], "need");
    assert!(v["files"][0]["hits"][0].get("text").is_none());
}

#[test]
fn find_modes_that_print_no_lines_ignore_context_flags() {
    for modes in [
        vec!["-o", r"need\w+"],
        vec!["-c", "needle"],
        vec!["-l", "needle"],
        vec!["--json", "needle"],
    ] {
        let run = |ctx: &[&str]| {
            let mut args = vec!["find"];
            args.extend_from_slice(&modes);
            args.extend_from_slice(ctx);
            acus(&args)
        };
        let plain = run(&[]);
        assert_eq!(plain.0, 0, "{modes:?}");
        for ctx in [&["-A", "2"][..], &["-B", "1"], &["-C", "3"]] {
            assert_eq!(run(ctx), plain, "{modes:?} {ctx:?}");
        }
    }
}

#[test]
fn find_invert_and_max_count_work_per_file() {
    let (code, out, _) = acus(&["find", "-v", "needle", "README.md"]);
    assert_eq!(code, 0);
    assert!(
        out.contains("Demo") && !out.contains("Run the needle"),
        "{out}"
    );
    let (_, out, _) = acus(&["find", "-m", "1", "needle", "src/lib.rs"]);
    assert_eq!(out.matches("needle").count(), 1, "{out}");
    assert_eq!(acus(&["find", "needle", "-m", "0"]).0, 2);
}

#[test]
fn find_grep_glob_flags_map_to_globs() {
    let rs = acus(&["find", "needle", "-l", "-g", "*.rs"]).1;
    assert_eq!(acus(&["find", "needle", "-l", "--include=*.rs"]).1, rs);
    assert_eq!(acus(&["find", "needle", "-l", "--include", "*.rs"]).1, rs);
    let (_, out, _) = acus(&[
        "find",
        "needle",
        "-l",
        "--exclude",
        "*.rs",
        "--exclude-dir",
        "web",
    ]);
    assert_eq!(out, "README.md 1\n");
    assert_eq!(
        acus(&["find", "needle", "-l", "-g", "!web/", "-g", "!src/"]).1,
        "README.md 1\n"
    );
}

#[test]
fn find_accepts_extended_and_perl_flags() {
    let plain = acus(&["find", "need(le|s)"]).1;
    assert_eq!(acus(&["find", "-E", "need(le|s)"]).1, plain);
    assert_eq!(acus(&["find", "-P", "need(le|s)"]).1, plain);
    let (code, _, err) = acus(&["find", "-P", r"(?<=let )needle"]);
    assert_eq!(code, 2);
    assert!(
        err.contains("-P is accepted") && err.contains("no look-around"),
        "{err}"
    );
}

#[test]
fn find_reads_bre_alternation() {
    let (code, out, err) = acus(&["find", r"Usage\|Parser", "-l"]);
    assert_eq!(code, 0);
    assert_eq!(out, acus(&["find", "Usage|Parser", "-l"]).1);
    assert!(err.contains("note: read `\\|` as alternation"), "{err}");
    // ERE users keep their literal pipe; so do -E, -F and classes.
    assert!(!acus(&["find", r"a|b\|c", "-l"]).2.contains("note:"));
    assert!(
        !acus(&["find", "-E", r"Usage\|Parser", "-l"])
            .2
            .contains("note:")
    );
    assert_eq!(acus(&["find", "-F", r"Usage\|Parser"]).0, 1);
    assert_eq!(acus(&["find", r"[\|]x", "-l"]).0, 1);
}

#[test]
fn find_nothing_exits_1() {
    assert_eq!(
        acus(&["find", "zzz_not_here"]),
        (
            1,
            String::new(),
            "(no matches; hidden and .gitignored files were skipped: --hidden, -u)\n".into()
        )
    );
}

#[test]
fn find_nothing_names_the_glob_it_searched() {
    let (code, out, err) = acus(&["find", "zzz_not_here", "-g", "*.rs"]);
    assert_eq!((code, out.as_str()), (1, ""));
    assert!(
        err.ends_with(
            "\n(searched -g '*.rs' in .; a glob without / matches a file name at any depth)\n"
        ),
        "{err}"
    );
    assert!(!acus(&["find", "zzz_not_here"]).2.contains("searched"));
}

#[test]
fn find_json_no_matches_is_null() {
    let (code, out, err) = acus(&["find", "zzz_not_here", "--json"]);
    assert_eq!(code, 1);
    assert!(err.is_empty());
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&out).unwrap(),
        serde_json::Value::Null
    );
}

#[test]
fn bad_regex_exits_2_with_error_line() {
    let (code, out, err) = acus(&["find", "("]);
    assert_eq!((code, out.as_str()), (2, ""));
    assert!(err.starts_with("error: "), "{err}");
}

#[test]
fn find_line_regexp_needs_whole_lines() {
    let d = tempfile::tempdir().unwrap();
    let stdin = "a.b\naxb\n  a.b\n";
    let (code, out, _) = acus_in(d.path(), &["find", "-x", "-F", "a.b", "-"], stdin);
    assert_eq!(code, 0);
    assert!(
        out.contains("1\ta.b") && !out.contains("2\t") && !out.contains("3\t"),
        "{out}"
    );
    // As a regex, `.` matches the x in line 2; the indented line 3 is never a whole-line match.
    let (_, out, _) = acus_in(d.path(), &["find", "-x", "a.b", "-"], stdin);
    assert!(
        out.contains("1\ta.b") && out.contains("2\taxb") && !out.contains("3\t"),
        "{out}"
    );
}

#[cfg(unix)]
#[test]
fn find_follow_searches_through_symlinked_directories() {
    let d = tempfile::tempdir().unwrap();
    let target = tempfile::tempdir().unwrap();
    std::fs::write(target.path().join("t.rs"), "needle here\n").unwrap();
    std::os::unix::fs::symlink(target.path(), d.path().join("link")).unwrap();
    let (code, _, _) = acus_in(d.path(), &["find", "needle", "-l"], "");
    assert_eq!(code, 1);
    assert_eq!(
        acus_in(d.path(), &["find", "needle", "-l", "-L"], "").1,
        "link/t.rs 1\n"
    );
}

#[test]
fn find_multiline_prints_every_line_of_a_match() {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(
        d.path().join("m.txt"),
        "one\nstart here\nmiddle\nend\nafter\n",
    )
    .unwrap();
    let (code, out, _) = acus_in(
        d.path(),
        &["find", "-U", r"start here\nmiddle\nend", "m.txt"],
        "",
    );
    assert_eq!(code, 0);
    assert!(out.contains("2\tstart here\n3\tmiddle\n4\tend\n"), "{out}");
    let (_, out, _) = acus_in(
        d.path(),
        &["find", "-U", "-o", r"start here\nmiddle", "m.txt"],
        "",
    );
    assert!(out.contains("2\tstart here\n3\tmiddle\n"), "{out}");
    let (code, _, err) = acus_in(d.path(), &["find", r"start here\nmiddle", "m.txt"], "");
    assert_eq!(code, 1);
    assert!(err.contains("add -U"), "{err}");
}

#[test]
fn find_multiline_anchors_match_at_line_boundaries() {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join("m.txt"), "x\nfn foo\nbar\nfn foo\nbaz\n").unwrap();
    let (code, out, _) = acus_in(d.path(), &["find", "-U", r"^fn foo\nbar$", "m.txt"], "");
    assert_eq!(code, 0);
    assert!(out.contains("2\tfn foo\n3\tbar\n"), "{out}");
    assert!(!out.contains("baz"), "{out}");
}

#[test]
fn find_multiline_refuses_the_modes_it_cannot_print() {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join("m.txt"), "start here\nmiddle\n").unwrap();
    for extra in [
        &["-c"][..],
        &["-v"][..],
        &["--block"][..],
        &["-A", "1"][..],
        &["-C", "1"][..],
    ] {
        let mut args = vec!["find", "-U", r"start here\nmiddle", "m.txt"];
        args.extend_from_slice(extra);
        let (code, out, err) = acus_in(d.path(), &args, "");
        assert_eq!((code, out.as_str()), (2, ""), "{extra:?}");
        assert!(
            err.starts_with("error: ")
                && err.contains("conflicts with -U")
                && err.contains("\nhint: "),
            "{extra:?}: {err}"
        );
    }
}

/// A directory with `f.txt` (hits on lines 1, 2, 4, 5) and `a.rs` (hits in `fn one` and
/// `fn two`).
fn ranged_dir() -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join("f.txt"), "hit 1\nhit 2\nmiss\nhit 4\nhit 5\n").unwrap();
    std::fs::write(
        d.path().join("a.rs"),
        "fn one() {\n    hit();\n}\nfn two() {\n    hit();\n    hit();\n}\n",
    )
    .unwrap();
    d
}

#[test]
fn find_range_searches_only_the_lines_with_file_absolute_numbers() {
    let d = ranged_dir();
    let (code, out, _) = acus_in(d.path(), &["find", "hit", "f.txt:2-4"], "");
    assert_eq!((code, out.as_str()), (0, "f.txt\n2\thit 2\n4\thit 4\n"));
    // `-` in a range is the last N lines, as `tail -n` counts them.
    let (_, out, _) = acus_in(d.path(), &["find", "hit", "f.txt:-2"], "");
    assert_eq!(out, "f.txt\n4\thit 4\n5\thit 5\n");
    // Context does not leave the range: line 4 is outside 2-3.
    let (_, out, _) = acus_in(d.path(), &["find", "hit", "f.txt:2-3", "-A", "2"], "");
    assert_eq!(out, "f.txt\n2\thit 2\n3-\tmiss\n");
}

#[test]
fn find_from_line_range_searches_to_the_end() {
    let d = ranged_dir();
    let (code, out, _) = acus_in(d.path(), &["find", "hit", "f.txt:4-"], "");
    assert_eq!((code, out.as_str()), (0, "f.txt\n4\thit 4\n5\thit 5\n"));
}

#[test]
fn find_range_takes_a_symbol_and_mixes_with_plain_paths() {
    let d = ranged_dir();
    let (code, out, _) = acus_in(d.path(), &["find", "hit", "a.rs#two"], "");
    assert_eq!(code, 0);
    assert!(
        out.contains("5\thit();") && out.contains("6\thit();"),
        "{out}"
    );
    assert!(!out.contains("2\t"), "{out}");
    let (_, out, _) = acus_in(d.path(), &["find", "-l", "hit", "a.rs#one", "f.txt:4"], "");
    assert_eq!(out, "a.rs 1\nf.txt 1\n");
    let (_, out, _) = acus_in(d.path(), &["find", "hit", "f.txt:4", "f.txt"], "");
    assert!(
        out.contains("4\thit 4") && out.contains("1\thit 1"),
        "{out}"
    );
}

#[test]
fn find_range_works_with_count_block_and_max_count() {
    let d = ranged_dir();
    let (_, out, _) = acus_in(d.path(), &["find", "hit", "f.txt:1-2", "-c"], "");
    assert_eq!(out, "f.txt 2\ntotal 2 in 1 files\n");
    let (_, out, _) = acus_in(d.path(), &["find", "hit", "f.txt:4-5", "-m", "1"], "");
    assert_eq!(out, "f.txt\n4\thit 4\n");
    let (_, out, _) = acus_in(d.path(), &["find", "hit", "a.rs:5", "--block"], "");
    assert!(
        out.contains("== a.rs#two 4-7") || out.contains("@two 4-7"),
        "{out}"
    );
    assert!(!out.contains("fn one"), "{out}");
    let (code, _, _) = acus_in(d.path(), &["find", "hit", "f.txt:9-12"], "");
    assert_eq!(code, 2);
}

#[test]
fn find_range_errors_name_the_address() {
    let d = ranged_dir();
    let (code, out, err) = acus_in(d.path(), &["find", "hit", "a.rs#nope"], "");
    assert_eq!((code, out.as_str()), (2, ""));
    assert!(err.contains("no symbol `nope` in a.rs"), "{err}");
    let (code, _, err) = acus_in(d.path(), &["find", "hit", "gone.txt:1-2"], "");
    assert_eq!(code, 2);
    assert!(err.contains("path not found: gone.txt"), "{err}");
    // A plain path wins over the address reading when the file exists under that name.
    std::fs::write(d.path().join("odd:1-2.txt"), "hit odd\n").unwrap();
    let (_, out, _) = acus_in(d.path(), &["find", "hit", "odd:1-2.txt"], "");
    assert_eq!(out, "odd:1-2.txt\n1\thit odd\n");
}

#[test]
fn find_cut_clips_hit_lines_char_safely_and_names_the_rest() {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join("c.txt"), "abcdefghijklmnopqrstuvwxyz\n").unwrap();
    let (_, out, _) = acus_in(d.path(), &["find", "a", "c.txt", "--cut", "10"], "");
    assert_eq!(out, "c.txt\n1\tabcdefghij… (+16 chars)\n");
    // Ten multi-byte characters: cut counts chars, so no byte boundary is split.
    std::fs::write(d.path().join("u.txt"), "é".repeat(20) + "\n").unwrap();
    let (code, out, _) = acus_in(d.path(), &["find", "é", "u.txt", "--cut", "5"], "");
    assert_eq!(
        (code, out),
        (0, format!("u.txt\n1\t{}… (+15 chars)\n", "é".repeat(5)))
    );
}

#[test]
fn find_last_keeps_the_last_hits_per_file_after_max_count() {
    let d = ranged_dir();
    let (_, out, _) = acus_in(d.path(), &["find", "hit", "f.txt", "--last", "2"], "");
    assert_eq!(out, "f.txt\n4\thit 4\n5\thit 5\n");
    // -m stops the search at its N matches first; --last then keeps the last of those.
    let (_, out, _) = acus_in(
        d.path(),
        &["find", "hit", "f.txt", "-m", "2", "--last", "1"],
        "",
    );
    assert_eq!(out, "f.txt\n2\thit 2\n");
    // -c and -l still count every hit.
    let (_, out, _) = acus_in(d.path(), &["find", "hit", "f.txt", "-c", "--last", "1"], "");
    assert_eq!(out, "f.txt 4\ntotal 4 in 1 files\n");
    assert_eq!(
        acus_in(d.path(), &["find", "hit", "f.txt", "--last", "0"], "").0,
        2
    );
}

#[test]
fn long_lines_and_decorators() {
    let d = tempfile::tempdir().unwrap();
    let min = format!("{}needle{}\n", "a;".repeat(5000), ";b".repeat(5000));
    std::fs::write(d.path().join("min.js"), min).unwrap();
    std::fs::write(
        d.path().join("s.py"),
        "import x\n\n\n@cache\ndef f():\n    pass\n",
    )
    .unwrap();
    let (_, out, _) = acus_in(d.path(), &["find", "needle"], "");
    assert!(
        out.len() < 400 && out.contains("needle") && out.contains("chars)"),
        "{out}"
    );
    let (_, out, _) = acus_in(d.path(), &["find", "-e", "--x", "-e", "needle", "-l"], "");
    assert_eq!(out, "min.js 1\n");
    let (_, out, _) = acus_in(d.path(), &["show", "s.py#f"], "");
    assert_eq!(out, "== s.py#f 4-6\n4\t@cache\n5\tdef f():\n6\t    pass\n");
    let (_, out, _) = acus_in(d.path(), &["run", "--json"], "find zzz\nshow s.py:1\n");
    assert_eq!(out.lines().next(), Some("null"));
}
