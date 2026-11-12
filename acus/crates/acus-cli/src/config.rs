// SPDX-License-Identifier: MPL-2.0
//! Runtime configuration: `config.toml` in the platform config dir (or `$ACUS_CONFIG`),
//! overridden by `ACUS_*` environment variables.

use anyhow::Result;
use capsa_core::config::{ConfigError, ConfigFile};
use capsa_core::hint::hinted;
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
pub fn config_file() -> ConfigFile {
    ConfigFile::locate("acus", "ACUS_CONFIG")
}

/// Loads the configuration; the ACUS_* variables override the file.
pub fn load() -> Result<Config> {
    let mut c = read(&config_file())?;
    overrides(&mut c);
    Ok(c)
}

/// Loads the configuration like load, but a file that does not load gives the defaults at its
/// path and the problem as text, so guard can still answer.
pub fn load_or_default() -> (Config, Option<String>) {
    let file = config_file();
    let (mut c, problem) = match read(&file) {
        Ok(c) => (c, None),
        Err(e) => (
            Config {
                path: file.path().to_path_buf(),
                ..Config::default()
            },
            Some(e.to_string()),
        ),
    };
    overrides(&mut c);
    (c, problem)
}

/// Reads the config file; a missing file counts as the defaults.
fn read(file: &ConfigFile) -> Result<Config> {
    let mut c: Config = file.load().map_err(|e| read_error(file.path(), &e))?;
    c.path = file.path().to_path_buf();
    Ok(c)
}

/// The error of a config file that does not load, with the fix the file needs.
fn read_error(path: &Path, e: &ConfigError) -> anyhow::Error {
    let hint = match e {
        ConfigError::Read { .. } => format!(
            "make {} readable, or point ACUS_CONFIG at another file",
            path.display()
        ),
        _ => format!(
            "fix {} (the message gives the line) or delete it",
            path.display()
        ),
    };
    hinted(e.to_string(), hint)
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
