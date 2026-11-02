// SPDX-License-Identifier: MPL-2.0
//! Owns the shared doctor model: checks with a status, grouped and rendered as one view.

use std::borrow::Cow;

use serde::Serialize;

use super::{Doc, Render};
use crate::exit::Outcome;

/// How a check came out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Ok,
    Warn,
    Fail,
}

/// One check: its name, status, what it found and, when it failed, a fix.
#[derive(Debug, Clone, Serialize)]
pub struct Check {
    pub name: Cow<'static, str>,
    pub status: Status,
    pub detail: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

/// A named set of checks, such as `setup`.
#[derive(Debug, Clone, Serialize)]
pub struct Group {
    pub name: &'static str,
    pub checks: Vec<Check>,
}

/// The result of `<tool> self doctor`.
#[derive(Debug, Clone, Serialize)]
pub struct Doctor {
    pub tool: &'static str,
    pub version: &'static str,
    pub ok: bool,
    pub groups: Vec<Group>,
}

impl Doctor {
    /// Builds the result, with `ok` set when no check failed.
    pub fn new(tool: &'static str, version: &'static str, groups: Vec<Group>) -> Self {
        let ok = groups
            .iter()
            .flat_map(|group| &group.checks)
            .all(|check| check.status != Status::Fail);
        Self {
            tool,
            version,
            ok,
            groups,
        }
    }
}

impl Render for Doctor {
    fn doc(&self, doc: &mut Doc) {
        doc.title(format!("{} self doctor", self.tool));
        for group in &self.groups {
            doc.section(group.name);
            for check in &group.checks {
                doc.check(check);
            }
        }
    }

    fn outcome(&self) -> Outcome {
        if self.ok {
            Outcome::Found
        } else {
            Outcome::Empty
        }
    }
}
