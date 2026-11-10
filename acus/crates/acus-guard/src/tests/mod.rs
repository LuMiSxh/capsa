// SPDX-License-Identifier: MPL-2.0
//! Guard tests, grouped by what they cover. They call private functions, so they live here.
//! The modules that read Bash need a Bash-like grammar (`bash` or `zsh`).

#[cfg(any(feature = "bash", feature = "zsh"))]
mod ast;
mod backends;
#[cfg(any(feature = "bash", feature = "zsh"))]
mod bash;
#[cfg(any(feature = "bash", feature = "zsh"))]
mod captured;
#[cfg(any(feature = "bash", feature = "zsh"))]
mod conversions;
mod corpus;
#[cfg(any(feature = "bash", feature = "zsh"))]
mod file_write;
#[cfg(any(feature = "bash", feature = "zsh"))]
mod folds;
mod never_panics;
mod powershell;
#[cfg(any(feature = "bash", feature = "zsh"))]
mod rewrite;
mod tools;

use super::*;
use acus_syntax::Lang;
use serde_json::json;
use std::path::Path;

/// Returns the reason of a refused command.
fn deny(v: Option<Verdict>) -> (&'static str, String) {
    match v {
        Some(Verdict::Deny(r, reason)) => (r.as_str(), reason),
        other => panic!("{other:?}"),
    }
}

/// Returns the command `rewrite` gives for `cmd` under `scope`, or `None` when the rules leave it
/// as it is.
#[cfg(any(feature = "bash", feature = "zsh"))]
fn rw(cmd: &str, scope: Option<&Scope>) -> Option<String> {
    rewrite(cmd, scope).map(|r| r.command)
}

/// Checks that each `cmd` is rewritten to its `want` under `scope`.
#[cfg(any(feature = "bash", feature = "zsh"))]
fn assert_rewrites(cases: &[(&str, &str)], scope: Option<&Scope>) {
    for (cmd, want) in cases {
        assert_eq!(rw(cmd, scope).as_deref(), Some(*want), "{cmd}");
    }
}
