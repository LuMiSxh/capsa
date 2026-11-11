// SPDX-License-Identifier: MPL-2.0
//! Turns the state of a hook into a doctor check.

use std::borrow::Cow;

use super::{HookSpec, HookState};
use crate::render::{Check, Status};

impl HookState {
    /// Returns the doctor check for the hook `spec` in this state.
    ///
    /// An installed hook, or one under a matcher the user chose, is ok and names its matcher. An
    /// outdated or missing hook warns with `run <setup>` as the fix, where `setup` is the tool's
    /// setup command, such as `acus self setup`.
    pub fn check(&self, spec: &HookSpec, setup: &str) -> Check {
        let name = Cow::Owned(format!("{} hook", spec.event));
        let fix = Some(format!("run {setup}"));
        let (status, detail, hint) = match self {
            Self::Installed => (Status::Ok, scope(spec.matcher, spec.event), None),
            Self::CustomMatcher { matcher } => {
                (Status::Ok, scope(matcher.as_deref(), spec.event), None)
            }
            Self::Outdated { command } => (Status::Warn, format!("outdated: {command}"), fix),
            Self::Missing => (Status::Warn, "not registered".to_owned(), fix),
        };
        Check {
            name,
            status,
            detail,
            hint,
        }
    }
}

fn scope(matcher: Option<&str>, event: &str) -> String {
    match matcher {
        Some(matcher) => format!("matcher {matcher:?}"),
        None => format!("every {event}"),
    }
}
