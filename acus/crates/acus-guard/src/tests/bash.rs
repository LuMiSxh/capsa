// SPDX-License-Identifier: MPL-2.0
//! Guard tests for Bash commands.

use super::*;

#[test]
fn names_the_refused_segment_and_its_replacement_in_compound_commands() {
    let (rule, r) = deny(check("cd X && ls a b; cat src/a.rs; acus find foo"));
    assert_eq!(rule, "read-shell");
    assert!(r.starts_with("Nothing ran: the whole command was refused because of `cat src/a.rs`; the other segments are fine."), "{r}");
    assert!(r.contains("Replace it with `acus show 'src/a.rs'`"), "{r}");

    let (rule, r) = deny(check("cmd1 && grep -n -i 'foo bar' src file.log && cmd3"));
    assert_eq!(rule, "grep");
    assert!(
        r.contains("because of `grep -n -i 'foo bar' src file.log`"),
        "{r}"
    );
    assert!(
        r.contains("`acus find 'foo bar' 'src' 'file.log' -i`"),
        "{r}"
    );

    let (_, r) = deny(check("cd app && cargo test -q | tail -5 && echo done"));
    assert!(r.contains("because of `cargo test -q | tail -5`"), "{r}");
    assert!(r.contains("acus ctx 'cargo test -q'"), "{r}");

    let (_, r) = deny(check("cd a && head -n 20 src/a.rs"));
    assert!(r.contains("`acus show 'src/a.rs:1-20'`"), "{r}");
    let (_, r) = deny(check("cd a && sed -n '10,20p' src/a.rs"));
    assert!(r.contains("`acus show 'src/a.rs:10-20'`"), "{r}");
    let (_, r) = deny(check("true && cat a.rs b.rs"));
    assert!(r.contains("`acus show 'a.rs' 'b.rs'`"), "{r}");
    let (_, r) = deny(check("cd x && sed -i 's/a/b/' a.rs b.rs"));
    assert!(r.contains("`*** Replace All: a.rs b.rs`"), "{r}");
    let (_, r) = deny(check("cd x && rg -n --glob '*.rs' -w foo"));
    assert!(r.contains("`acus find 'foo' -g '*.rs' -w`"), "{r}");

    // The escape hint stays, and a lone command is not called compound.
    assert!(r.contains("rerun with a `command ` prefix."), "{r}");
    let (_, r) = deny(check("cat src/a.rs"));
    assert!(!r.contains("Nothing ran"), "{r}");

    // Long segments are cut.
    let (_, r) = deny(check(&format!("cd x && cat {}.rs", "a".repeat(400))));
    // The send line repeats the whole command, so the cut is checked on the first line.
    assert!(
        r.contains('…') && r.lines().next().is_some_and(|l| l.len() < 700),
        "{r}"
    );
}

#[test]
fn leaves_data_files_plain_files_and_other_commands_output_alone() {
    // Decisions: JSON, TOML and YAML are data, read whole on purpose (the Read tool agrees);
    // searching a log or text file is not a code search; a pipe or stdin is another
    // command's output; `>`/`>>` make a read a copy or an append.
    for cmd in [
        "cat .claude/settings.local.json",
        "cat Cargo.toml",
        "head -20 config.yaml",
        "tail -n 5 data.yml",
        "grep version Cargo.toml",
        "grep -c error build.log",
        "grep -n TODO notes.txt out.csv",
        "grep -l foo *.log",
        "cmd | grep -c foo",
        "cmd | grep -l foo",
        "grep -c foo <(cmd)",
        "grep -q foo <<< \"$out\"",
        "grep -c foo < results.log",
        "grep -c foo",
        "cat a.rs >> all.txt",
        "cat a.rs b.rs > all.txt",
        "cat a.rs 1> all.txt",
        "cat > conf.json <<'EOF'\n{}\nEOF",
        "cat > notes.md <<'EOF'\ngrep foo src\nEOF",
    ] {
        assert_eq!(check(cmd), None, "{cmd}");
    }
}

#[test]
fn git_log_oneline_gets_no_hint() {
    for cmd in [
        "git log --oneline",
        "git log --oneline -5",
        "git log --oneline -n 5 -- src",
        "git log --oneline main..HEAD",
        "git log --oneline -n 5 src",
        "git log --oneline -n5",
        "git log --oneline -- src/main.rs",
        "git log --oneline | grep fix",
        "git log --oneline 2>&1 | grep -c fix",
    ] {
        assert!(check(cmd).is_none(), "{cmd}");
    }
}

#[test]
fn git_status_and_pinned_formats_get_no_hint() {
    for cmd in [
        "git status -s",
        "git status --short",
        "git log -1 --format=%H",
    ] {
        assert!(check(cmd).is_none(), "{cmd}");
    }
}

#[test]
fn git_log_gets_no_hint() {
    for cmd in [
        "git log",
        "git log -p",
        "git log --stat",
        "git log -5 --format=%s",
        "git log | grep fix",
        "git log 2>&1 | grep -c fix",
        "git log | head -20",
    ] {
        assert!(check(cmd).is_none(), "{cmd}");
    }
}

#[test]
fn still_refuses_source_reads_and_searches_that_look_harmless() {
    for cmd in [
        "cat src/a.rs 2>&1",
        "cat src/a.rs 2>/dev/null",
        "cat settings.json src/a.rs",
        "grep foo src/a.rs",
        "grep -l foo README.md",
        "grep -rn foo .",
        "grep -l foo src",
        "grep -l foo *",
        "grep -l foo build.log src",
        "grep -rn foo",
        "rg foo",
        "rg foo notes.txt src",
        "sed -i '' 's/a/b/' config.json",
        "find . -name '*.rs' -exec grep -n foo {} +",
        "ls | xargs grep -l foo",
        "env FOO=1 grep -rn foo src",
    ] {
        assert!(matches!(check(cmd), Some(Verdict::Deny(..))), "{cmd}");
    }
}

// The shell expands these words before grep sees them, so `acus find` cannot take them as they
// are: the command runs as written, with a hint. Captured or redirected output gets no verdict.
#[test]
fn grep_with_shell_expansion_runs_with_a_hint() {
    for cmd in [
        "grep -rn \"$PAT\" src",
        "grep -r $(cat f) .",
        "rg \"${X}\" .",
        "grep -l foo $file",
    ] {
        assert!(
            matches!(check(cmd), Some(Verdict::Hint(Rule::Grep, _))),
            "{cmd}"
        );
        assert!(rw(cmd, None).is_none(), "{cmd}");
    }
    for cmd in [
        "x=$(grep -rn \"$PAT\" src)",
        "grep -rn \"$PAT\" src > out.txt",
    ] {
        assert!(check(cmd).is_none(), "{cmd}");
    }
}

// A compound command with a shell-expanded grep is rewritten, not hinted: the rewrite is tried
// before the verdict, and folding reads the grep's rule.
#[test]
fn compound_rewrites_win_over_the_expansion_hint() {
    let scope = Scope::fixed("/");
    for (cmd, want) in [
        (
            "B=~/x; grep -n \"pub fn \\|^impl\" $B/a.rs | head -70",
            "B=~/x; acus find 'pub fn |^impl' $B/a.rs --max-hits 70",
        ),
        (
            "C=~/x; ls $C; grep -n \"pub fn \" $C/a.rs | head -30",
            "C=~/x; ls $C; acus find 'pub fn ' $C/a.rs --max-hits 30",
        ),
        (
            "cd x && f=a.rs; wc -l $f; grep -n 'x' $f | head -40; head -8 $f",
            "cd x && f=a.rs; wc -l $f; acus find 'x' $f --max-hits 40; head -8 $f",
        ),
        (
            "R=$(ls -d ~/x*); grep -n \"a\\|b\" $R/a.rs | head -60; ls $R | head",
            "R=$(ls -d ~/x*); acus find 'a|b' $R/a.rs --max-hits 60; ls $R | head",
        ),
    ] {
        assert_eq!(rw(cmd, Some(&scope)).as_deref(), Some(want), "{cmd}");
        assert!(
            matches!(
                crate::decide_bash(cmd, Some(&scope), true),
                crate::BashDecision::Rewrite(_)
            ),
            "{cmd}"
        );
    }
}

#[test]
fn compound_with_one_unrewritable_segment_is_still_refused() {
    let cmd = "cd X && sed -i 's/a/b/' b.rs; cat a.rs; echo done";
    assert!(rewrite(cmd, None).is_none());
    let Some(Verdict::Deny(_, reason)) = check(cmd) else {
        panic!("expected refusal")
    };
    assert!(reason.contains("sed -i 's/a/b/' b.rs"), "{reason}");
}

#[test]
fn refuses_file_searches_reads_and_in_place_edits() {
    for cmd in [
        "grep -rn foo src",
        "cd app && rg -n 'fn main'",
        "git ls-files | xargs grep -n TODO",
        "sed -n 1,40p src/main.rs",
        "sed -i '' 's/a/b/' Sources/App.swift",
        "perl -pi -e 's/a/b/' lib.py",
        "cat src/a.rs src/b.rs",
        "X=1 head -50 README.md",
        "python3 - <<'EOF'\nopen('a.rs','w').write(s.replace('a','b'))\nEOF\ngrep -n x a.rs",
        "cargo test 2>&1 | grep -E 'FAILED|panicked'",
        "cd crates && cargo test -q -p acus-cli | tail -30",
        "swift build 2>&1 | head -50",
        "npx tsc --noEmit | head",
        "pnpm run build | tail -20",
    ] {
        assert!(matches!(check(cmd), Some(Verdict::Deny(..))), "{cmd}");
    }
}

#[test]
fn hints_at_edit_scripts() {
    for cmd in [
        "python3 - <<'EOF'\np='src/a.rs'\ns=open(p).read()\nopen(p,'w').write(s.replace('a','b'))\nEOF",
        "cd app && uv run python - <<'E'\nimport re, pathlib\nf=pathlib.Path(\"Sources/A.swift\")\nf.write_text(re.sub(r'x','y',f.read_text()))\nE",
        "python3 -c \"p='lib.py'; s=open(p).read(); open(p,'w').write(s.replace('a','b'))\"",
        "node -e \"const fs=require('fs');fs.writeFileSync('a.ts',fs.readFileSync('a.ts','utf8').replace('a','b'))\"",
        "cd /w && cat > .tmp_e.py <<'EOF'\nfrom pathlib import Path\np = Path(\"a/b.py\")\np.write_text(p.read_text().replace('x', 'y'))\nEOF\npython .tmp_e.py && rm .tmp_e.py",
        "tee fix.rb <<'EOF'\nf = 'a.rb'\nFile.write(f, File.read(f).gsub('a', 'b'))\nEOF",
    ] {
        assert!(matches!(check(cmd), Some(Verdict::Deny(..))), "{cmd}");
    }
    // A literal outside the repository keeps the hint.
    let scope = Scope::fixed("/repo");
    let outside = "python3 - <<'EOF'\np='/tmp/a.rs'\ns=open(p).read()\nopen(p,'w').write(s.replace('a','b'))\nEOF";
    assert!(matches!(
        check_in(outside, false, Some(&scope)),
        Some(Verdict::Hint(Rule::EditScript, _))
    ));
    // A script file is a source file like any other.
    assert!(matches!(
        check("cat > run.py <<'EOF'\nprint('hi')\nEOF"),
        Some(Verdict::Deny(Rule::FileWrite, _))
    ));
}

#[test]
fn hints_at_acus_map_for_recursive_listings() {
    for cmd in [
        "tree -L 2",
        "ls -laR src",
        "find . -type f",
        "find Sources -maxdepth 2",
    ] {
        assert!(
            matches!(check(cmd), Some(Verdict::Hint(Rule::Tree, _))),
            "{cmd}"
        );
    }
    assert!(matches!(
        check_ps("Get-ChildItem -Recurse src", None),
        Some(Verdict::Hint(Rule::Tree, _))
    ));
    for cmd in [
        "ls -la",
        "find . -name '*.rs'",
        "find build -delete",
        "ls --color",
    ] {
        assert!(check(cmd).is_none(), "{cmd}");
    }
}

#[test]
fn hints_at_acus_for_plain_git_status_diff_and_show() {
    for (cmd, rule) in [
        ("git status", "git-status"),
        ("git diff", "git-diff"),
        ("git diff --cached src/main.rs", "git-diff"),
        ("git diff --stat", "git-diff"),
        ("git show HEAD", "git-show"),
    ] {
        assert!(
            matches!(check(cmd), Some(Verdict::Hint(r, _)) if r.as_str() == rule),
            "{cmd}"
        );
    }
    assert!(matches!(
        check_ps("git status", None),
        Some(Verdict::Hint(Rule::GitStatus, _))
    ));
    for cmd in [
        "git status --short",
        "git status -sb",
        "git diff --name-only HEAD~1",
        "git log --oneline --graph",
        "git log --oneline --author=me",
        "git log --oneline -n",
        "git log --oneline --decorate -5",
        "git log --format=%h",
        "git show HEAD:src/main.rs",
        "git show --stat HEAD",
        "git commit -m x",
        "git add -A",
    ] {
        assert!(check(cmd).is_none(), "{cmd}");
    }
}

#[test]
fn code_extensions_cover_every_outlined_language() {
    use super::super::read::CODE;
    for ext in Lang::extensions() {
        assert!(CODE.contains(&ext), "{ext}");
    }
    for ext in ["sh", "bash", "zsh", "c", "h", "scss", "htm", "plist", "xml"] {
        assert!(CODE.contains(&ext), "{ext}");
    }
    assert!(!CODE.contains(&"m"));
}

#[test]
fn allows_filters_data_and_escapes() {
    for cmd in [
        "cargo tree | grep serde",
        "cargo test 2>&1 | tee test.log",
        "acus ctx 'cargo test | tail'",
        "npm install | tail -3",
        "git commit -m \"fix; grep foo\"",
        "cat > notes.md <<'EOF'\ngrep foo bar\nEOF",
        "acus patch <<'P'\nsed -i x a.rs\nP",
        "cat Cargo.lock",
        "tail -f server.log",
        "cat a.rs > b.rs",
        "sed 's/a/b/' input.txt",
        "ls src && find . -name '*.rs'",
        "python3 - <<'EOF'\nimport json\nprint(json.load(open('data.json'))['x'].replace('a','b'))\nEOF",
        "python3 - <<'EOF'\nrows=open('in.csv').read().replace(';',',')\nopen('out.csv','w').write(rows)\nEOF",
        "acus patch <<'P'\n*** Update File: tool.py\n<<<<<<< SEARCH\nx\n=======\nopen('a.py','w').write(s.replace('a','b'))\n>>>>>>> REPLACE\nP",
    ] {
        assert_eq!(check(cmd), None, "{cmd}");
    }
}

#[test]
fn logs_escapes_as_such() {
    for (cmd, rule) in [
        ("command grep -rn foo src", "grep"),
        ("cd a && command cat src/a.rs", "read-shell"),
        ("command cargo test | tail", "build-pipe"),
        ("grep -rn foo src # acus-skip", "grep"),
    ] {
        assert!(
            matches!(check(cmd), Some(Verdict::Escape(r)) if r.as_str() == rule),
            "{cmd}"
        );
    }
    // An escape elsewhere does not excuse another refused segment.
    assert!(matches!(
        check("command grep x a.rs; grep y src"),
        Some(Verdict::Deny(Rule::Grep, _))
    ));
}

#[test]
fn logged_calls_are_cut_to_the_logged_length() {
    // The log goes into a temporary config dir, never the user's.
    let dir = tempfile::tempdir().unwrap();
    let cfg = crate::GuardConfig {
        hint_every: 10,
        hint_min: 5,
        hint_max: 40,
        state_dir: dir.path().join("guard-hints"),
        log_path: dir.path().join("guard.jsonl"),
    };
    let hook = json!({
        "session_id": "s",
        "tool_name": "Bash",
        "tool_input": { "command": "x".repeat(LOG_CALL_MAX + 1000) },
    });
    log::append(&cfg, &hook, &Verdict::Deny(Rule::Grep, "r".to_owned())).unwrap();
    let text = std::fs::read_to_string(&cfg.log_path).unwrap();
    let line: Value = serde_json::from_str(text.lines().next().unwrap()).unwrap();
    assert_eq!(line["call"].as_str().unwrap().chars().count(), LOG_CALL_MAX);
}

/// Pins that `|&` pipes stderr as well as stdout. The guard does not read it as a pipe, so
/// `cat a.rs |& head -5` is refused as a plain read of `a.rs` and not rewritten, where
/// `cat a.rs | head -5` is rewritten.
#[test]
fn stderr_pipe_is_refused_as_a_plain_read_and_not_rewritten() {
    let (r, reason) = deny(check("cat a.rs |& head -5"));
    assert_eq!(r, "read-shell");
    assert!(reason.contains("because of `cat a.rs`"), "{reason}");
    assert!(rewrite("cat a.rs |& head -5", None).is_none());
    assert_eq!(
        rewrite("cat a.rs | head -5", None)
            .map(|r| r.command)
            .as_deref(),
        Some("acus show 'a.rs:1-5'")
    );
}
