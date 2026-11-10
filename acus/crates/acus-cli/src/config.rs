// SPDX-License-Identifier: MPL-2.0
//! Runtime configuration: `config.toml` in the platform config dir (or `$ACUS_CONFIG`),
//! overridden by `ACUS_*` environment variables.

use anyhow::Result;
use serde::Deserialize;
use std::env::var;
use std::path::{Path, PathBuf};

#[derive(Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub commands: Commands,
    pub ctx: Ctx,
    pub patch: Patch,
    pub guard: Guard,
    #[serde(skip)]
    pub path: PathBuf,
}

#[derive(Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
pub struct Commands {
    /// Command names that refuse to run, e.g. `["ctx"]`.
    pub disabled: Vec<String>,
}

#[derive(Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
pub struct Ctx {
    /// Extra regexes; `acus ctx` drops output lines matching any of them.
    pub drop: Vec<String>,
    /// Command `acus ctx` runs when none is given; default: detected from the project files.
    pub command: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
pub struct Patch {
    /// Run the file's formatter after every patch, as if `--fmt` was given.
    pub fmt: bool,
    /// Match SEARCH blocks that differ only in whitespace, as if `--fuzzy` was given.
    pub fuzzy: bool,
}

#[derive(Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Guard {
    /// Repeated pure guard hints show on the first call of a rule per session, then on every Nth;
    /// 1 shows every one, and 0 counts as 1.
    pub hint_every: usize,
}

impl Default for Guard {
    fn default() -> Self {
        Guard { hint_every: 10 }
    }
}

/// Returns the config file: ACUS_CONFIG when set, else config.toml in the platform config directory.
pub fn config_path() -> PathBuf {
    var("ACUS_CONFIG")
        .map(PathBuf::from)
        .unwrap_or_else(|_| capsa_core::paths::config_dir("acus").join("config.toml"))
}

/// Loads the configuration; the ACUS_* variables override the file.
pub fn load() -> Result<Config> {
    let mut c = read(&config_path())?;
    overrides(&mut c);
    Ok(c)
}

/// Loads the configuration like load, but a file that does not load gives the defaults at its
/// path and the problem as text, so guard can still answer.
pub fn load_or_default() -> (Config, Option<String>) {
    let path = config_path();
    let (mut c, problem) = match read(&path) {
        Ok(c) => (c, None),
        Err(e) => (
            Config {
                path,
                ..Config::default()
            },
            Some(e.to_string()),
        ),
    };
    overrides(&mut c);
    (c, problem)
}

/// Reads the config file at path; a missing file counts as the defaults.
fn read(path: &Path) -> Result<Config> {
    let mut c: Config = match std::fs::read_to_string(path) {
        Ok(s) => toml::from_str(&s).map_err(|e| invalid(path, &e))?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Config::default(),
        Err(e) => {
            return Err(capsa_core::hint::Hinted::new(
                format!("cannot read {}: {e}", path.display()),
                format!(
                    "make {} readable, or point ACUS_CONFIG at another file",
                    path.display()
                ),
            )
            .into());
        }
    };
    c.path = path.to_path_buf();
    Ok(c)
}

/// The error of a config file that does not parse, with the file as its fix.
fn invalid(path: &Path, e: &toml::de::Error) -> anyhow::Error {
    capsa_core::hint::Hinted::new(
        format!("invalid config {}: {e}", path.display()),
        format!(
            "fix {} (the message gives the line) or delete it",
            path.display()
        ),
    )
    .into()
}

/// Applies ACUS_DISABLE, a comma-separated list of commands to refuse.
fn overrides(c: &mut Config) {
    if let Ok(v) = var("ACUS_DISABLE") {
        c.commands.disabled.extend(
            v.split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(Into::into),
        );
    }
}
