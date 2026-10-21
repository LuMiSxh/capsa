// SPDX-License-Identifier: MPL-2.0
//! Guard tests for folding line ranges and `grep` piped into `head` or `tail` into acus calls.

use super::*;

#[test]
fn sed_ranges_and_head_counts_piped_into_head_or_tail_fold_to_one_range() {
    for (cmd, want) in [
        (
            "sed -n 10,40p src/a.rs | head -5",
            "acus show 'src/a.rs:10-14'",
        ),
        (
            "sed -n 10,40p src/a.rs | head",
            "acus show 'src/a.rs:10-19'",
        ),
        (
            "sed -n 10,40p src/a.rs | head -50",
            "acus show 'src/a.rs:10-40'",
        ),
        (
            "sed -n 10,40p src/a.rs | tail -5",
            "acus show 'src/a.rs:36-40'",
        ),
        (
            "sed -n 10,40p src/a.rs | tail -50",
            "acus show 'src/a.rs:10-40'",
        ),
        ("sed -n 7p src/a.rs | head -5", "acus show 'src/a.rs:7'"),
        (
            "sed -n 10,40p src/a.rs 2>/dev/null | head -5",
            "acus show 'src/a.rs:10-14' 2>/dev/null",
        ),
        (
            "sed -n 10,40p src/a.rs 2>/dev/null | tail -5",
            "acus show 'src/a.rs:36-40' 2>/dev/null",
        ),
        (
            "sed -n '10,$p' src/a.rs | head -5",
            "acus show 'src/a.rs:10-14'",
        ),
        ("head -20 src/a.rs | head -5", "acus show 'src/a.rs:1-5'"),
        ("head -20 src/a.rs | head", "acus show 'src/a.rs:1-10'"),
        ("head -20 src/a.rs | head -50", "acus show 'src/a.rs:1-20'"),
        ("head -20 src/a.rs | tail -5", "acus show 'src/a.rs:16-20'"),
        ("head -5 src/a.rs | tail -20", "acus show 'src/a.rs:1-5'"),
        (
            "head -n 20 src/a.rs 2>/dev/null | tail -n 5",
            "acus show 'src/a.rs:16-20' 2>/dev/null",
        ),
        (
            "cat src/a.rs | head -20 | tail -5",
            "acus show 'src/a.rs:16-20'",
        ),
        (
            "cat src/a.rs | head -20 | head -5",
            "acus show 'src/a.rs:1-5'",
        ),
        (
            "cat src/a.rs 2>/dev/null | head -20 | tail -5",
            "acus show 'src/a.rs:16-20' 2>/dev/null",
        ),
    ] {
        assert_eq!(rw(cmd, None).as_deref(), Some(want), "{cmd}");
    }
}

#[test]
fn cat_piped_into_sed_ranges_folds_with_and_without_a_head_or_tail() {
    for (cmd, want) in [
        (
            "cat src/a.rs | sed -n 25,204p",
            "acus show 'src/a.rs:25-204'",
        ),
        (
            "cat src/a.rs | sed -n '25,204p'",
            "acus show 'src/a.rs:25-204'",
        ),
        ("cat src/a.rs | sed -n 7p", "acus show 'src/a.rs:7'"),
        ("cat src/a.rs | sed -n '25,$p'", "acus show 'src/a.rs:25-'"),
        (
            "cat src/a.rs | sed -n '25,204p' | head -10",
            "acus show 'src/a.rs:25-34'",
        ),
        (
            "cat src/a.rs | sed -n '25,204p' | tail -10",
            "acus show 'src/a.rs:195-204'",
        ),
        (
            "cat src/a.rs | sed -n '25,$p' | head -10",
            "acus show 'src/a.rs:25-34'",
        ),
        (
            "cat src/a.rs 2>/dev/null | sed -n '25,204p' | head -10",
            "acus show 'src/a.rs:25-34' 2>/dev/null",
        ),
    ] {
        assert_eq!(rw(cmd, None).as_deref(), Some(want), "{cmd}");
    }
}

#[test]
fn span_folds_refuse_zero_open_reversed_and_overflowing_counts() {
    // A window open to the end is a read of its own, and `tail` over it stays as a display filter.
    assert_eq!(
        rw("sed -n '10,$p' src/a.rs | tail -5", None).as_deref(),
        Some("acus show 'src/a.rs:10-' | tail -5")
    );
    for cmd in [
        "sed -n 10,40p src/a.rs | head -0",
        "sed -n 10,40p src/a.rs | tail -0",
        "sed -n 40,10p src/a.rs | head -5",
        "sed -n 10,40p src/a.rs | head -c 5",
        "sed -n 10,40p src/a.rs | head -n +3",
        "sed -n 10,40p src/a.rs | head -5 | grep x",
        "head -20 src/a.rs | head -5 | grep x",
        "cat src/a.rs | sed -n 25,204p | grep x",
        "sed -n 10,40p | head -5",
    ] {
        assert_eq!(rw(cmd, None), None, "{cmd}");
    }
}

#[test]
fn grep_piped_into_head_or_tail_becomes_find_with_its_limit() {
    // `--last` needs one regular file: the fixed scope has `src/a.rs` as one.
    let scope = Scope::fixed("/repo");
    for (cmd, want) in [
        (
            "grep -n \"I3\" AGENTS.md | head",
            "acus find 'I3' 'AGENTS.md' --max-hits 10",
        ),
        (
            "grep fn src/a.rs | head -5",
            "acus find 'fn' 'src/a.rs' --max-hits 5",
        ),
        (
            "grep fn src/a.rs | tail -5",
            "acus find 'fn' 'src/a.rs' --last 5",
        ),
        (
            "grep fn src/a.rs | tail",
            "acus find 'fn' 'src/a.rs' --last 10",
        ),
        (
            "grep fn src/a.rs src/b.rs | head -5",
            "acus find 'fn' 'src/a.rs' 'src/b.rs' --max-hits 5",
        ),
        (
            "grep \"a b\" src/a.rs | head -5",
            "acus find 'a b' 'src/a.rs' --max-hits 5",
        ),
        (
            "grep -o fn src/a.rs | head -5",
            "acus find 'fn' 'src/a.rs' -o | head -5",
        ),
        (
            "grep fn src/a.rs 2>/dev/null | head -5",
            "acus find 'fn' 'src/a.rs' --max-hits 5 2>/dev/null",
        ),
        (
            "grep fn src/a.rs 2>/dev/null | tail -5",
            "acus find 'fn' 'src/a.rs' --last 5 2>/dev/null",
        ),
    ] {
        assert_eq!(rw(cmd, Some(&scope)).as_deref(), Some(want), "{cmd}");
    }
}

#[test]
fn grep_limits_without_an_exact_form_are_not_folded() {
    // No exact limit: the filter stays after the acus call as a display filter, or is refused.
    for (cmd, want) in [
        (
            "grep -C 2 fn src/a.rs | head -5",
            Some("acus find 'fn' 'src/a.rs' -C 2 | head -5"),
        ),
        (
            "grep -A 2 fn src/a.rs | tail -5",
            Some("acus find 'fn' 'src/a.rs' -A 2 | tail -5"),
        ),
        (
            "grep fn src/a.rs src/b.rs | tail -5",
            Some("acus find 'fn' 'src/a.rs' 'src/b.rs' | tail -5"),
        ),
        (
            "grep -l fn src/a.rs | head -5",
            Some("acus find 'fn' 'src/a.rs' -l | head -5"),
        ),
        ("grep -c fn src/a.rs | head -5", None),
        ("grep fn src/a.rs | head -5 | grep x", None),
        ("grep fn src/a.rs | head -0", None),
        // `-o` prints one line per match, so the line count of `tail` is no limit of its.
        (
            "grep -o fn src/a.rs | tail -5",
            Some("acus find 'fn' 'src/a.rs' -o | tail -5"),
        ),
    ] {
        assert_eq!(rw(cmd, None).as_deref(), want, "{cmd}");
    }
}

#[test]
fn grep_tail_gets_last_only_for_one_regular_file() {
    let scope = Scope::fixed("/repo");
    assert_eq!(
        rw("grep fn src/a.rs | tail -5", Some(&scope)).as_deref(),
        Some("acus find 'fn' 'src/a.rs' --last 5")
    );
    // `--last` counts the lines of each file: a directory, a recursive search, several files or
    // no file at all keep the `tail` as it is.
    for cmd in [
        "grep -rn foo src | tail -3",
        "grep -r foo | tail -3",
        "grep foo src/a.rs src/b.rs | tail -3",
    ] {
        assert!(
            rw(cmd, Some(&scope)).is_none_or(|c| !c.contains("--last")),
            "{cmd}"
        );
    }
}

// A drive path is a path: its `:` belongs to the drive, and a range follows the last `:`. Pure
// strings, so every OS runs them.
#[test]
fn drive_paths_fold_like_unix_paths() {
    for (cmd, want) in [
        (
            r"head -20 'C:\work\a.rs'",
            Some(r"acus show 'C:\work\a.rs:1-20'"),
        ),
        (
            "head -20 C:/work/a.rs",
            Some("acus show 'C:/work/a.rs:1-20'"),
        ),
        ("tail -5 C:/work/a.rs", Some("acus show 'C:/work/a.rs:-5'")),
        (
            "sed -n 2,4p C:/work/a.rs",
            Some("acus show 'C:/work/a.rs:2-4'"),
        ),
        ("cat C:/work/a.rs", Some("acus show 'C:/work/a.rs'")),
        (r"cat 'C:\work\a.rs'", Some(r"acus show 'C:\work\a.rs'")),
        // Unquoted, bash reads `\w` as `w`: that is not the file the command names, so no rewrite.
        (r"head -20 C:\work\a.rs", None),
    ] {
        assert_eq!(rw(cmd, None).as_deref(), want, "{cmd}");
    }
}

// Absolute temp paths: a Windows drive path carries a `:` that reads as an address.
#[cfg(unix)]
#[test]
fn folds_of_a_window_stop_at_the_last_line_of_the_file() {
    let dir = std::env::temp_dir().join(format!("acus-folds-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let ten = dir.join("ten.rs");
    let three = dir.join("three.rs");
    std::fs::write(&ten, "1\n2\n3\n4\n5\n6\n7\n8\n9\n10\n").unwrap();
    // A last line without its newline is a line too.
    std::fs::write(&three, "a\nb\nc").unwrap();
    let (ten, three) = (ten.display().to_string(), three.display().to_string());
    // Quoted, so a Windows path keeps its backslashes in the shell command.
    let (qten, qthree) = (format!("'{ten}'"), format!("'{three}'"));
    let cases = [
        (
            format!("head -20 {qten} | tail -5"),
            Some(format!("acus show '{ten}:6-10'")),
        ),
        (
            format!("sed -n 2,20p {qten} | tail -3"),
            Some(format!("acus show '{ten}:8-10'")),
        ),
        (
            format!("cat {qten} | sed -n 2,20p | tail -3"),
            Some(format!("acus show '{ten}:8-10'")),
        ),
        (
            format!("sed -n 9,20p {qten} | head -5"),
            Some(format!("acus show '{ten}:9-10'")),
        ),
        (
            format!("head -20 {qthree} | head -5"),
            Some(format!("acus show '{three}:1-3'")),
        ),
        (
            format!("head -20 {qthree} | tail -5"),
            Some(format!("acus show '{three}:1-3'")),
        ),
        // A window that starts past the last line has no lines: it is not folded.
        (format!("sed -n 11,20p {qten} | head -5"), None),
        (format!("sed -n 11,20p {qten} | tail -3"), None),
        // `tail -n +N | head -M` stops at the last line too, and a start past it is not folded.
        (
            format!("tail -n +8 {qten} | head -5"),
            Some(format!("acus show '{ten}:8-10'")),
        ),
        (
            format!("cat {qten} | tail -n +8 | head -5"),
            Some(format!("acus show '{ten}:8-10'")),
        ),
        // A count past usize is no window; a window of huge length reads to the last line.
        (
            format!("head -18446744073709551615 {qten} | head -18446744073709551615"),
            None,
        ),
        (
            format!("sed -n 1,18446744073709551615p {qten} | tail -2"),
            Some(format!("acus show '{ten}:9-10'")),
        ),
        (format!("tail -n +11 {qten} | head -5"), None),
    ];
    for (cmd, want) in cases {
        assert_eq!(rw(&cmd, None), want, "{cmd}");
    }
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn grep_word_and_end_anchors_in_quotes_fold_and_unquoted_specials_do_not() {
    assert_eq!(
        rw(r"grep -n '\<foo\>' src/a.rs", None).as_deref(),
        Some(r"acus find '\bfoo\b' 'src/a.rs'")
    );
    assert_eq!(
        rw("grep -n 'foo$' src/a.rs", None).as_deref(),
        Some("acus find 'foo$' 'src/a.rs'")
    );
    for cmd in [
        "grep foo $F",
        "grep foo src/a.rs > out",
        r#"grep "$X" src/a.rs"#,
    ] {
        assert!(rw(cmd, None).is_none(), "{cmd}");
    }
}

#[test]
fn grep_head_refusal_names_the_limit_it_would_fold_to() {
    let (rule, reason) = deny(check_in("grep -C 2 fn src/a.rs | head -5", false, None));
    assert_eq!(rule, Rule::Grep.as_str());
    assert!(
        reason.contains("Replace it with `acus find 'fn' 'src/a.rs' -C 2 --max-hits 5`"),
        "{reason}"
    );
    let (_, reason) = deny(check_in("grep -l fn src/a.rs | head -5", false, None));
    assert!(
        reason.contains("Replace it with `acus find 'fn' 'src/a.rs' -l`"),
        "{reason}"
    );
}

#[test]
fn read_piped_into_grep_folds_to_one_find_over_the_range() {
    for (cmd, want) in [
        (
            "sed -n 10,20p src/a.rs | grep x",
            "acus find 'x' 'src/a.rs:10-20'",
        ),
        ("cat src/a.rs | grep x", "acus find 'x' 'src/a.rs'"),
        (
            "head -20 AGENTS.md | grep x",
            "acus find 'x' 'AGENTS.md:1-20'",
        ),
        ("tail -5 AGENTS.md | grep x", "acus find 'x' 'AGENTS.md:-5'"),
        (
            "sed -n '30,$p' AGENTS.md | grep x",
            "acus find 'x' 'AGENTS.md:30-'",
        ),
        (
            "cat src/a.rs | grep x | head -3",
            "acus find 'x' 'src/a.rs' --max-hits 3",
        ),
    ] {
        assert_eq!(rw(cmd, None).as_deref(), Some(want), "{cmd}");
    }
    // A read of one file that greps it passes when it is a plain display of a manifest.
    assert!(check("cat Cargo.toml | grep version").is_none());
    // More than one file is not one range: it stays refused.
    assert!(check("sed -n 1,5p AGENTS.md crates/acus-cli/src/main.rs | grep x").is_some());
}
