// SPDX-License-Identifier: MPL-2.0
//! Owns the report of `self doctor --fix`: the checks run again after the repairs, then the
//! repairs made.

use std::path::Path;

use serde::Serialize;

use super::ok;
use crate::exit::Outcome;
use crate::format::Format;
use crate::hint::ResultExt;
use crate::hooks::{HookFile, HookSpec, HookState};
use crate::render::{Check, Doc, Doctor, Render, emit};
use crate::skill::{self, Skill, SkillState, Target};

/// The doctor report with the repairs made before it ran, as `fixed` in JSON.
#[derive(Debug, Serialize)]
pub struct Repaired<'a> {
    #[serde(flatten)]
    pub report: &'a Doctor,
    pub fixed: Vec<Check>,
}

impl Render for Repaired<'_> {
    fn doc(&self, doc: &mut Doc) {
        self.report.doc(doc);
        doc.section("fixed");
        if self.fixed.is_empty() {
            doc.line("nothing to fix");
        }
        for check in &self.fixed {
            doc.check(check);
        }
    }

    fn outcome(&self) -> Outcome {
        self.report.outcome()
    }
}

/// Prints the doctor report, with a `fixed` section when `--fix` ran, and returns its outcome.
///
/// Without repairs (`fixed` is `None`) the output is the plain report, byte for byte.
pub fn emit_report(
    report: &Doctor,
    fixed: Option<Vec<Check>>,
    format: Format,
) -> anyhow::Result<Outcome> {
    match fixed {
        None => emit(report, format),
        Some(fixed) => emit(&Repaired { report, fixed }, format),
    }
}

/// Updates the skill for `target` when it is installed but outdated. A missing skill needs the
/// user's choice, so it keeps its hint. Returns the check for what was done, or `None` when the
/// skill was left alone.
pub fn repair_skill(
    home: &Path,
    skill: &Skill,
    target: Target,
    name: &'static str,
) -> anyhow::Result<Option<Check>> {
    if skill::state(home, skill, target) != SkillState::Outdated {
        return Ok(None);
    }
    skill::install(home, skill, target).hinted()?;
    let dirs = target
        .dirs(home, skill.tool)
        .iter()
        .map(|d| d.display().to_string())
        .collect::<Vec<_>>()
        .join(", ");
    Ok(Some(ok(name, format!("updated in {dirs}"))))
}

/// Adds the missing hooks of `specs` to `file` and replaces the outdated ones, returning one check
/// per hook written. An entry whose matcher the user changed is kept, since replacing it would drop
/// that matcher. The caller writes `file` when a check is returned.
pub fn repair_hooks(file: &mut HookFile, specs: &[HookSpec]) -> Vec<Check> {
    let mut checks = Vec::new();
    for spec in specs {
        if matches!(
            file.state(spec),
            HookState::Missing | HookState::Outdated { .. }
        ) {
            let step = file.set(spec);
            let detail = format!("{} in {}", step.text(false), file.path().display());
            checks.push(ok(format!("hook {}", spec.event), detail));
        }
    }
    checks
}
