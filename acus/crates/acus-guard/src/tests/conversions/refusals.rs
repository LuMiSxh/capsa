// SPDX-License-Identifier: MPL-2.0

use super::*;

#[test]
fn cat_head_and_tail_keep_a_stderr_redirect_and_name_the_range_when_refused() {
    for (cmd, want) in [
        (
            "cat a.rs 2>/dev/null | head -20",
            "acus show 'a.rs:1-20' 2>/dev/null",
        ),
        (
            "cat a.rs 2> /dev/null | head -20",
            "acus show 'a.rs:1-20' 2>/dev/null",
        ),
        ("cat a.rs 2>&1 | tail -5", "acus show 'a.rs:-5' 2>&1"),
        (
            "cat a.rs 2>/dev/null | tail -n 5",
            "acus show 'a.rs:-5' 2>/dev/null",
        ),
    ] {
        let r = rewrite(cmd, None).unwrap_or_else(|| panic!("not converted: {cmd}"));
        assert_eq!(r.command, want, "{cmd}");
    }
    // A redirect into a file is not a stderr one: cat feeds nothing to the head.
    assert!(rewrite("cat a.rs > x | head -3", None).is_none());
    // `tail -n +N` reads from line N to the end: the open-ended `acus show` range says so.
    for (cmd, want) in [
        ("cat a.rs | tail -n +3", "acus show 'a.rs:3-'"),
        (
            "cat a.rs 2>/dev/null | tail -n +3",
            "acus show 'a.rs:3-' 2>/dev/null",
        ),
        ("cat a.rs | tail -n+3", "acus show 'a.rs:3-'"),
        ("cat a.rs | tail +3", "acus show 'a.rs:3-'"),
        ("tail -n +3 a.rs", "acus show 'a.rs:3-'"),
        ("tail --lines=+3 a.rs", "acus show 'a.rs:3-'"),
        ("tail +3 a.rs", "acus show 'a.rs:3-'"),
        ("sed -n '3,$p' a.rs", "acus show 'a.rs:3-'"),
        ("sed -n \"3,\\$p\" a.rs", "acus show 'a.rs:3-'"),
    ] {
        let r = rewrite(cmd, None).unwrap_or_else(|| panic!("not converted: {cmd}"));
        assert_eq!(r.command, want, "{cmd}");
    }
    // `head -n +N` is no line range, and `+0` starts at no line: both are refused, not rewritten.
    for cmd in [
        "cat a.rs | head -n +3",
        "head -n +3 a.rs",
        "tail -n +0 a.rs",
        "cat a.rs | tail -n +0",
    ] {
        assert!(rewrite(cmd, None).is_none(), "{cmd}");
    }
    // Refused because of the sed, which has no rewrite; the cat and head pair is rewritten in the
    // send line.
    let Some(Verdict::Deny(_, reason)) =
        check("cat a.rs 2>/dev/null | head -20; sed -i 's/a/b/' x.json")
    else {
        panic!("not refused");
    };
    assert!(
        reason.contains("because of `sed -i 's/a/b/' x.json`"),
        "{reason}"
    );
    assert!(
        reason.contains("Send instead: acus show 'a.rs:1-20' 2>/dev/null; sed -i 's/a/b/' x.json"),
        "{reason}"
    );
    // `head -c` and `tail -c` have no line range, so the reason names the segment, not the file.
    for cmd in [
        "cat a.rs 2>/dev/null | head -c 20; sed -i 's/a/b/' x.json",
        "cat a.rs 2>/dev/null | tail -c +3; sed -i 's/a/b/' x.json",
    ] {
        let Some(Verdict::Deny(_, reason)) = check(cmd) else {
            panic!("not refused: {cmd}");
        };
        assert!(!reason.contains("Send instead"), "{reason}");
        assert!(!reason.contains("acus show 'a.rs'"), "{reason}");
    }
    // A filter after the head is no pair with the cat's range: the refused sed is what is named.
    let Some(Verdict::Deny(_, reason)) =
        check("cat a.rs | head -3 | grep x; sed -i 's/a/b/' x.json")
    else {
        panic!("not refused");
    };
    assert!(!reason.contains("acus show 'a.rs'"), "{reason}");
}

#[test]
fn leaves_shell_expanded_words_and_time_options_unconverted() {
    for cmd in [
        "cargo test | grep FAIL*",
        "cargo test | grep -e FAIL?",
        "cargo test | grep [ab]",
        "cargo test | grep {a,b}",
        "cargo test | grep ~/x",
        "grep FAIL* src/a.rs",
        "time -p cargo test | tail -3",
        "time -p cargo test | grep x",
    ] {
        assert!(rewrite(cmd, None).is_none(), "{cmd}");
    }
    use crate::{plain_text, plain_word};
    // Not converted, so a build pipe is refused as before.
    assert!(matches!(
        check("cargo test | grep FAIL*"),
        Some(Verdict::Deny(Rule::BuildPipe, _))
    ));
    assert!(!plain_word("FAIL*") && plain_word("'FAIL*'") && plain_word("\"FAIL*\""));
    assert!(!plain_word("~/x") && plain_word("--include=a") && plain_text("*.rs"));
}

#[test]
fn gnu_bre_escapes_convert_to_rust_regex() {
    for (cmd, want) in [
        // Real calls that were refused or unrewritten: `\b`, `\s`, `\|` and `\+` have Rust forms.
        (
            "grep -n 'R1\\b\\|R12' AGENTS.md",
            "acus find 'R1\\b|R12' 'AGENTS.md'",
        ),
        (
            "grep -n \"^## \\|^### \" AGENTS.md",
            "acus find '^## |^### ' 'AGENTS.md'",
        ),
        (
            "grep -n '11\\.1[0-9]' AGENTS.md",
            "acus find '11\\.1[0-9]' 'AGENTS.md'",
        ),
        (
            "grep -n 'a\\s\\+b' AGENTS.md",
            "acus find 'a\\s+b' 'AGENTS.md'",
        ),
        ("grep -E '\\bfoo' src/a.rs", "acus find '\\bfoo' 'src/a.rs'"),
        // A single quote in the pattern stays quoted for the shell: `\\` is `\` inside `"…"`.
        (
            "grep -n \"it's\\b\" AGENTS.md",
            "acus find \"it's\\\\b\" 'AGENTS.md'",
        ),
    ] {
        assert_eq!(rw(cmd, None).as_deref(), Some(want), "{cmd}");
    }
}

#[test]
fn leaves_what_it_cannot_convert_exactly_refused() {
    for cmd in [
        // Syntax that means something else for the Rust regex.
        "grep 'x\\<' src/a.rs",
        "grep -E 'x\\<' src/a.rs",
        "grep 'a^b' src/a.rs",
        // A repetition that can match nothing: `\<` and `\>` would no longer mean the same.
        "grep '\\<a*-' src/a.rs",
        "head -5 src/a.rs | grep '\\<a*-'",
        "grep 'a\\>*' src/a.rs",
        "grep '[a\\]b]' src/a.rs",
        // Context together with modes that print no lines.
        "grep -l -C1 foo src/a.rs",
        "grep -o -B1 foo src/a.rs",
        "grep -m1 -A1 foo src/a.rs",
        "grep -A x foo src/a.rs",
        "grep -A '1;touch pwned' foo src/a.rs",
        "grep --include=src/*.rs foo src",
        // Braces are literal for grep, so `-g` would translate a different glob.
        "grep --include='*.{rs,md}' foo src",
        "grep -rn --exclude='{a,b}' x src",
        // The shell would expand or change these words.
        "grep \"a\\$b\" src/a.rs",
        "grep a\\|b src/a.rs",
        "cargo test | grep \"a\\$b\"",
        "cargo test | grep -A3 -v ok",
        "cargo test | head -5 | grep x",
        "cargo test 2>/dev/null | tail -3",
        "cargo test | grep 'a'\\''b'",
    ] {
        assert!(rewrite(cmd, None).is_none(), "{cmd}");
        assert!(matches!(check(cmd), Some(Verdict::Deny(..))), "{cmd}");
    }
}

#[test]
fn converts_several_paths_name_filters_and_globs_as_the_shell_expands_them() {
    for (cmd, want) in [
        (
            "grep -rn \"A\\|B\" src tests",
            "acus find 'A|B' 'src' 'tests'",
        ),
        (
            "grep -rn x --include='*.rs' src",
            "acus find 'x' 'src' -g '*.rs'",
        ),
        (
            "grep -rn x src --exclude-dir=target",
            "acus find 'x' 'src' -g '!target'",
        ),
        ("grep -n x src/a.rs -i", "acus find 'x' 'src/a.rs' -i"),
        ("grep -rli x src", "acus find 'x' 'src' -l -i"),
        ("grep -rln x src", "acus find 'x' 'src' -l"),
        ("grep -n x src/*.rs", "acus find 'x' src/*.rs"),
        ("grep -rn x src/*", "acus find 'x' src/*"),
        ("grep -n x *.md -i", "acus find 'x' *.md -i"),
        ("cat src/*.rs", "acus show src/*.rs"),
        ("cat src/{a,b}.rs", "acus show src/{a,b}.rs"),
    ] {
        assert_eq!(rw(cmd, None).as_deref(), Some(want), "{cmd}");
    }
    // A glob that is the pattern, that a flag takes as its value, or that may name a directory
    // for a grep without -r, and a cat of non-code files: the expanded words would differ.
    for cmd in [
        "grep -n *.rs src",
        "grep -n x src/*",
        "grep -rn x src --include *.rs",
        "grep -n x src/*.rs -A *.md",
        "cat -n src/*.rs",
        "cat src/*.txt",
        "cat src/{a,b}.txt",
        "head -n 3 src/*.rs",
        "cat src/*.rs | head -5",
    ] {
        assert!(rw(cmd, None).is_none(), "{cmd}");
    }
}

#[test]
fn new_conversions_never_pass_user_text_unquoted() {
    for (cmd, want) in [
        (
            "grep --include='*.rs;touch pwned' foo src",
            "acus find 'foo' 'src' -g '*.rs;touch pwned'",
        ),
        (
            "grep --include=\"a'b;touch x\" foo src",
            "acus find 'foo' 'src' -g \"a'b;touch x\"",
        ),
        (
            "grep \"a'; touch pwned; '\" src/a.rs",
            "acus find \"a'; touch pwned; '\" 'src/a.rs'",
        ),
        (
            "cargo test \"a;touch pwned\" | tail -3",
            "acus ctx 'cargo test \"a;touch pwned\"' --tail 3",
        ),
        (
            "cargo test 'x;touch pwned' | tail -3",
            "acus ctx \"cargo test 'x;touch pwned'\" --tail 3",
        ),
        (
            "cargo test | grep \"a;touch pwned\" -A 1",
            "acus ctx 'cargo test' --grep 'a;touch pwned' -A 1",
        ),
    ] {
        assert_eq!(rw(cmd, None).as_deref(), Some(want), "{cmd}");
    }
    for cmd in [
        "cargo test $(touch pwned) | tail -3",
        "cargo test `touch pwned` | tail -3",
        "cat a.rs | head -$(touch pwned)",
        "grep -A $(touch pwned) foo src/a.rs",
        "grep --include=$(touch pwned) foo src",
    ] {
        assert!(rw(cmd, None).is_none(), "{cmd}");
    }
}

#[test]
fn grep_without_r_on_a_directory_is_not_rewritten() {
    // grep prints "Is a directory" and searches nothing for a directory operand without -r; acus
    // find recurses, so such a call is refused without a replacement.
    let dir = tempfile::tempdir().unwrap();
    for d in ["crates", "tests", "src"] {
        std::fs::create_dir(dir.path().join(d)).unwrap();
    }
    std::fs::write(dir.path().join("src/a.rs"), "fn a() {}\n").unwrap();
    let scope = Scope::new(dir.path().to_str());
    assert_eq!(
        rw("grep -n foo src/a.rs", scope.as_ref()).as_deref(),
        Some("acus find 'foo' 'src/a.rs'")
    );
    assert_eq!(
        rw("grep -rn foo crates", scope.as_ref()).as_deref(),
        Some("acus find 'foo' 'crates'")
    );
    // A directory is what the disk says grep cannot search. A missing path says nothing, since
    // the scope does not follow a `cd` earlier in the command.
    assert_eq!(
        rw("grep -n foo missing.rs", scope.as_ref()).as_deref(),
        Some("acus find 'foo' 'missing.rs'")
    );
    for cmd in [
        "grep -n foo crates",
        "grep -n foo crates tests",
        "grep -n foo crates src/a.rs",
    ] {
        assert!(rw(cmd, scope.as_ref()).is_none(), "{cmd}");
        assert!(
            matches!(
                check_in(cmd, false, scope.as_ref()),
                Some(Verdict::Deny(Rule::Grep, _))
            ),
            "{cmd}"
        );
    }
    // A fixed scope cannot read the disk, so its operands keep the rewrite they had.
    let fixed = Scope::fixed("/repo");
    assert!(rewrite("grep -n foo justfile", Some(&fixed)).is_some());
    assert!(rewrite("grep -n foo /tmp/x.rs", Some(&fixed)).is_some());
}

#[test]
fn cat_of_several_files_is_rewritten_unless_one_is_a_directory() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.md"), "# a\n").unwrap();
    std::fs::write(dir.path().join("b.md"), "# b\n").unwrap();
    std::fs::create_dir(dir.path().join("d.md")).unwrap();
    let scope = Scope::new(dir.path().to_str());
    assert_eq!(
        rw("cat a.md b.md", scope.as_ref()).as_deref(),
        Some("acus show 'a.md' 'b.md'")
    );
    // A missing path says nothing, since the scope does not follow a `cd` earlier in the command.
    assert_eq!(
        rw("cat a.md missing.md", scope.as_ref()).as_deref(),
        Some("acus show 'a.md' 'missing.md'")
    );
    for cmd in ["cat a.md d.md", "cat d.md a.md"] {
        assert!(rw(cmd, scope.as_ref()).is_none(), "{cmd}");
        assert!(
            matches!(
                check_in(cmd, false, scope.as_ref()),
                Some(Verdict::Deny(Rule::ReadShell, _))
            ),
            "{cmd}"
        );
    }
}
