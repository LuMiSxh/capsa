// SPDX-License-Identifier: MPL-2.0
//! Owns where saved scripts live and how a version is stored: `NAME/vN.js` files, immutable, and
//! a `meta.json` listing them. Repo scripts sit in `<repo root>/.acus/scripts`, global scripts
//! beside the config file.

use crate::config::Config;
use anyhow::{Context, Result};
use capsa_core::hint::hinted;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io::Write;
use std::path::{Path, PathBuf};

/// The words that name a script action, so no script can be called by one of them.
pub(super) const RESERVED: [&str; 4] = ["save", "list", "show", "log"];

/// Where a saved script lives.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(super) enum Scope {
    Repo,
    User,
}

impl Scope {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Scope::Repo => "repo",
            Scope::User => "user",
        }
    }
}

/// One saved version: what `meta.json` records about `vN.js`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct Version {
    pub(super) version: u32,
    /// Unix seconds.
    pub(super) at: u64,
    pub(super) note: String,
    pub(super) description: String,
    pub(super) params: Value,
}

#[derive(Default, Serialize, Deserialize)]
pub(super) struct Meta {
    pub(super) versions: Vec<Version>,
}

impl Meta {
    pub(super) fn latest(&self) -> Option<&Version> {
        self.versions.last()
    }
}

/// A script found by name: its scope and the version to run.
pub(super) struct Located {
    pub(super) scope: Scope,
    pub(super) version: u32,
}

/// The two script directories visible from the current directory.
#[derive(Clone)]
pub(super) struct Store {
    repo: Option<PathBuf>,
    user: PathBuf,
}

/// The git repository root above `cwd`, or `cwd` itself inside none.
pub(super) fn repo_root(cwd: &Path) -> Option<PathBuf> {
    cwd.ancestors()
        .find(|dir| dir.join(".git").exists())
        .map(Path::to_path_buf)
}

impl Store {
    pub(super) fn open(cfg: &Config, cwd: &Path) -> Self {
        Self::new(cfg, repo_root(cwd).as_deref())
    }

    pub(super) fn new(cfg: &Config, repo_root: Option<&Path>) -> Self {
        Store {
            repo: repo_root.map(|root| root.join(".acus/scripts")),
            user: cfg.path.with_file_name("scripts"),
        }
    }

    fn root(&self, scope: Scope) -> Option<&Path> {
        match scope {
            Scope::Repo => self.repo.as_deref(),
            Scope::User => Some(&self.user),
        }
    }

    /// Returns the history of `name` in `scope`, or `None` when it has none.
    pub(super) fn meta(&self, scope: Scope, name: &str) -> Option<Meta> {
        let text = std::fs::read_to_string(self.root(scope)?.join(name).join("meta.json")).ok()?;
        serde_json::from_str(&text).ok()
    }

    /// Returns the names saved in `scope`, sorted.
    pub(super) fn names(&self, scope: Scope) -> Vec<String> {
        let Some(entries) = self
            .root(scope)
            .and_then(|root| std::fs::read_dir(root).ok())
        else {
            return Vec::new();
        };
        let mut names: Vec<String> = entries
            .filter_map(|e| e.ok()?.file_name().into_string().ok())
            .filter(|name| self.meta(scope, name).is_some())
            .collect();
        names.sort();
        names
    }

    /// Finds `name` in the repo first, then among the global scripts; `version` pins one.
    pub(super) fn locate(&self, name: &str, version: Option<u32>) -> Result<Located> {
        for scope in [Scope::Repo, Scope::User] {
            let Some(meta) = self.meta(scope, name) else {
                continue;
            };
            let latest = meta.latest().map_or(0, |v| v.version);
            let version = version.unwrap_or(latest);
            if !meta.versions.iter().any(|v| v.version == version) {
                return Err(hinted(
                    format!("script {name} has no version {version}"),
                    format!("the latest is {name}@{latest}; `acus script log {name}` lists them"),
                ));
            }
            return Ok(Located { scope, version });
        }
        Err(hinted(
            format!("no saved script named {name}"),
            "`acus script list` shows the saved scripts; `acus script save NAME --note TEXT` adds one",
        ))
    }

    /// Reads the source of one version.
    pub(super) fn source(&self, scope: Scope, name: &str, version: u32) -> Result<String> {
        let root = self
            .root(scope)
            .context("a repo script needs a git repository")?;
        let file = root.join(name).join(format!("v{version}.js"));
        std::fs::read_to_string(&file).with_context(|| format!("cannot read {}", file.display()))
    }

    /// Returns the source of an imported module, `repo:NAME@V` or `user:NAME`.
    pub(super) fn import(&self, specifier: &str) -> Result<String> {
        let (scheme, spec) = specifier.split_once(':').unwrap_or_default();
        let scope = match scheme {
            "repo" => Scope::Repo,
            "user" => Scope::User,
            _ => {
                return Err(hinted(
                    format!("cannot import {specifier}: unknown scheme `{scheme}`"),
                    "import a saved script as \"repo:NAME@V\" or \"user:NAME\"",
                ));
            }
        };
        let (name, version) = parse_spec(spec)?;
        let meta = self.meta(scope, &name).ok_or_else(|| {
            hinted(
                format!(
                    "cannot import {specifier}: no {} script named {name}",
                    scope.as_str()
                ),
                "`acus script list` shows the saved scripts",
            )
        })?;
        let latest = meta.latest().map_or(0, |v| v.version);
        self.source(scope, &name, version.unwrap_or(latest))
    }

    /// Saves `source` as the next version of `name` and returns that version, or `None` when the
    /// latest version already holds exactly this source.
    pub(super) fn save(
        &self,
        scope: Scope,
        name: &str,
        source: &str,
        note: &str,
        info: &acus_script::ModuleInfo,
    ) -> Result<Option<u32>> {
        let root = self.root(scope).ok_or_else(|| {
            hinted(
                "saving a repo script needs a git repository",
                "run it inside one, or pass --global",
            )
        })?;
        let dir = root.join(name);
        let mut meta = self.meta(scope, name).unwrap_or_default();
        let version = meta.latest().map_or(0, |v| v.version);
        if version > 0 && self.source(scope, name, version)? == source {
            return Ok(None);
        }
        std::fs::create_dir_all(&dir)
            .with_context(|| format!("cannot create {}", dir.display()))?;
        if scope == Scope::Repo {
            ignore_itself(root)?;
        }
        // A `vN.js` without a meta entry is left by a crashed or concurrent save: skip past it.
        let mut version = version.max(highest_file(&dir)) + 1;
        loop {
            let file = dir.join(format!("v{version}.js"));
            let made = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&file)
                .and_then(|mut f| f.write_all(source.as_bytes()));
            match made {
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => version += 1,
                other => {
                    break other.with_context(|| format!("cannot write {}", file.display()))?;
                }
            }
        }
        meta.versions.push(Version {
            version,
            at: now(),
            note: note.to_owned(),
            description: info.description.clone(),
            params: info.params.clone(),
        });
        let json = serde_json::to_string_pretty(&meta).context("cannot encode meta.json")?;
        let target = dir.join("meta.json");
        let staged = dir.join(format!("meta.json.{}.tmp", std::process::id()));
        std::fs::write(&staged, json)
            .and_then(|()| std::fs::rename(&staged, &target))
            .with_context(|| format!("cannot write {}", target.display()))?;
        Ok(Some(version))
    }
}

/// The highest `N` among the `vN.js` files in `dir`, or 0.
fn highest_file(dir: &Path) -> u32 {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    entries
        .filter_map(|e| {
            let name = e.ok()?.file_name().into_string().ok()?;
            name.strip_prefix('v')?
                .strip_suffix(".js")?
                .parse::<u32>()
                .ok()
        })
        .max()
        .unwrap_or(0)
}

/// Gives `.acus/` its own `.gitignore` of `*`, so the repository's own file stays untouched.
fn ignore_itself(scripts: &Path) -> Result<()> {
    let Some(acus) = scripts.parent() else {
        return Ok(());
    };
    let file = acus.join(".gitignore");
    if !file.exists() {
        std::fs::write(&file, "*\n").with_context(|| format!("cannot write {}", file.display()))?;
    }
    Ok(())
}

pub(super) fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Splits `NAME` or `NAME@V` and checks the name: `a-z0-9-`, not a reserved word.
pub(super) fn parse_spec(spec: &str) -> Result<(String, Option<u32>)> {
    let (name, version) = match spec.split_once('@') {
        Some((name, v)) => {
            let version = v.parse::<u32>().ok().filter(|&v| v > 0).ok_or_else(|| {
                hinted(
                    format!("`{v}` is not a version number"),
                    "write NAME@3 to pin version 3",
                )
            })?;
            (name, Some(version))
        }
        None => (spec, None),
    };
    check_name(name)?;
    Ok((name.to_owned(), version))
}

/// Refuses a name that is empty, has characters outside `a-z0-9-`, or is a reserved word.
pub(super) fn check_name(name: &str) -> Result<()> {
    let valid = !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    if !valid || name.starts_with('-') {
        return Err(hinted(
            format!("`{name}` is not a script name"),
            "use lowercase letters, digits and `-`, as in fix-imports",
        ));
    }
    if RESERVED.contains(&name) {
        return Err(hinted(
            format!("`{name}` is reserved for `acus script {name}`"),
            "choose another name",
        ));
    }
    Ok(())
}
