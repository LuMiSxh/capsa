// SPDX-License-Identifier: MPL-2.0
//! `acus script save|list|show|log`: keeps the saved scripts and reports on them.

use super::bindings::Bindings;
use super::store::{self, Meta, Scope, Store, Version};
use super::usage;
use crate::config::Config;

use anyhow::{Context, Result};
use capsa_core::exit::Outcome;
use capsa_core::hint::hinted;
use capsa_core::render::{Align, Doc, Render, Table};
use serde::Serialize;
use serde_json::Value;
use std::io::Read;
use std::path::Path;

#[derive(clap::Args)]
pub struct SaveArgs {
    /// The script's name: lowercase letters, digits and `-`.
    name: String,
    /// What this version changes; it becomes the changelog entry.
    #[arg(long, value_name = "TEXT")]
    note: String,
    /// Save it for every repository instead of the current one.
    #[arg(long)]
    global: bool,
}

#[derive(clap::Args)]
pub struct ShowArgs {
    /// The script, `NAME` for the latest version or `NAME@3` for one version.
    spec: String,
}

#[derive(clap::Args)]
pub struct LogArgs {
    /// The script whose versions and notes to list.
    name: String,
}

/// The result of `script save`.
#[derive(Serialize)]
pub(super) struct Saved {
    name: String,
    scope: Scope,
    /// The new version, or the latest one when the content was unchanged.
    version: u32,
    created: bool,
}

impl Render for Saved {
    fn doc(&self, doc: &mut Doc) {
        let Saved {
            name,
            scope,
            version,
            created,
        } = self;
        let scope = scope.as_str();
        doc.line(if *created {
            format!("saved {name}@{version} ({scope})")
        } else {
            format!(
                "unchanged: {name}@{version} ({scope}) already has this content, no version added"
            )
        });
    }
}

pub(super) fn save(a: &SaveArgs, cfg: Config) -> Result<Saved> {
    store::check_name(&a.name)?;
    if a.note.trim().is_empty() {
        return Err(hinted(
            "--note is empty",
            "say what this version does or changes",
        ));
    }
    let mut source = String::new();
    std::io::stdin()
        .read_to_string(&mut source)
        .context("cannot read the script from stdin")?;
    let cwd = std::env::current_dir().context("cannot read the current directory")?;
    let store = Store::open(&cfg, &cwd);
    let limits = super::limits(&cfg);
    let info = acus_script::check_module(&source, Bindings::for_check(cfg)?, &limits)
        .map_err(super::failure)?;
    let scope = if a.global { Scope::User } else { Scope::Repo };
    let created = store.save(scope, &a.name, &source, a.note.trim(), &info)?;
    let version = created.unwrap_or_else(|| {
        let meta = store.meta(scope, &a.name);
        meta.and_then(|m| m.latest().map(|v| v.version))
            .unwrap_or(1)
    });
    Ok(Saved {
        name: a.name.clone(),
        scope,
        version,
        created: created.is_some(),
    })
}

#[derive(Serialize)]
struct Item {
    name: String,
    scope: Scope,
    version: u32,
    description: String,
    last_use: Option<u64>,
    uses: usize,
}

/// The result of `script list`.
#[derive(Serialize)]
pub(super) struct Listed {
    scripts: Vec<Item>,
}

impl Render for Listed {
    fn doc(&self, doc: &mut Doc) {
        let rows = self
            .scripts
            .iter()
            .map(|s| {
                vec![
                    s.name.clone(),
                    s.scope.as_str().to_owned(),
                    format!("v{}", s.version),
                    s.description.clone(),
                    s.last_use.map_or_else(|| "-".to_owned(), day),
                    s.uses.to_string(),
                ]
            })
            .collect();
        doc.table(Table {
            head: ["name", "scope", "latest", "description", "last use", "uses"]
                .map(str::to_owned)
                .to_vec(),
            align: vec![
                Align::Left,
                Align::Left,
                Align::Left,
                Align::Left,
                Align::Left,
                Align::Right,
            ],
            rows,
        });
    }

    fn outcome(&self) -> Outcome {
        if self.scripts.is_empty() {
            Outcome::Empty
        } else {
            Outcome::Found
        }
    }
}

pub(super) fn list(cfg: &Config, cwd: &Path) -> Listed {
    let store = Store::open(cfg, cwd);
    let root = store::repo_root(cwd);
    let runs = usage::runs(cfg);
    let mut scripts = Vec::new();
    for scope in [Scope::Repo, Scope::User] {
        for name in store.names(scope) {
            let Some(latest) = store.meta(scope, &name).and_then(|m| m.latest().cloned()) else {
                continue;
            };
            let mine: Vec<_> = runs
                .iter()
                .filter(|u| u.run.name == name && u.run.scope == scope)
                .filter(|u| {
                    scope == Scope::User || root.as_deref().is_some_and(|r| usage::under(&u.run, r))
                })
                .collect();
            scripts.push(Item {
                name,
                scope,
                version: latest.version,
                description: latest.description,
                last_use: mine.iter().map(|u| u.ts).max(),
                uses: mine.len(),
            });
        }
    }
    Listed { scripts }
}

/// The result of `script show`.
#[derive(Serialize)]
pub(super) struct Shown {
    name: String,
    scope: Scope,
    version: u32,
    description: String,
    params: Value,
    source: String,
}

impl Render for Shown {
    fn doc(&self, doc: &mut Doc) {
        doc.title(format!(
            "{}@{} ({}): {}",
            self.name,
            self.version,
            self.scope.as_str(),
            self.description
        ));
        doc.line(format!("params: {}", self.params));
        doc.line(self.source.trim_end());
    }
}

pub(super) fn show(a: &ShowArgs, cfg: &Config, cwd: &Path) -> Result<Shown> {
    let (name, version) = store::parse_spec(&a.spec)?;
    let store = Store::open(cfg, cwd);
    let found = store.locate(&name, version)?;
    let meta = store.meta(found.scope, &name).unwrap_or_default();
    let recorded = meta.versions.iter().find(|v| v.version == found.version);
    Ok(Shown {
        source: store.source(found.scope, &name, found.version)?,
        description: recorded.map(|v| v.description.clone()).unwrap_or_default(),
        params: recorded.map(|v| v.params.clone()).unwrap_or_default(),
        name,
        scope: found.scope,
        version: found.version,
    })
}

/// The result of `script log`.
#[derive(Serialize)]
pub(super) struct History {
    name: String,
    scope: Scope,
    versions: Vec<Version>,
}

impl Render for History {
    fn doc(&self, doc: &mut Doc) {
        for v in self.versions.iter().rev() {
            doc.line(format!("v{}  {}  {}", v.version, day(v.at), v.note));
        }
    }
}

pub(super) fn history(a: &LogArgs, cfg: &Config, cwd: &Path) -> Result<History> {
    let (name, _) = store::parse_spec(&a.name)?;
    let found = Store::open(cfg, cwd).locate(&name, None)?;
    let meta: Meta = Store::open(cfg, cwd)
        .meta(found.scope, &name)
        .unwrap_or_default();
    Ok(History {
        name,
        scope: found.scope,
        versions: meta.versions,
    })
}

/// Formats unix seconds as `YYYY-MM-DD` (UTC).
fn day(ts: u64) -> String {
    let days = i64::try_from(ts / 86_400).unwrap_or_default();
    let (y, m, d) = crate::cmd_blame::civil(days);
    format!("{y:04}-{m:02}-{d:02}")
}
