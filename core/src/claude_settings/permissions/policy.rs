// SPDX-License-Identifier: MPL-2.0
//! What the permission mode asks of a call that no deny or ask rule names.
//!
//! | mode | Bash, Edit | fetch, MCP | Read |
//! |------|-----------|------------|------|
//! | none, and `bypassPermissions` | goes | goes | goes |
//! | `plan` | refused | goes | goes |
//! | `dontAsk` | needs an allow rule | needs an allow rule | goes |
//! | `default`, `auto`, `acceptEdits` | goes, or with `strict` as in `dontAsk` | same | goes |
//!
//! With `strict`, `acceptEdits` lets an edit inside the working directory go without a rule, and a
//! mode that is not known counts as `default`. In every mode but `bypassPermissions`, and whether
//! or not `strict` is set, an edit of a protected path (see [`Scope::Protected`]) needs an allow
//! rule; with no mode known, only deny and ask rules apply.

use super::{Mode, Permissions, Refusal, Tool};

/// Where a call reaches, for the modes that treat places differently.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Scope {
    /// Outside the working directory, or not a path at all.
    Elsewhere,
    /// Inside the working directory.
    Local,
    /// `.git/`, or a settings file the rules are read from: editing it changes what the agent may
    /// do, so no mode's leniency covers it.
    Protected,
}

/// What a call needs before it may go ahead.
enum Need {
    Nothing,
    Allow,
    Never,
}

impl Permissions {
    fn need(&self, tool: Tool, scope: Scope) -> Need {
        let Some(mode) = self.mode.or(self.strict.then_some(Mode::Default)) else {
            return Need::Nothing;
        };
        match (mode, tool) {
            (Mode::BypassPermissions, _) | (_, Tool::Read) => Need::Nothing,
            (Mode::Plan, Tool::Bash | Tool::Edit) => Need::Never,
            (Mode::Plan, _) => Need::Nothing,
            (Mode::DontAsk, _) => Need::Allow,
            (_, Tool::Edit) if scope == Scope::Protected => Need::Allow,
            (Mode::AcceptEdits, Tool::Edit) if self.strict && scope == Scope::Local => {
                Need::Nothing
            }
            _ if self.strict => Need::Allow,
            _ => Need::Nothing,
        }
    }

    /// Returns the mode's refusal of a call to `what`, unless it may go ahead. `allowed` says
    /// whether an allow rule covers the call and is asked only when the mode needs one.
    pub(super) fn gate(
        &self,
        tool: Tool,
        scope: Scope,
        what: &str,
        allowed: impl FnOnce() -> bool,
    ) -> Result<(), Refusal> {
        match self.need(tool, scope) {
            Need::Nothing => Ok(()),
            Need::Allow if allowed() => Ok(()),
            Need::Allow => Err(self.no_allow_rule(what, scope)),
            Need::Never => Err(
                Refusal::new(format!("{what} refused in plan mode")).with_hint(
                    "plan mode is read-only for scripts; run a read-only command as its own tool \
                 call, or leave plan mode to change files",
                ),
            ),
        }
    }

    fn no_allow_rule(&self, what: &str, scope: Scope) -> Refusal {
        if scope == Scope::Protected {
            return Refusal::new(format!(
                "{what} refused: a protected path needs an allow rule"
            ))
            .with_hint("protected path; run the edit as its own tool call");
        }
        if self.mode == Some(Mode::DontAsk) {
            return Refusal::new(format!(
                "{what} refused in dontAsk mode: no allow rule covers it"
            ))
            .with_hint(
                "dontAsk mode denies what no allow rule covers; add one to permissions.allow",
            );
        }
        let mode = self.mode.map_or_else(
            || "an unknown mode".to_owned(),
            |m| format!("{} mode", m.as_str()),
        );
        Refusal::new(format!(
            "{what} refused in {mode} with strict set: no allow rule covers it"
        ))
        .with_hint("no allow rule; add one or run it as its own tool call")
    }
}
