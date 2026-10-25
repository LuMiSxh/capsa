// SPDX-License-Identifier: MPL-2.0

use super::*;

#[test]
fn quiets_an_acus_patch_whose_last_head_or_tail_stage_discards_its_output() {
    for (cmd, want) in [
        ("acus patch | tail -3", "acus patch -q"),
        ("acus patch | head -n 5", "acus patch -q"),
        (
            "acus patch --file p.txt 2>&1 | tail -5",
            "acus patch -q --file p.txt 2>&1",
        ),
        (
            "cd X && acus patch --file p.txt | tail -5",
            "cd X && acus patch -q --file p.txt",
        ),
        ("acus patch < p.txt | tail -3", "acus patch -q < p.txt"),
        ("acus patch -q | tail -3", "acus patch -q"),
        (
            "acus patch <<'PATCH' | tail -3\n*** Begin Patch\n*** End Patch\nPATCH\n",
            "acus patch -q <<'PATCH'\n*** Begin Patch\n*** End Patch\nPATCH\n",
        ),
        (
            "cd X; acus patch <<'PATCH' | tail -5\n*** Begin Patch\n*** End Patch\nPATCH\n",
            "cd X; acus patch -q <<'PATCH'\n*** Begin Patch\n*** End Patch\nPATCH\n",
        ),
    ] {
        assert_quiet_rewrite(cmd, want);
    }
}

#[test]
fn quiets_an_acus_patch_sent_to_dev_null_and_keeps_stderr_visible() {
    for (cmd, want) in [
        ("acus patch < p.txt > /dev/null", "acus patch -q < p.txt"),
        ("acus patch < p.txt >/dev/null", "acus patch -q < p.txt"),
        (
            "acus patch < p.txt >/dev/null 2>&1",
            "acus patch -q < p.txt",
        ),
        (
            "acus patch < p.txt > /dev/null 2>&1",
            "acus patch -q < p.txt",
        ),
        ("acus patch < p.txt &>/dev/null", "acus patch -q < p.txt"),
        ("acus patch < p.txt &> /dev/null", "acus patch -q < p.txt"),
        (
            "cd X && acus patch --file p.txt >/dev/null; echo done",
            "cd X && acus patch -q --file p.txt; echo done",
        ),
        (
            "acus patch <<'PATCH' >/dev/null\n*** Begin Patch\n*** End Patch\nPATCH\n",
            "acus patch -q <<'PATCH'\n*** Begin Patch\n*** End Patch\nPATCH\n",
        ),
    ] {
        assert_quiet_rewrite(cmd, want);
    }
}

/// Checks that `cmd` is rewritten to `want` as a quiet patch, deferred to the permission flow
/// and not refused.
fn assert_quiet_rewrite(cmd: &str, want: &str) {
    let r = rewrite(cmd, None).unwrap_or_else(|| panic!("not rewritten: {cmd}"));
    assert_eq!(r.command, want, "{cmd}");
    assert_eq!(r.rule, Rule::PatchQuiet, "{cmd}");
    assert!(r.deferred, "{cmd}");
    assert!(
        r.note.contains(
            "-q prints only the summary and errors; the patch output was being discarded"
        ),
        "{cmd}: {}",
        r.note
    );
    assert!(!matches!(check(cmd), Some(Verdict::Deny(..))), "{cmd}");
}

#[test]
fn leaves_acus_patch_output_that_goes_elsewhere_as_it_is() {
    for cmd in [
        "acus patch | grep x",
        "acus patch | wc -l",
        "acus patch | tee log.txt | tail -3",
        "acus patch | tail -3 | wc -l",
        "acus patch | tail -3 > log.txt",
        "acus patch > log.txt",
        "acus patch --file p.txt 2>/dev/null",
        "acus patch 2>&1 >/dev/null",
        "acus patch --file p.txt 2>/dev/null >/dev/null",
        "x=$(acus patch --file p.txt | tail -3)",
        "echo $(acus patch --file p.txt >/dev/null)",
        "{ acus patch --file p.txt | tail -3; } > log.txt",
        "(acus patch --file p.txt >/dev/null)",
    ] {
        assert!(
            rewrite(cmd, None).is_none_or(|r| r.rule != Rule::PatchQuiet),
            "{cmd}"
        );
    }
}

#[test]
fn an_acus_patch_that_is_already_quiet_is_a_fixed_point() {
    for cmd in [
        "acus patch -q",
        "acus patch --quiet --file p.txt",
        "acus patch -q <<'PATCH'\n*** End Patch\nPATCH\n",
    ] {
        assert!(rewrite(cmd, None).is_none(), "{cmd}");
    }
    for cmd in [
        "acus patch -q | tail -3",
        "acus patch --quiet >/dev/null",
        "acus patch | head -5",
        "acus patch < p.txt >/dev/null 2>&1",
    ] {
        let once = rewrite(cmd, None)
            .unwrap_or_else(|| panic!("not rewritten: {cmd}"))
            .command;
        assert!(rewrite(&once, None).is_none(), "{cmd} -> {once}");
    }
}
