// SPDX-License-Identifier: MPL-2.0
//! Guard tests for segments whose stdout is data rather than output the agent reads.

use super::*;

/// Checks that captured output is neither refused, hinted nor rewritten.
fn runs_unchanged(cmd: &str) {
    assert_eq!(check(cmd), None, "{cmd}");
    assert!(rewrite(cmd, None).is_none(), "{cmd}");
}

#[test]
fn captured_segments_run_unchanged() {
    for cmd in [
        "{ echo; cat a.md; } > b",
        "( echo; cat a.md ) > b",
        "{ echo; cat a.md; } | wc -l",
        "if true; then echo; cat a.md; fi > b",
        "for i in 1; do echo; cat a.md; done > c",
        "{ echo; head -3 a.md; } > b",
        "{ echo; tail -n 3 a.md; } > b",
        "{ echo; grep x a.rs; } > b",
        "{ cat a.md; } > b",
        "diff <(cat a.md) f",
        "diff <(cat a.md) <(grep x a.rs)",
        "cat a.md | tee >(cat a.rs)",
        "x=$(cat a.md)",
        "x=`cat a.md`",
        "y=$(head -2 a.md)",
        "z=$(grep x a.rs)",
        "x=$(cd d && cat a.md)",
        "cat a.md >&2",
        "cat a.md &> b",
        "cat a.md &>> b",
        "cat a.md >| b",
        "cat a.md >> b",
        "cat a.md > b",
        "cat a.md 1>b",
        "cat a.md 1>&2",
        "grep x a.rs > b",
        "head -3 a.md > b",
        "tail -n 3 a.md >b",
        "gh release view v1 --json body --jq .body > old.md && { cat <<'EOF'\nnew\nEOF\ncat old.md; } > new.md && gh release edit v1 --notes-file new.md",
        "cat > x.sh <<'EOF'\ngrep foo a.md\nEOF",
    ] {
        runs_unchanged(cmd);
    }
}

/// Inside a loop or group the output still reaches the agent when nothing captures it: a read or
/// search there is hinted, never refused or rewritten.
#[test]
fn loop_reads_get_a_hint_because_their_output_reaches_the_agent() {
    for cmd in [
        "{ cat a.md; }",
        "{ { cat a.md; } > b; cat a.rs; }",
        "while read l; do cat a.md; done < list",
        "case $x in a) cat a.md ;; esac",
    ] {
        assert!(
            matches!(check(cmd), Some(Verdict::Hint(Rule::ReadShell, _))),
            "{cmd}"
        );
        assert!(rewrite(cmd, None).is_none(), "{cmd}");
    }
    assert!(matches!(
        check("for f in $(git ls-files); do grep x $f; done"),
        Some(Verdict::Hint(Rule::Grep, _))
    ));
    assert!(matches!(
        check("for f in a.rs b.rs; do head -3 a.rs; done"),
        Some(Verdict::Hint(Rule::ReadShell, hint)) if hint.contains("acus show 'a.rs:1-3'")
    ));
}

/// Git and other refusals inside a loop or group are hints too, and the command still runs.
#[test]
fn loop_refusals_and_git_hints_never_refuse_or_rewrite() {
    for cmd in [
        "{ git diff -- a.rs; }",
        "for f in a b; do git diff -- $f; done",
        "for f in a b; do sed -i s/x/y/ $f; done",
    ] {
        assert!(matches!(check(cmd), Some(Verdict::Hint(_, _))), "{cmd}");
        assert!(rewrite(cmd, None).is_none(), "{cmd}");
    }
    assert!(matches!(
        check("{ git diff -- a.rs; }"),
        Some(Verdict::Hint(Rule::GitDiff, hint)) if hint.starts_with("acus hint: `acus diff")
    ));
}

#[test]
fn segments_outside_captured_ones_still_convert() {
    assert_eq!(
        rw("{ echo; cat a.md; } > b; cat a.md", None).as_deref(),
        Some("{ echo; cat a.md; } > b; acus show 'a.md'")
    );
    assert_eq!(
        rw("if true; then echo; fi; cat a.md", None).as_deref(),
        Some("if true; then echo; fi; acus show 'a.md'")
    );
    assert_eq!(
        rw("cat a.md 2>/dev/null", None).as_deref(),
        Some("acus show 'a.md' 2>/dev/null")
    );
    // The segment outside a substitution is still refused (and rewritten, see the ast tests).
    assert!(matches!(
        check("x=$(cat a.md); cat a.md"),
        Some(Verdict::Deny(Rule::ReadShell, _))
    ));
    assert!(matches!(
        check("cat a.md 2>/dev/null"),
        Some(Verdict::Deny(Rule::ReadShell, _))
    ));
}
