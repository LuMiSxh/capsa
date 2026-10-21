// SPDX-License-Identifier: MPL-2.0
//! Golden replay of `corpus.jsonl`: each case's decision on one line, so a snapshot diff shows
//! which decisions a guard change flips.

use super::*;
use crate::replay::{Decision, decide};
use std::fmt::Write;

/// Returns the decision of the PowerShell tool, which never rewrites.
fn decide_ps(cmd: &str, scope: Option<&Scope>) -> Decision {
    check_ps(cmd, scope).into()
}

#[test]
fn corpus_decisions_are_golden() {
    // A fixed scope has a fixed home, which the golden writes as `~`: the same on every OS,
    // whatever HOME is.
    let home = home_dir(Some(&Scope::fixed("/"))).unwrap();
    let have = Grammars {
        bash: cfg!(feature = "bash"),
        zsh: cfg!(feature = "zsh"),
        powershell: cfg!(feature = "powershell"),
    };
    let (mut cases, mut skipped) = (Vec::new(), 0);
    for (i, text) in include_str!("corpus.jsonl").lines().enumerate() {
        let v: Value = serde_json::from_str(text).unwrap();
        let dialect = v["dialect"].as_str().unwrap_or("bash");
        let built = match dialect {
            "bash" => have.bash,
            "zsh" => have.zsh,
            "powershell" => have.powershell,
            other => panic!("case {}: unknown dialect {other}", i + 1),
        };
        if !built {
            skipped += 1;
            continue;
        }
        let scope = Scope::fixed(v["cwd"].as_str().unwrap());
        let cmd = v["cmd"].as_str().unwrap();
        let d = match dialect {
            "zsh" => tests_shell::with(Some("zsh"), || decide(cmd, Some(&scope))),
            "powershell" => decide_ps(cmd, Some(&scope)),
            _ => decide(cmd, Some(&scope)),
        };
        let rewritten = d
            .rewritten
            .unwrap_or_default()
            .replace(&home, "~")
            .replace('\n', "\\n");
        let line = format!(
            "{:04} {} {} ⇒ {rewritten}",
            i + 1,
            d.kind,
            d.rule.unwrap_or("-")
        );
        cases.push(line.trim_end().to_owned());
    }
    if skipped == 0 {
        let mut out = String::from("skipped 0 cases (grammar not built)\n");
        for line in &cases {
            writeln!(out, "{line}").unwrap();
        }
        insta::assert_snapshot!(out);
        return;
    }
    // A partial build: every case it ran must have the golden decision.
    let golden =
        include_str!("snapshots/acus_guard__tests__corpus__corpus_decisions_are_golden.snap");
    let golden: Vec<&str> = golden.lines().collect();
    for line in &cases {
        assert!(golden.contains(&line.as_str()), "not in the golden: {line}");
    }
}
