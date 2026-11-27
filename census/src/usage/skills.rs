// SPDX-License-Identifier: MPL-2.0
//! Owns the skill-use statistics: `Skill` tool calls and skill text printed through acus or census.

use super::FileStats;
use serde::Serialize;
use std::collections::{BTreeSet, HashMap, HashSet};

/// One skill, used through the `Skill` tool or printed from a tool's own binary.
#[derive(Debug, Default, Clone, Serialize)]
pub struct SkillStat {
    pub skill: String,
    /// `tool` for a `Skill` call, `print` for `acus self skill` or `census self skill`.
    pub source: &'static str,
    pub calls: u64,
    pub sessions: u64,
    pub roles: Vec<String>,
    pub result_chars: u64,
}

#[derive(Default)]
pub(super) struct SkillAcc {
    calls: u64,
    sessions: HashSet<String>,
    roles: BTreeSet<String>,
    result_chars: u64,
}

/// Adds the skill calls of one transcript. `sessions` counts a session once per skill.
pub(super) fn add_file(acc: &mut HashMap<(String, &'static str), SkillAcc>, f: &FileStats) {
    for c in &f.calls {
        let Some((skill, source)) = &c.skill else {
            continue;
        };
        let a = acc.entry((skill.clone(), *source)).or_default();
        a.calls += 1;
        a.sessions.insert(f.session.clone());
        a.roles.insert(f.role.clone());
        a.result_chars += c.result_chars;
    }
}

pub(super) fn finish(acc: HashMap<(String, &'static str), SkillAcc>) -> Vec<SkillStat> {
    let mut rows: Vec<SkillStat> = acc
        .into_iter()
        .map(|((skill, source), a)| SkillStat {
            skill,
            source,
            calls: a.calls,
            sessions: a.sessions.len() as u64,
            roles: a.roles.into_iter().collect(),
            result_chars: a.result_chars,
        })
        .collect();
    rows.sort_by(|a, b| {
        b.calls
            .cmp(&a.calls)
            .then(a.skill.cmp(&b.skill))
            .then(a.source.cmp(b.source))
    });
    rows
}
