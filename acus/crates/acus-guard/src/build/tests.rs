// SPDX-License-Identifier: MPL-2.0

use crate::rewrite;

// The helpers of `tests/mod.rs` are private to that module, so the same two lines appear here.
fn rw(cmd: &str) -> Option<String> {
    rewrite(cmd, None).map(|r| r.command)
}

fn assert_rewrites(cases: &[(&str, &str)]) {
    for (cmd, want) in cases {
        assert_eq!(rw(cmd).as_deref(), Some(*want), "{cmd}");
    }
}

#[test]
fn folds_build_pipes_into_run_flags() {
    assert_rewrites(&[
        (
            "gh run view 123 --log-failed | grep error | tail -20",
            "acus run 'gh run view 123 --log-failed' --grep 'error' --tail 20",
        ),
        (
            "gh run view 123 --log | tail -5",
            "acus run 'gh run view 123 --log' --tail 5",
        ),
        (
            "timeout 300 cargo test 2>&1 | tail -30",
            "acus run 'cargo test' --timeout 300 --tail 30",
        ),
        (
            "gtimeout 2h cargo test | tail -3",
            "acus run 'cargo test' --timeout 7200 --tail 3",
        ),
        (
            "timeout 5m cargo build | grep -E 'error|warning'",
            "acus run 'cargo build' --timeout 300 --grep 'error|warning'",
        ),
        (
            "cargo test 2>&1 | grep '<T>' | tail -5",
            "acus run 'cargo test' --grep '<T>' --tail 5",
        ),
        // A head that keeps every tail line folds away.
        (
            "acus run 'cargo test' --tail 20 | head -25",
            "acus run 'cargo test' --tail 20",
        ),
        (
            "acus run 'cargo test' --tail 20 | head -20",
            "acus run 'cargo test' --tail 20",
        ),
        (
            "cargo test 2>&1 | tail -20 | head -25",
            "acus run 'cargo test' --tail 20",
        ),
        (
            "cargo test 2>&1 | tail -20 | head -20; echo hi",
            "acus run 'cargo test' --tail 20; echo hi",
        ),
    ]);
}

#[test]
fn keeps_the_old_verdict_where_a_timeout_or_gh_fold_does_not_apply() {
    for cmd in [
        // Filters that no run flag can express.
        "gh run list --limit 4 2>&1 | head; gh run view 7 --log-failed 2>&1 | grep -E a -A5 | grep -v b | head -4",
        "timeout 590 cargo clippy --workspace 2>&1 | grep -E \"^(warning|error)\" -A4 | grep -v b | head -9",
        "pkill -f foo; cd d && echo x >> n.md; timeout 590 cargo clippy 2>&1 | grep -E \"^e\" -A4 | grep -v b | head -9",
        "timeout 300 cargo test -p foo 2>&1 | grep -E '^error' | sort | uniq -c; echo done",
        // A `$` that the rewritten text would expand twice.
        "id=$(gh run list -L 1 -q x); gh run view $id --log-failed 2>&1 | grep -E 'a|b' | head -30",
        "gh run view $(gh run list -L1 -q x) --log 2>/dev/null | grep -iE 'a|b' | head -15",
        "cd ~/p && timeout 900 make -j\"$(sysctl -n hw.logicalcpu)\" all 2>&1 | tail -5; ls",
    ] {
        assert!(crate::check(cmd).is_none(), "{cmd}");
    }
    // A build that folds is still refused, and rewritten.
    assert!(crate::check("timeout 300 cargo test 2>&1 | tail -3").is_some());
}

#[test]
fn head_zero_after_a_build_folds_as_before() {
    assert_eq!(
        rw("swift build -q 2>&1 | head -n 0; echo hi").as_deref(),
        Some("acus run 'swift build -q' --max-output 2; echo hi")
    );
    assert!(crate::check("swift build -q 2>&1 | head -n 0").is_some());
}

#[test]
fn leaves_other_gh_and_timeout_commands() {
    for cmd in [
        "gh run view 123 | tail -5",
        "timeout -k 5 300 cargo test | tail -3",
        "timeout 300 grep x f | tail -3",
        "timeout 0 cargo test | tail -3",
        "timeout 5x cargo test | tail -3",
        "cargo test | grep x > out.txt",
        "cargo test | grep x >out",
    ] {
        let got = rw(cmd);
        assert!(
            got.as_deref()
                .is_none_or(|s| !s.contains("--timeout") && !s.starts_with("acus run 'gh")),
            "{cmd}: {got:?}"
        );
    }
}

#[test]
fn names_every_filter_that_blocks_the_ctx_form() {
    for (cmd, want) in [
        (
            "cargo test 2>&1 | grep FAIL | tail -n +5 | grep -c x",
            "no run flag does `tail`, `grep`,",
        ),
        (
            "acus run 'cargo test' | tail -n +5 | grep x | tail -n +2",
            "no run flag does `tail`, so",
        ),
    ] {
        let Some(crate::Verdict::Deny(_, reason)) = crate::check(cmd) else {
            panic!("{cmd}")
        };
        assert!(reason.contains(want), "{cmd}: {reason}");
    }
}

#[test]
fn chains_that_cannot_fold_stay_refused() {
    for cmd in [
        "acus run 'cargo test' --tail 20 | head -5",
        "cargo test 2>&1 | tail -20 | head -5",
        "cargo test 2>&1 | tail -n +5 | grep x",
        "acus run 'cargo test' | tail -n +5 | grep x",
        "cargo test 2>&1 | grep -A2 error | grep -v warn",
    ] {
        assert!(rw(cmd).is_none(), "{cmd}");
        assert!(
            matches!(crate::check(cmd), Some(crate::Verdict::Deny(..))),
            "{cmd}"
        );
    }
}

#[test]
fn chains_that_no_run_flag_expresses_run_as_written() {
    for cmd in [
        "cargo test 2>&1 | grep FAIL | sort | uniq -c",
        "npm run build | sort -u",
        "cargo test 2>&1 | sort | uniq",
        "acus run 'cargo test' | grep FAIL | sort | uniq",
        "acus run 'cargo test' | grep a | grep b",
        "cargo test 2>&1 | grep a | grep b",
        "cargo test | awk '{print $1}'",
        "acus run 'cargo test' | awk '{print $1}'",
    ] {
        assert!(crate::check(cmd).is_none(), "{cmd}");
        assert!(rw(cmd).is_none(), "{cmd}");
    }
}

// Stacked greps that one run flag set expresses still fold: `grep -v` becomes `--drop`.
#[test]
fn stacked_greps_that_fold_keep_their_rewrite() {
    let cmd = "acus run 'cargo test' | grep -v x | grep y";
    assert!(
        matches!(crate::check(cmd), Some(crate::Verdict::Deny(..))),
        "{cmd}"
    );
    assert_eq!(
        rw(cmd).as_deref(),
        Some("acus run 'cargo test' --drop 'x' --grep 'y'")
    );

    let cmd = "cargo test 2>&1 | grep -v x | grep y";
    assert!(
        matches!(crate::check(cmd), Some(crate::Verdict::Deny(..))),
        "{cmd}"
    );
    assert_eq!(
        rw(cmd).as_deref(),
        Some("acus run 'cargo test' --drop 'x' --grep 'y'")
    );
}

#[test]
fn a_send_line_keeps_every_stage_of_the_chain() {
    for (cmd, want) in [
        (
            "xcodebuild build 2>&1 | grep -E 'error|BUILD' | cut -c1-150 | tail -4",
            "acus run 'xcodebuild build' --grep 'error|BUILD' --tail 4 | cut -c1-150",
        ),
        (
            "acus run 'xcodebuild build' | grep -E 'error|BUILD' | cut -c1-150 | tail -4",
            "acus run 'xcodebuild build' --grep 'error|BUILD' --tail 4 | cut -c1-150",
        ),
        (
            "timeout 300 xcodebuild build 2>&1 | grep -E 'error' | cut -c1-150 | tail -4",
            "acus run 'xcodebuild build' --timeout 300 --grep 'error' --tail 4 | cut -c1-150",
        ),
    ] {
        let Some(crate::Verdict::Deny(_, reason)) = crate::check(cmd) else {
            panic!("not refused: {cmd}");
        };
        assert!(
            reason.contains(&format!("Send instead: {want}")),
            "{reason}"
        );
    }
    // A stage that no run flag does, or one that is not an output filter, names no command.
    for cmd in [
        "xcodebuild build 2>&1 | grep -E 'error' | awk '{print $1}'",
        "xcodebuild build 2>&1 | grep -E 'error' | cut -c1-150 | awk '{print $1}'",
        "xcodebuild build 2>&1 | grep -E 'error' | cut -c1-150 | grep x",
        "xcodebuild build 2>&1 | grep -E 'error' | tr a b | tail -4",
    ] {
        if let Some(crate::Verdict::Deny(_, reason)) = crate::check(cmd) {
            assert!(!reason.contains("Send instead"), "{cmd}: {reason}");
        }
    }
}
