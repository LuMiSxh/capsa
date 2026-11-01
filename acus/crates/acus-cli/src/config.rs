// SPDX-License-Identifier: MPL-2.0
//! Runtime configuration: `config.toml` in the platform config dir (or `$ACUS_CONFIG`),
//! overridden by `ACUS_*` environment variables.

use anyhow::{Context, Result};
use serde::Deserialize;
use std::env::var;
use std::path::PathBuf;

#[derive(Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub commands: Commands,
    pub decide: Decide,
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

#[derive(Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
pub struct Decide {
    pub enabled: Option<bool>,
    /// Full endpoint, e.g. `https://api.typesafe.ai/v1/systemone`.
    pub url: Option<String>,
    pub model: Option<String>,
    /// Name of the environment variable holding the bearer token.
    pub api_key_env: Option<String>,
}

pub fn load() -> Result<Config> {
    let path = var("ACUS_CONFIG")
        .map(PathBuf::from)
        .unwrap_or_else(|_| capsa_core::paths::config_dir("acus").join("config.toml"));
    let mut c: Config = match std::fs::read_to_string(&path) {
        Ok(s) => {
            toml::from_str(&s).with_context(|| format!("invalid config {}", path.display()))?
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Config::default(),
        Err(e) => return Err(e).with_context(|| format!("cannot read {}", path.display())),
    };
    c.path = path;
    if let Ok(v) = var("ACUS_DISABLE") {
        c.commands.disabled.extend(
            v.split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(Into::into),
        );
    }
    if let Ok(v) = var("ACUS_DECIDE_ENABLED") {
        c.decide.enabled = Some(!matches!(v.as_str(), "0" | "false" | "no" | "off"));
    }
    for (key, slot) in [
        ("ACUS_DECIDE_URL", &mut c.decide.url),
        ("ACUS_DECIDE_MODEL", &mut c.decide.model),
        ("ACUS_DECIDE_API_KEY_ENV", &mut c.decide.api_key_env),
    ] {
        if let Ok(v) = var(key) {
            *slot = Some(v);
        }
    }
    Ok(c)
}
