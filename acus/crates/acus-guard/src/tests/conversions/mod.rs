// SPDX-License-Identifier: MPL-2.0

//! Guard tests for conversions of reads, searches and filters into exact acus calls.

use super::*;

mod pipes;
mod quiet_patch;
mod refusals;

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "one table of conversions; splitting it scatters the cases"
)]
fn converts_reads_searches_and_filters_exactly() {
    for (cmd, want) in [
        ("cat a.rs | head -5", "acus show 'a.rs:1-5'"),
        (
            "FOO=1 cargo test | tail -3",
            "acus ctx 'FOO=1 cargo test' --tail 3",
        ),
        (
            "cargo test | grep 'FAIL*'",
            "acus ctx 'cargo test' --grep 'FAIL*'",
        ),
        (
            "cargo test | grep \"FAIL*\"",
            "acus ctx 'cargo test' --grep 'FAIL*'",
        ),
        ("cat a.rs | head", "acus show 'a.rs:1-10'"),
        ("cat a.rs | head -n 20", "acus show 'a.rs:1-20'"),
        ("cat a.rs | head -n20", "acus show 'a.rs:1-20'"),
        ("rg foo src", "acus find 'foo' 'src'"),
        ("grep -n 'a\\|b' src/a.rs", "acus find 'a|b' 'src/a.rs'"),
        ("grep 'a|b' src/a.rs", "acus find 'a\\|b' 'src/a.rs'"),
        ("grep -E 'a|b' src/a.rs", "acus find 'a|b' 'src/a.rs'"),
        ("egrep 'f(x)' src/a.rs", "acus find 'f(x)' 'src/a.rs'"),
        ("fgrep 'a.b' src/a.rs", "acus find 'a.b' 'src/a.rs' -F"),
        ("grep -e -x src/a.rs", "acus find -e '-x' 'src/a.rs'"),
        (
            "grep --include='*.rs' -rn foo src",
            "acus find 'foo' 'src' -g '*.rs'",
        ),
        (
            "grep --include=*.rs foo src",
            "acus find 'foo' 'src' -g '*.rs'",
        ),
        (
            "grep -rn --exclude-dir=target --exclude=*.lock foo src",
            "acus find 'foo' 'src' -g '!target' -g '!*.lock'",
        ),
        (
            "grep -n -C 3 foo src/a.rs",
            "acus find 'foo' 'src/a.rs' -C 3",
        ),
        (
            "grep -A2 -B1 foo src/a.rs",
            "acus find 'foo' 'src/a.rs' -A 2 -B 1",
        ),
        ("grep -3 foo src/a.rs", "acus find 'foo' 'src/a.rs' -C 3"),
        (
            "grep --after-context=2 foo src/a.rs",
            "acus find 'foo' 'src/a.rs' -A 2",
        ),
        (
            "grep -v -A1 foo src/a.rs",
            "acus find 'foo' 'src/a.rs' -v -A 1",
        ),
        (
            "CARGO_TARGET_DIR=/tmp/t cargo test 2>&1 | tail -5",
            "acus ctx 'CARGO_TARGET_DIR=/tmp/t cargo test' --tail 5",
        ),
        (
            "time cargo test | tail -5",
            "time acus ctx 'cargo test' --tail 5",
        ),
        (
            "cargo test --test 'a b' | tail -3",
            "acus ctx \"cargo test --test 'a b'\" --tail 3",
        ),
        (
            "cargo test | grep -E 'FAIL|panic' -A 2",
            "acus ctx 'cargo test' --grep 'FAIL|panic' -A 2",
        ),
        (
            "cargo test | grep -A3 FAILED",
            "acus ctx 'cargo test' --grep 'FAILED' -A 3",
        ),
        (
            "cargo test | grep -C2 -i x",
            "acus ctx 'cargo test' --grep '(?i)x' -C 2",
        ),
        (
            "cargo test | grep 'a\\|b'",
            "acus ctx 'cargo test' --grep 'a|b'",
        ),
        (
            "cd X && cargo build 2>&1 | grep -E '^(error|warning)' -A4",
            "cd X && acus ctx 'cargo build' --grep '^(error|warning)' -A 4",
        ),
        (
            "swift build 2>&1 | grep -E \"error\" | head -20",
            "acus ctx 'swift build' --grep 'error' --max-output 20",
        ),
        (
            "cargo test | grep FAIL | tail -5",
            "acus ctx 'cargo test' --grep 'FAIL' --tail 5",
        ),
        (
            "grep -n \"a\\.b\\|c\" src/a.rs",
            "acus find 'a\\.b|c' 'src/a.rs'",
        ),
    ] {
        assert_eq!(rw(cmd, None).as_deref(), Some(want), "{cmd}");
    }
    // The same call without the rewrite (Codex) is still refused.
    assert!(matches!(
        check("cat a.rs | head -5"),
        Some(Verdict::Deny(Rule::ReadShell, _))
    ));
    assert!(
        rewrite("cargo test | grep -A3 FAILED", None)
            .unwrap()
            .deferred
    );
}

#[test]
fn converts_name_listings_to_acus_files() {
    for (cmd, want) in [
        ("find src -name '*.rs'", "acus files 'src' -g '*.rs'"),
        ("find . -type f -name '*.rs'", "acus files '.' -g '*.rs'"),
        (
            "find src -name \"*.md\" -type f",
            "acus files 'src' -g '*.md'",
        ),
        ("find -name '*.rs'", "acus files -g '*.rs'"),
        (
            "find src tests -name Cargo.toml",
            "acus files 'src' 'tests' -g '**/Cargo.toml'",
        ),
        ("find src -iname '*.RS'", "acus files 'src' -g '*.[rR][sS]'"),
        (
            "find src -iname README.md",
            "acus files 'src' -g '**/[rR][eE][aA][dD][mM][eE].[mM][dD]'",
        ),
        (
            "find . -name '*.rs' 2>/dev/null",
            "acus files '.' -g '*.rs' 2>/dev/null",
        ),
        (
            "cd x && find . -name '*.md'; echo hi",
            "cd x && acus files '.' -g '*.md'; echo hi",
        ),
        ("rg --files", "acus files"),
        (
            "rg --files src -g '!target'",
            "acus files 'src' -g '!target'",
        ),
        (
            "rg --files --glob='!target' --hidden",
            "acus files -g '!target' --hidden",
        ),
        ("rg --files -u -L src", "acus files 'src' -u -L"),
    ] {
        let r = rewrite(cmd, None).unwrap_or_else(|| panic!("not converted: {cmd}"));
        assert_eq!(r.command, want, "{cmd}");
        assert!(!r.deferred, "{cmd}");
    }
    // Not the same listing, or not exactly: left as it is (find) or refused (rg).
    for cmd in [
        "find . -name '*.rs' -exec wc -l {} +",
        "find . -name '*.rs' -delete",
        "find . -name '*.rs' -mtime -1",
        "find . -name '*.rs' -size +1M",
        "find . -maxdepth 2 -name '*.rs'",
        "find . -name '*.rs' -o -name '*.md'",
        "find . ! -name '*.rs'",
        "find . -type d -name src",
        "find . -type l -name '*.rs'",
        "find . -name 'test_*'",
        "find . -name 'a*b'",
        "find . -name 'file?.rs'",
        "find . -name '{a,b}.rs'",
        "find . -name '[ab].rs'",
        "find . -name 'src/*.rs'",
        "find . -name '*.rs' -name '*.md'",
        "find . -name *.rs",
        "find . -name",
        "find . -iname 'é*'",
        "find '-x' -name '*.rs'",
        "find . -name '*.rs' | wc -l",
        "find . -name '*.rs' | xargs wc -l",
        "find . -name '*.rs' > files.txt",
        "git ls-files",
        "git ls-files '*.rs'",
        "fd '\\.rs$' src",
        "fd -e rs",
        "fd -g '*.rs'",
    ] {
        assert!(rewrite(cmd, None).is_none(), "{cmd}");
        assert!(!matches!(check(cmd), Some(Verdict::Deny(..))), "{cmd}");
    }
    for cmd in [
        "rg --files --sort path",
        "rg --files -g 'src/*.rs'",
        "rg --files -g 'test_*'",
        "rg --files -g '*.rs' -g '!a/b'",
        "rg --files -uu",
        "rg --files -0",
        "rg --files -T rs",
        // Includes override ignore and hidden rules; `-t` uses ripgrep's own type table.
        "rg --files src -g '*.rs'",
        "rg -g '*.log' --files",
        "rg --files --glob='*.rs'",
        "rg --files -t rs",
        "rg --files -t c",
        "rg --files --type=sh",
        "rg --files -trs",
        "rg --files 'a b' -g '[ab]'",
    ] {
        assert!(rewrite(cmd, None).is_none(), "{cmd}");
        assert!(
            matches!(check(cmd), Some(Verdict::Deny(Rule::Grep, _))),
            "{cmd}"
        );
    }
    // `fd` has regex semantics, so it gets a hint that names `acus files`; piped, nothing.
    for cmd in ["fd '\\.rs$' src", "fd -e rs", "fdfind -g '*.rs'"] {
        let Some(Verdict::Hint(Rule::GlobTool, h)) = check(cmd) else {
            panic!("{cmd}")
        };
        assert!(h.contains("acus files [DIR] -g 'GLOB'"), "{h}");
    }
    assert!(check("fd rs | wc -l").is_none());
    // The Glob tool cannot become a Bash call, so its hint names the replacement.
    for hint in [GLOB_HINT, TREE_HINT] {
        assert!(hint.contains("`acus files [DIR] -g 'GLOB'`"), "{hint}");
    }
    // A rewritten listing is a conversion of a call the guard let through before.
    assert_eq!(
        rewrite("find src -name '*.rs'", None).unwrap().rule,
        Rule::GlobTool
    );
}

#[test]
fn keeps_listings_whose_exit_status_gates_the_next_command() {
    // `find` exits 0 on no match and `acus files` exits 1: `&&` and `||` would change meaning.
    for cmd in [
        "find . -name '*.rs' && echo found",
        "find . -name '*.rs' || echo none",
        "find . -name '*.rs' 2>/dev/null && echo found",
        "cd x && find . -name '*.rs' || echo none",
    ] {
        assert!(rewrite(cmd, None).is_none(), "{cmd}");
        assert!(!matches!(check(cmd), Some(Verdict::Deny(..))), "{cmd}");
    }
    for cmd in ["rg --files && echo found", "rg --files src || echo none"] {
        assert!(rewrite(cmd, None).is_none(), "{cmd}");
        let (_, reason) = deny(check(cmd));
        assert!(!reason.contains("acus files"), "{cmd}: {reason}");
    }
    // Before a `;`, after `&&`, or as the last stage, the status gates nothing.
    for (cmd, want) in [
        (
            "find . -name '*.rs'; echo done",
            "acus files '.' -g '*.rs'; echo done",
        ),
        (
            "echo a && find . -name '*.rs'",
            "echo a && acus files '.' -g '*.rs'",
        ),
        ("rg --files; echo done", "acus files; echo done"),
    ] {
        assert_eq!(rewrite(cmd, None).unwrap().command, want, "{cmd}");
    }
}

#[test]
fn keeps_escaped_spaces_inside_their_word() {
    assert_eq!(
        split_words("grep -c . ~/My\\ Dir/a.rs"),
        ["grep", "-c", ".", "~/My\\ Dir/a.rs"]
    );
    for (cmd, want) in [
        (
            "grep -l foo src/my\\ dir/a.rs",
            "acus find 'foo' 'src/my dir/a.rs' -l",
        ),
        ("cat src/my\\ dir/a.rs", "acus show 'src/my dir/a.rs'"),
        (
            "cd x && tail -n 3 a\\ b.rs; echo hi",
            "cd x && acus show 'a b.rs:-3'; echo hi",
        ),
    ] {
        assert_eq!(rw(cmd, None).as_deref(), Some(want), "{cmd}");
    }
    // A leading `~` is expanded to the home directory, as the shell would.
    let home = std::env::var("HOME").unwrap_or_default();
    let home = if cfg!(windows) {
        home.replace('\x5c', "/")
    } else {
        home
    };
    if !home.is_empty() && !home.contains(' ') {
        assert_eq!(
            rw("grep -l foo ~/My\\ Dir/a.rs", None).as_deref(),
            Some(format!("acus find 'foo' '{home}/My Dir/a.rs' -l").as_str())
        );
    }
    // Other escapes and wildcards in the same word: not converted.
    for cmd in [
        "cat src/my\\ dir/*.rs",
        "cat src/my\\ \\$dir/a.rs",
        "cat src/my\\ dir/a\\'b.rs",
    ] {
        assert!(rw(cmd, None).is_none(), "{cmd}");
    }
    // The path is one operand, so a plain file is still left alone.
    assert_eq!(
        check("grep -c . ~/Library/Application\\ Support/acus/guard.jsonl"),
        None
    );
    // The refusal names the whole path and keeps the flag.
    let (_, r) = deny(check("grep -l foo src/my\\ dir/a.rs; echo hi"));
    assert!(r.contains("`acus find 'foo' 'src/my dir/a.rs' -l`"), "{r}");
}

#[test]
fn converts_git_diff_into_acus_diff() {
    for (cmd, want) in [
        ("git diff", "acus diff -p"),
        ("git diff --stat", "acus diff"),
        ("git diff --stat HEAD~1", "acus diff HEAD~1"),
        ("git diff -p HEAD", "acus diff -p HEAD"),
        (
            "git diff HEAD~2..HEAD -- src/lib.rs",
            "acus diff -p HEAD~2..HEAD src/lib.rs",
        ),
        ("git diff -- README.md", "acus diff -p HEAD README.md"),
        (
            "git diff --staged src/a.rs",
            "acus diff --staged -p src/a.rs",
        ),
        (
            "git diff --cached -- src/a.rs",
            "acus diff --staged -p HEAD src/a.rs",
        ),
    ] {
        let r = rewrite(cmd, None).unwrap_or_else(|| panic!("not rewritten: {cmd}"));
        assert_eq!(r.command, want, "{cmd}");
        assert_eq!(r.rule, Rule::GitDiff, "{cmd}");
    }
    // Shapes acus diff does not take, a status that gates the next command, a piped diff and
    // `git -C` keep the hint.
    for cmd in [
        "git diff main...feature",
        "git diff -U0",
        "git diff --word-diff",
        "git diff main HEAD",
        "git diff --stat -p",
        "git diff && echo ok",
        "git diff | head -5",
        "git -C ../x diff",
    ] {
        assert!(rewrite(cmd, None).is_none(), "{cmd}");
        assert!(
            matches!(check(cmd), Some(Verdict::Hint(Rule::GitDiff, _))),
            "{cmd}"
        );
    }
}

#[test]
fn folds_the_status_echo_after_a_pipeline_into_dollar_question() {
    for (cmd, want) in [
        (
            "cargo test | tail -5; echo \"exit=${PIPESTATUS[0]}\"",
            "acus ctx 'cargo test' --tail 5; echo \"exit=$?\"",
        ),
        (
            "cargo test 2>&1 | tail -n 5; echo \"exit ${pipestatus[1]}\"",
            "acus ctx 'cargo test' --tail 5; echo \"exit $?\"",
        ),
        (
            "cargo test | grep -E 'ok|FAILED'; echo \"exit=${PIPESTATUS[0]}\"",
            "acus ctx 'cargo test' --grep 'ok|FAILED'; echo \"exit=$?\"",
        ),
        // Another status index, single quotes, or `&&` before the echo: the pipeline converts
        // and the echo stays as written.
        (
            "cargo test | tail -5; echo \"a ${PIPESTATUS[0]} ${PIPESTATUS[1]}\"",
            "acus ctx 'cargo test' --tail 5; echo \"a ${PIPESTATUS[0]} ${PIPESTATUS[1]}\"",
        ),
        (
            "cargo test | tail -5; echo 'exit=${PIPESTATUS[0]}'",
            "acus ctx 'cargo test' --tail 5; echo 'exit=${PIPESTATUS[0]}'",
        ),
        (
            "cargo test | tail -5 && echo \"exit=${PIPESTATUS[0]}\"",
            "acus ctx 'cargo test' --tail 5 && echo \"exit=${PIPESTATUS[0]}\"",
        ),
    ] {
        let r = rewrite(cmd, None).unwrap_or_else(|| panic!("not rewritten: {cmd}"));
        assert_eq!(r.command, want, "{cmd}");
        assert!(r.deferred, "{cmd}");
    }
}
