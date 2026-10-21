// SPDX-License-Identifier: MPL-2.0
//! Guard tests for rewrites of commands into acus calls.

use super::*;

#[test]
fn rewrites_reads_and_searches_into_acus_calls() {
    let cases = [
        ("cat src/a.rs", "acus show 'src/a.rs'"),
        ("head -n 40 src/a.rs", "acus show 'src/a.rs:1-40'"),
        ("sed -n 1,40p src/a.rs", "acus show 'src/a.rs:1-40'"),
        ("grep -rn foo src", "acus find 'foo' 'src'"),
        ("head src/a.rs", "acus show 'src/a.rs:1-10'"),
        ("head -5 src/a.rs", "acus show 'src/a.rs:1-5'"),
        ("tail -30 src/a.rs", "acus show 'src/a.rs:-30'"),
        ("tail -n 20 src/a.rs", "acus show 'src/a.rs:-20'"),
        ("tail -n20 src/a.rs", "acus show 'src/a.rs:-20'"),
        ("tail --lines=7 src/a.rs", "acus show 'src/a.rs:-7'"),
        ("tail src/a.rs", "acus show 'src/a.rs:-10'"),
        ("cat -n src/a.rs", "acus show 'src/a.rs'"),
        (
            "tail -n 5 src/a.rs 2>/dev/null",
            "acus show 'src/a.rs:-5' 2>/dev/null",
        ),
        (
            "cd app && cat a.rs; ls | wc -l",
            "cd app && acus show 'a.rs'; ls | wc -l",
        ),
    ];
    assert_rewrites(&cases, None);
    let r = rewrite("sed -n 1,40p a.rs", None).unwrap();
    assert!(!r.deferred);
    assert_eq!(
        r.note,
        "acus guard: ran `acus show 'a.rs:1-40'` instead of `sed -n 1,40p a.rs`. The command was rewritten, not refused; use acus directly next time."
    );
    // `~` stays expandable, as an absolute path.
    if let Ok(home) = std::env::var("HOME") {
        let r = rw("cat ~/x/a.rs", None).unwrap();
        assert_eq!(r, format!("acus show '{home}/x/a.rs'"));
    }
    // A glob is left to the shell, as cat gets it.
    assert_eq!(
        rw("cat src/*.rs", None).as_deref(),
        Some("acus show src/*.rs")
    );
    // No exact replacement: one such segment keeps the whole command refused.
    for cmd in [
        "tail -n 0 src/a.rs",
        "tail -n -5 src/a.rs",
        "tail src/a.rs src/b.rs",
        "tail -n 5 src/a.rs src/b.rs",
        "head -n 0 src/a.rs",
        "cat -n src/a.rs src/b.rs",
        "sed -i 's/a/b/' a.rs",
        "perl -pi -e 's/a/b/' lib.py",
        "cat a.rs && sed -i 's/a/b/' b.rs",
    ] {
        assert!(rw(cmd, None).is_none(), "{cmd}");
        assert!(matches!(check(cmd), Some(Verdict::Deny(..))), "{cmd}");
    }
    // `tail -n +N` reads from line N to the end: the open-ended range says so.
    assert_eq!(
        rw("tail -n +5 src/a.rs", None).unwrap(),
        "acus show 'src/a.rs:5-'"
    );
    // Following a file is a wait, not a read.
    assert!(rw("tail -f src/a.rs", None).is_none());
    assert_eq!(check("tail -f src/a.rs"), None);
    // Byte counts are not line reads, so they pass.
    for cmd in [
        "tail -c 100 src/a.rs",
        "head -c 5 src/a.rs",
        "tail -c 5 notes.txt",
    ] {
        assert_eq!(check(cmd), None, "{cmd}");
    }
    assert!(rewrite("tail -c 100 src/a.rs", None).is_none());
}

#[test]
fn sed_numeric_scripts_become_line_addresses_and_regex_scripts_are_refused() {
    assert_eq!(rw("sed -n 3p a.rs", None).unwrap(), "acus show 'a.rs:3'");
    assert_eq!(
        rw("sed -n '3p;7,9p' a.rs", None).unwrap(),
        "acus show 'a.rs:3' 'a.rs:7-9'"
    );
    assert!(rw("sed -n '/fn main/p' a.rs", None).is_none());
    assert!(matches!(
        check("sed -n '/fn main/p' a.rs"),
        Some(Verdict::Deny(..))
    ));
}

#[test]
fn globs_stay_unquoted_in_suggestions() {
    // The shell expands a wildcard, so a glob passes as the unglobbed path does.
    assert_eq!(
        check("grep -c foo crates/*/Cargo.toml"),
        check("grep -c foo Cargo.toml")
    );
}

#[test]
fn rewrites_compound_commands_in_place() {
    let cases = [
        (
            "cd X && cat src/tests.rs src/bounded.rs Cargo.toml; grep -n \"exec\" ../../CHANGELOG.md",
            "cd X && acus show 'src/tests.rs' 'src/bounded.rs' 'Cargo.toml'; acus find",
        ),
        (
            "cd X && sed -n 25,70p src/sign.rs; cat Cargo.toml",
            "cd X && acus show 'src/sign.rs:25-70'; cat Cargo.toml",
        ),
        (
            "cd X && acus patch <<'P'\n*** Begin Patch\ncat a.rs `x` $(y)\nP\ncargo test -p a -p b 2>&1 | tail -40",
            "cd X && acus patch <<'P'\n*** Begin Patch\ncat a.rs `x` $(y)\nP\nacus ctx 'cargo test -p a -p b' --tail 40",
        ),
        (
            "cd X && acus patch <<'P' && cargo test -p a -p b 2>&1 | tail -40\nbody cat a.rs\nP\n",
            "cd X && acus patch <<'P' && acus ctx 'cargo test -p a -p b' --tail 40\nbody cat a.rs\nP\n",
        ),
        (
            "acus ctx 'just check' 2>&1 | head -n 120",
            "acus ctx 'just check' --max-output 120",
        ),
        (
            "cd X && cat a.rs b.rs 2>/dev/null && echo ok",
            "cd X && acus show 'a.rs' 'b.rs' 2>/dev/null && echo ok",
        ),
        (
            "acus ctx 'just check' --tail 5 | tail -3",
            "acus ctx 'just check' --tail 3",
        ),
    ];
    for (cmd, want) in cases {
        let r = rewrite(cmd, None).unwrap_or_else(|| panic!("not rewritten: {cmd}"));
        assert!(
            r.command
                .starts_with(want.split("acus find").next().unwrap()),
            "{}",
            r.command
        );
        if !want.ends_with("acus find") {
            assert_eq!(r.command, want, "{cmd}");
        }
        assert!(r.note.contains("instead of"), "{}", r.note);
    }
    // A search piped into `head` folds into `acus find` with its limit; the read beside it too.
    assert_eq!(
        rewrite(
            "cd X && cat a.rs; grep -n \"exec\" ../../CHANGELOG.md | head -40",
            None
        )
        .map(|r| r.command)
        .as_deref(),
        Some("cd X && acus show 'a.rs'; acus find 'exec' '../../CHANGELOG.md' --max-hits 40")
    );
    // The ctx pipe is deferred like a build; plain reads are not.
    assert!(
        rewrite("acus ctx 'just check' | head -9", None)
            .unwrap()
            .deferred
    );
    // No exact flag, or the flag is there already: refused as before.
    let cmd = "cat a.rs && acus ctx 'x' | tail -n +5";
    assert!(rw(cmd, None).is_none(), "{cmd}");
    assert!(matches!(check(cmd), Some(Verdict::Deny(..))), "{cmd}");
    // A hint is not a refusal: nothing to rewrite.
    assert!(rw("git status", None).is_none());
}

#[test]
fn rewrites_search_flags_it_can_carry_and_refuses_the_rest() {
    let cases = [
        (
            "grep -l 'only when X' src/a.rs src/b.rs; grep -l 'Y' src/c.rs",
            "acus find 'only when X' 'src/a.rs' 'src/b.rs' -l; acus find 'Y' 'src/c.rs' -l",
        ),
        ("grep -l foo src", "acus find 'foo' 'src' -l"),
        ("grep -iw foo src", "acus find 'foo' 'src' -i -w"),
        ("grep -n -r -E foo src", "acus find 'foo' 'src'"),
        ("grep -o -F 'a b' src", "acus find 'a b' 'src' -o -F"),
        (
            "grep -e foo --max-count=1 src",
            "acus find 'foo' 'src' -m 1",
        ),
        (
            "rg -l --hidden -t rs foo src",
            "acus find 'foo' 'src' -l --hidden -t 'rs'",
        ),
        ("grep -x foo src", "acus find 'foo' 'src' -x"),
        ("grep -rx foo src", "acus find 'foo' 'src' -x"),
        (
            "grep --line-regexp -F foo src",
            "acus find 'foo' 'src' -x -F",
        ),
        ("rg -x foo src", "acus find 'foo' 'src' -x"),
        ("rg -L foo src", "acus find 'foo' 'src' -L"),
        ("rg --follow -i foo src", "acus find 'foo' 'src' -L -i"),
        ("rg -U 'a\\nb' src", "acus find 'a\\nb' 'src' -U"),
        (
            "rg --multiline -l 'a\\nb' src",
            "acus find 'a\\nb' 'src' -U -l",
        ),
        // `grep -R` follows symlinks, `grep -r` only those on the command line.
        ("grep -R foo src", "acus find 'foo' 'src' -L"),
        (
            "grep --dereference-recursive foo src",
            "acus find 'foo' 'src' -L",
        ),
    ];
    assert_rewrites(&cases, None);
    // A flag, pattern source or output mode without an exact equivalent: not rewritten.
    for cmd in [
        "grep -P 'a(?=b)' src",
        "grep -z foo src",
        "grep -v -o foo src",
        "grep -m 2 -v foo src",
        "grep -h foo src",
        "grep -L foo src",
        "grep -U foo src",
        "grep --follow foo src",
        "rg -U -v foo src",
        "rg -U -C 2 foo src",
        "rg -U -A1 foo src",
        "grep -xw foo src",
        "ag -x foo src",
        "ag -L foo src",
        "ack -U foo src",
        "grep -e a -e b src",
        "grep -f pats.txt src",
        "grep foo - src",
        "rg -r rep foo src",
        "sed -n 40,1p src/a.rs",
        "sed -n 0,5p src/a.rs",
    ] {
        assert!(rw(cmd, None).is_none(), "{cmd}");
        assert!(matches!(check(cmd), Some(Verdict::Deny(..))), "{cmd}");
    }
    // A count is not the lines: `-c` and `--count` run as written, with no rewrite or hint.
    for cmd in [
        "grep -c foo src/a.rs",
        "grep --count foo src",
        "grep -c -l foo src",
        "grep -nc foo README.md",
        "rg -c foo src",
        "rg --count -i foo src",
        "rg -U -c foo src",
    ] {
        assert!(rw(cmd, None).is_none(), "{cmd}");
        assert!(check(cmd).is_none(), "{cmd}");
    }
}

#[test]
fn compound_rewrites_never_pass_user_text_unquoted() {
    for (cmd, want) in [
        ("cd x && cat 'a'\\'';rm -rf x;'\\''.rs'", None),
        (
            "cd x && cat \"a';rm -rf x;'.rs\"; echo hi",
            Some("cd x && acus show \"a';rm -rf x;'.rs\"; echo hi"),
        ),
        ("cd x && cat $(touch pwned).rs; echo hi", None),
        ("cd x && cat `touch pwned`.rs; echo hi", None),
        ("cd x && cat \"$(touch pwned).rs\"; echo hi", None),
        (
            "cat a.rs 2>/dev/null; touch pwned > $HOME/x",
            Some("acus show 'a.rs' 2>/dev/null; touch pwned > $HOME/x"),
        ),
    ] {
        assert_eq!(rw(cmd, None).as_deref(), want, "{cmd}");
    }
    // A filter pattern with a quote has no flag, so the pipe is refused, not rewritten.
    assert_eq!(
        rw("cd x && cargo test | grep \"a';touch pwned;'\"", None),
        None
    );
    // Nothing but quoted words reaches an `acus ctx` flag.
    let r = rw("cd x && cargo test | grep \"a;touch pwned\"", None).unwrap();
    assert_eq!(r, "cd x && acus ctx 'cargo test' --grep 'a;touch pwned'");
}

#[test]
fn a_rewrite_leaves_heredocs_and_the_commands_they_hold_as_they_are() {
    for cmd in [
        "cat a.rs; sh <<EOF\n$(rm -rf ~)\nEOF\n",
        "cat a.rs; sh <<-EOF\n`rm -rf ~`\nEOF\n",
        "cat a.rs; sh <<\"EOF\" <<EOF2\nx\nEOF\n$(rm x)\nEOF2\n",
        "cat a.rs; sh <<<\"$(rm -rf ~)\"\n",
        "cat a.rs; sh <<<word\n$(rm -rf ~)\n",
        "cat a.rs; sh <<\\EOF\n$(rm -rf ~)\nEOF\n",
        "cat a.rs; sh <<'END-OF'\n$(rm -rf ~)\nEND-OF\n",
        "cat a.rs; cat <<'A' <<'B'\nx\nA\n$(rm x)\nB\n",
    ] {
        // Where the parser cannot tell what the line means, nothing is rewritten; elsewhere only
        // the `cat` changes, whatever the heredoc or here-string expands.
        if let Some(r) = rewrite(cmd, None) {
            assert_eq!(r.command, cmd.replacen("cat a.rs", "acus show 'a.rs'", 1));
        }
    }
    for cmd in [
        "cat a.rs; sh <<EOF\n$(rm -rf ~)\nEOF\n",
        "cat a.rs; sh <<<word\n$(rm -rf ~)\n",
        "cat a.rs; sh <<'EOF'\n$(ok)\nEOF\n",
    ] {
        assert!(rewrite(cmd, None).is_some(), "{cmd}");
    }
}

#[test]
fn rewrites_build_pipes_into_deferred_acus_calls() {
    for (cmd, want) in [
        (
            "cargo test 2>&1 | tail -3",
            "acus ctx 'cargo test' --tail 3",
        ),
        (
            "cd app && swift build | grep error && echo ok",
            "cd app && acus ctx 'swift build' --grep 'error' && echo ok",
        ),
    ] {
        assert_eq!(rw(cmd, None).as_deref(), Some(want), "{cmd}");
        assert!(rewrite(cmd, None).unwrap().deferred, "{cmd}");
    }
    // A read next to a build pipe is rewritten too, and the whole command is deferred.
    let r = rewrite("cat a.rs; cargo test | tail -3", None).unwrap();
    assert_eq!(
        r.command,
        "acus show 'a.rs'; acus ctx 'cargo test' --tail 3"
    );
    assert!(r.deferred);
    // A filter ctx has no flag for stays refused.
    assert_eq!(rw("cargo test | tail -n +5", None), None);
}

#[test]
fn build_pipes_chain_their_filters_into_one_ctx_call() {
    for (cmd, want) in [
        (
            "swift build --build-tests 2>&1 | grep -E 'error|warning' | grep -v '^\\[' | head -30",
            "acus ctx 'swift build --build-tests' --grep 'error|warning' --drop '^\\[' --max-output 30",
        ),
        (
            "cargo test 2>&1 | grep -v '^warning\\|^note'",
            "acus ctx 'cargo test' --drop '^warning|^note'",
        ),
    ] {
        assert_eq!(rw(cmd, None).as_deref(), Some(want), "{cmd}");
        assert!(rewrite(cmd, None).unwrap().deferred, "{cmd}");
    }
    // A build chain that one ctx call cannot carry stays refused.
    for cmd in [
        "cargo test 2>&1 | head -3 | grep x",
        "cargo test 2>&1 | grep -v x | grep -v y | wc -l",
    ] {
        assert!(matches!(check(cmd), Some(Verdict::Deny(..))), "{cmd}");
        assert!(rewrite(cmd, None).is_none(), "{cmd}");
    }
    // ctx only drops adjacent repeats, so a chain with sort or uniq runs as written.
    for cmd in [
        "cargo test 2>&1 | grep error | sort | head -30",
        "swift build --build-tests 2>&1 | grep -E 'error' | sort -u | head -30",
        "cargo test 2>&1 | grep x | sort | uniq | head -30",
        "cargo test 2>&1 | grep x | sort -u | head -30",
    ] {
        assert!(check(cmd).is_none(), "{cmd}");
        assert!(rewrite(cmd, None).is_none(), "{cmd}");
    }
}

#[test]
fn help_and_version_output_is_not_a_build_and_passes() {
    for cmd in [
        "acus ctx --help 2>&1 | grep -E '^\\s+-'",
        "cargo --help | grep x",
        "swift build --help | grep x",
        "cargo -V | grep x",
        "cargo --version | grep x",
        "swift build -h | grep x",
        "cargo help build | grep x",
        "cargo test -- -h | tail -5",
    ] {
        assert!(check(cmd).is_none(), "{cmd}");
        assert!(rewrite(cmd, None).is_none(), "{cmd}");
    }
    // `help` after the subcommand is a test or filter name: the build still runs and is rewritten.
    for cmd in [
        "cargo test help | tail -5",
        "swift test --filter help | tail -5",
    ] {
        assert!(matches!(check(cmd), Some(Verdict::Deny(..))), "{cmd}");
        assert!(rewrite(cmd, None).is_some(), "{cmd}");
    }
}

#[test]
fn names_the_ctx_flag_that_replaces_a_build_filter() {
    for (cmd, want) in [
        (
            "cargo test -q | tail -n 40",
            "`acus ctx 'cargo test -q' --tail 40`",
        ),
        (
            "cargo test -q 2>&1 | tail -40",
            "`acus ctx 'cargo test -q' --tail 40`",
        ),
        (
            "cargo test -q | tail -n40",
            "`acus ctx 'cargo test -q' --tail 40`",
        ),
        (
            "cargo build | tail --lines=5",
            "`acus ctx 'cargo build' --tail 5`",
        ),
        ("cargo build | tail", "`acus ctx 'cargo build' --tail 10`"),
        (
            "cargo test | grep -E 'FAILED|panicked'",
            "`acus ctx 'cargo test' --grep 'FAILED|panicked'`",
        ),
        (
            "cargo test | grep -n FAILED",
            "`acus ctx 'cargo test' --grep 'FAILED'`",
        ),
        (
            "cargo test | grep -i error",
            "`acus ctx 'cargo test' --grep '(?i)error'`",
        ),
        (
            "cargo test | grep -v ok",
            "`acus ctx 'cargo test' --drop 'ok'`",
        ),
        (
            "cargo test | grep -e FAILED",
            "`acus ctx 'cargo test' --grep 'FAILED'`",
        ),
        (
            "cargo test | grep 'a\\|b'",
            "`acus ctx 'cargo test' --grep 'a|b'`",
        ),
        (
            "cargo test | rg 'a|b'",
            "`acus ctx 'cargo test' --grep 'a|b'`",
        ),
        (
            "swift build 2>&1 | head -50",
            "`acus ctx 'swift build' --max-output 50`",
        ),
        (
            "npm test | head -n 20",
            "`acus ctx 'npm test' --max-output 20`",
        ),
    ] {
        let (rule, r) = deny(check(cmd));
        assert_eq!(rule, "build-pipe", "{cmd}");
        assert!(
            r.contains(&format!("Use {want} instead of piping")),
            "{cmd}: {r}"
        );
    }
    // No equivalent flag: the reason still names both.
    for cmd in [
        "cargo test | grep -A3 -v FAILED",
        "cargo test | tail -n +5",
        "cargo test | tail -c 100",
        "cargo test | grep \"it's\"",
    ] {
        let (rule, r) = deny(check(cmd));
        assert_eq!(rule, "build-pipe", "{cmd}");
        assert!(r.contains("`acus ctx 'cargo test'`"), "{cmd}: {r}");
        assert!(
            r.contains("`--tail N`") && r.contains("`--grep RE`"),
            "{cmd}: {r}"
        );
        assert!(!r.contains("Use `acus ctx 'cargo test' --"), "{cmd}: {r}");
    }
    let (_, r) = deny(check("cd app && cargo test -q | tail -5 && echo done"));
    assert!(r.contains("Use `acus ctx 'cargo test -q' --tail 5`"), "{r}");
    assert!(r.contains("Replace that segment"), "{r}");
}

#[test]
fn refuses_acus_ctx_pipes_that_cannot_fold() {
    // A pipe inside the quoted command: the pipe goes and its filter becomes the flag.
    let (rule, r) = deny(check(
        "acus ctx 'swift build --quiet | tail -40' | head -60",
    ));
    assert_eq!(rule, "ctx-pipe");
    assert!(
        r.contains(
            "Use `acus ctx 'swift build --quiet' --tail 40` instead of piping acus ctx into head"
        ),
        "{r}"
    );
    assert!(
        r.contains("drop the pipe `| tail -40` inside the quoted command"),
        "{r}"
    );
    assert!(
        r.contains("Send instead: acus ctx 'swift build --quiet' --tail 40"),
        "{r}"
    );
    // The command already has a line count: a second one would keep other lines.
    let (rule, r) = deny(check("acus ctx 'cargo test' --tail 15 | head -5"));
    assert_eq!(rule, "ctx-pipe");
    assert!(
        r.contains("Use `acus ctx 'cargo test' --tail 15` instead of piping acus ctx into head"),
        "{r}"
    );
    // `sort` and `uniq` dedupe or reorder whole lines, so such a chain runs as written.
    for cmd in [
        "acus ctx 'cargo test' | sort | tail -3",
        "acus ctx 'cargo test' | uniq | tail -3",
        "acus ctx 'cargo test' | sort -u",
    ] {
        assert!(check(cmd).is_none(), "{cmd}");
    }
    // A flag the command has already, or a `--` before the pipe, has no second copy: the refusal
    // names no command to send.
    for cmd in [
        "acus ctx 'x' --grep a | grep b",
        "acus ctx 'x' --tail=5 | tail -3",
        "acus ctx 'x' -- | grep y",
    ] {
        let (rule, r) = deny(check(cmd));
        assert_eq!(rule, "ctx-pipe", "{cmd}");
        assert!(!r.contains("Send instead:"), "{cmd}: {r}");
    }
    // A context window before a `grep -v` reads other lines than the pipe does: refused, in both
    // the ctx pipe and the build pipe.
    // A filter chain after the pair: no command is named unless every stage maps, since one that
    // maps only the pair would drop the stages after it.
    let cmd = "acus ctx 'c' | grep -A2 error | grep -v warn";
    let (rule, r) = deny(check(cmd));
    assert_eq!(rule, "ctx-pipe");
    assert!(!r.contains("Send instead:"), "{cmd}: {r}");
    assert!(rewrite(cmd, None).is_none(), "{cmd}");
    let cmd = "cargo test | grep -A2 error | grep -v warn";
    let (rule, r) = deny(check(cmd));
    assert_eq!(rule, "build-pipe");
    assert!(!r.contains("Send instead:"), "{cmd}: {r}");
    assert!(rewrite(cmd, None).is_none(), "{cmd}");
    // Stacked greps that no ctx flag set expresses run as written, in a build pipe as after ctx.
    let cmd = "cargo test | grep a | grep b";
    assert!(check(cmd).is_none(), "{cmd}");
    assert!(rewrite(cmd, None).is_none(), "{cmd}");
    assert_eq!(
        check("command acus ctx 'cargo test' | tail"),
        Some(Verdict::Escape(Rule::CtxPipe))
    );
}

#[test]
fn folds_acus_ctx_pipes_into_its_flags() {
    let cases = [
        (
            "acus ctx 'cargo test -q' 2>&1 | tail -5",
            "acus ctx 'cargo test -q' --tail 5",
        ),
        (
            "acus ctx 'cargo test -q' | tail -n 30",
            "acus ctx 'cargo test -q' --tail 30",
        ),
        ("acus ctx | grep FAILED", "acus ctx --grep 'FAILED'"),
        (
            "acus ctx 'make' --raw | head -20",
            "acus ctx 'make' --raw --max-output 20",
        ),
        (
            "~/bin/acus ctx 'go test ./...' | tail",
            "~/bin/acus ctx 'go test ./...' --tail 10",
        ),
        (
            "acus ctx 'cargo test' | grep -v ok",
            "acus ctx 'cargo test' --drop 'ok'",
        ),
        (
            "acus ctx 'cargo test' | grep -vE 'ok|FAIL' | tail -20",
            "acus ctx 'cargo test' --drop 'ok|FAIL' --tail 20",
        ),
        // `grep -v` first: ctx drops the lines before the context window, as the pipe does.
        (
            "acus ctx 'cargo test' | grep -v warn | grep -A2 error",
            "acus ctx 'cargo test' --drop 'warn' --grep 'error' -A 2",
        ),
        (
            "cargo test | grep -v warn | grep -A2 error",
            "acus ctx 'cargo test' --drop 'warn' --grep 'error' -A 2",
        ),
        // A `2>&1` inside the quoted command is kept; the one after ctx is dropped.
        (
            "acus ctx 'cargo build 2>&1' | tail -40",
            "acus ctx 'cargo build 2>&1' --tail 40",
        ),
        // A trailing `cut` stays behind the rewritten command.
        (
            "acus ctx 'cargo test' | tail -n 5 | cut -c1-300",
            "acus ctx 'cargo test' --tail 5 | cut -c1-300",
        ),
        (
            "acus ctx 'cargo test' | grep -E ok | cut -c1-80",
            "acus ctx 'cargo test' --grep 'ok' | cut -c1-80",
        ),
        (
            "acus ctx 'cargo test' --tail 15 | tail -15",
            "acus ctx 'cargo test' --tail 15",
        ),
    ];
    assert_rewrites(&cases, None);
    // Counts and word counts print no lines to cut: they pass, on both paths.
    for cmd in [
        "acus ctx 'cargo test' | wc -l",
        "acus ctx 'cargo test' | grep -c ok",
        "cargo test | grep -c ok",
        "acus ctx 'cargo test' --tail 40",
        "acus find foo | head",
        "acus ctx 'cargo test | tail -5'",
    ] {
        assert_eq!(check(cmd), None, "{cmd}");
    }
}

#[test]
fn build_pipes_with_expansions_are_refused_and_plain_ones_rewritten() {
    // ctx's shell would expand `$X` again: refused; the suggestion names double quotes.
    for (cmd, want) in [
        (
            "cargo test -p $P | tail -3",
            "`acus ctx \"cargo test -p $P\" --tail 3`",
        ),
        (
            "X='a; rm -rf y'; cargo build $X 2>&1 | tail -5",
            "`acus ctx \"cargo build $X\" --tail 5`",
        ),
    ] {
        assert!(rewrite(cmd, None).is_none(), "{cmd}");
        let (rule, r) = deny(check(cmd));
        assert_eq!(rule, "build-pipe", "{cmd}");
        assert!(
            r.contains(&format!("Use {want} instead of piping")),
            "{cmd}: {r}"
        );
    }
    // Plain commands keep single quotes; a backtick is refused as well.
    let r = rewrite("cargo build 2>&1 | tail -5", None).unwrap();
    assert_eq!(r.command, "acus ctx 'cargo build' --tail 5");
    assert!(rewrite("cargo build `pwd` | tail -5", None).is_none());
}
