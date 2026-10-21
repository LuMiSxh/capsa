// SPDX-License-Identifier: MPL-2.0
#![cfg(any(feature = "bash", feature = "zsh"))]
use acus_guard::{BashDecision, decide_bash};

#[test]
fn public_api_rewrites_reads_and_passes_plain_commands() {
    match decide_bash("cat src/lib.rs", None, true) {
        BashDecision::Rewrite(r) => assert!(r.command.starts_with("acus show "), "{}", r.command),
        other => panic!("{other:?}"),
    }
    assert!(matches!(
        decide_bash("echo hi", None, true),
        BashDecision::Pass
    ));
    assert!(matches!(
        decide_bash("cat src/lib.rs", None, false),
        BashDecision::Verdict(_)
    ));
}
