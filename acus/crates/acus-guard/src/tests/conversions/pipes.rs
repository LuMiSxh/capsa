// SPDX-License-Identifier: MPL-2.0

use super::*;

#[test]
fn folds_filters_after_acus_ctx_into_its_flags() {
    for (cmd, want) in [
        (
            "acus ctx 'cargo test' | tail -20",
            "acus ctx 'cargo test' --tail 20",
        ),
        (
            "acus ctx 'cargo test' | tail -n 20",
            "acus ctx 'cargo test' --tail 20",
        ),
        (
            "acus ctx 'cargo test' 2>&1 | tail",
            "acus ctx 'cargo test' --tail 10",
        ),
        (
            "acus ctx 'cargo test' 2>&1 | tail -3",
            "acus ctx 'cargo test' --tail 3",
        ),
        (
            "acus ctx 'cargo test' | grep -E 'a|b'",
            "acus ctx 'cargo test' --grep 'a|b'",
        ),
        (
            "acus ctx 'cargo test' | grep FAILED",
            "acus ctx 'cargo test' --grep 'FAILED'",
        ),
        (
            "acus ctx 'cargo test' --tail 5 | tail -3",
            "acus ctx 'cargo test' --tail 3",
        ),
        (
            "acus ctx 'cargo test' --tail 3 | tail -5",
            "acus ctx 'cargo test' --tail 3",
        ),
    ] {
        let r = rewrite(cmd, None).unwrap_or_else(|| panic!("not rewritten: {cmd}"));
        assert_eq!(r.command, want, "{cmd}");
        assert!(r.deferred, "{cmd}");
    }
    // No exact flag, or the flag is set already: refused.
    for cmd in [
        "acus ctx 'cargo test' | tail -n +5",
        "acus ctx 'cargo test' | grep a*",
    ] {
        assert!(rewrite(cmd, None).is_none(), "{cmd}");
        assert!(
            matches!(check(cmd), Some(Verdict::Deny(Rule::CtxPipe, _))),
            "{cmd}"
        );
    }
    // `grep -v` becomes ctx's `--drop`.
    assert_eq!(
        rewrite("acus ctx 'cargo test' | grep -v ok", None)
            .map(|r| r.command)
            .as_deref(),
        Some("acus ctx 'cargo test' --drop 'ok'")
    );
}

#[test]
fn runs_output_that_goes_into_a_pipe_as_it_is() {
    for cmd in [
        "grep -l foo src | xargs sed -n 1p",
        "cat src/a.rs | python3 -c 'import sys'",
        "grep foo src/a.rs > out.txt",
        "grep -q foo src/a.rs && echo yes",
        "cargo test 2>&1 | grep -q ok",
    ] {
        assert!(rewrite(cmd, None).is_none(), "{cmd}");
        assert_eq!(check(cmd), None, "{cmd}");
    }
    // Only the segment that reaches the model is converted.
    assert_eq!(
        rewrite("cat a.rs; grep -rn x src | xargs wc -l", None)
            .unwrap()
            .command,
        "acus show 'a.rs'; grep -rn x src | xargs wc -l"
    );
}

#[test]
fn display_filters_show_the_output_to_the_model() {
    // A search piped into `head` is the converted search with its limit, so its output is capped.
    for (cmd, want) in [
        (
            "grep -rn foo src | head -80",
            "acus find 'foo' 'src' --max-hits 80",
        ),
        (
            "grep -n foo README.md | head -5",
            "acus find 'foo' 'README.md' --max-hits 5",
        ),
        (
            "grep -rn foo crates | head",
            "acus find 'foo' 'crates' --max-hits 10",
        ),
    ] {
        assert_eq!(
            rewrite(cmd, None).map(|r| r.command).as_deref(),
            Some(want),
            "{cmd}"
        );
    }
    // A read piped into a search is one acus find over the file.
    let cmd = "cat src/a.rs | grep x";
    assert_eq!(
        rewrite(cmd, None).map(|r| r.command).as_deref(),
        Some("acus find 'x' 'src/a.rs'")
    );
    assert!(matches!(check(cmd), Some(Verdict::Deny(..))), "{cmd}");
    // A display filter that keeps the lines (`head N`) stays after the acus read.
    assert_eq!(
        rewrite("cat README.md Cargo.toml | head -50", None)
            .unwrap()
            .command,
        "acus show 'README.md' 'Cargo.toml' | head -50"
    );
    // `cat f | head -N` is the one exact rewrite of a display filter.
    assert_eq!(
        rewrite("cat a.rs | head -5", None).unwrap().command,
        "acus show 'a.rs:1-5'"
    );
    // A sort in the chain is no ctx flag: the chain runs as written.
    assert!(check("cargo build 2>&1 | grep error | sort -u | head -20").is_none());
    // A count is not the lines: the search runs as written.
    for cmd in ["grep -rn foo src | wc -l", "grep foo a.rs | sort | wc -l"] {
        assert!(rewrite(cmd, None).is_none(), "{cmd}");
        assert!(check(cmd).is_none(), "{cmd}");
    }
}

#[test]
fn tail_from_and_head_count_fold_into_one_range() {
    for (cmd, want) in [
        ("cat a.rs | tail -n +3 | head -5", "acus show 'a.rs:3-7'"),
        ("tail -n +3 a.rs | head -5", "acus show 'a.rs:3-7'"),
        ("tail -n +3 a.rs | head", "acus show 'a.rs:3-12'"),
        (
            "cat a.rs 2>/dev/null | tail -n +3 | head -2",
            "acus show 'a.rs:3-4' 2>/dev/null",
        ),
    ] {
        let r = rewrite(cmd, None).unwrap_or_else(|| panic!("not converted: {cmd}"));
        assert_eq!(r.command, want, "{cmd}");
    }
    // A stage after the head reads the rewritten output, and `head -n +N` is no count.
    for cmd in [
        "tail -n +3 a.rs | head -5 | grep x",
        "cat a.rs | tail -n +3 | head -5 | grep x",
        "tail -n +3 a.rs | head -n +5",
        "tail -n +0 a.rs | head -5",
    ] {
        assert!(rewrite(cmd, None).is_none(), "{cmd}");
    }
    // A range past usize does not fold into one range (and does not overflow): the read stays its
    // own call, and the head count stays after it.
    assert_eq!(
        rewrite("tail -n +5 a.rs | head -18446744073709551615", None)
            .unwrap()
            .command,
        "acus show 'a.rs:5-' | head -18446744073709551615"
    );
    assert_eq!(
        rewrite("tail -n +18446744073709551615 a.rs | head -3", None)
            .unwrap()
            .command,
        "acus show 'a.rs:18446744073709551615-' | head -3"
    );
}

#[test]
fn never_converts_output_that_feeds_another_stage() {
    for cmd in [
        "cat a.rs | head -5 | grep '^use'",
        "cat a.rs | head -5 | sort",
        "cat a.rs | head | grep x",
    ] {
        assert!(rewrite(cmd, None).is_none(), "{cmd}");
        assert_eq!(check(cmd), None, "{cmd}");
    }
    // Head then tail is one window of the file, and nothing reads the output after it.
    assert_eq!(
        rewrite("cat a.rs | head -5 | tail -2", None)
            .map(|r| r.command)
            .as_deref(),
        Some("acus show 'a.rs:4-5'")
    );
    // Folded into one ctx call, a trailing stage would read the rewritten output.
    assert!(rewrite("cargo test | tail -5 | grep x", None).is_none());
    assert!(rewrite("cargo test | grep a | head -5 | tail -2", None).is_none());
}
