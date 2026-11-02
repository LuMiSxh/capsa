// SPDX-License-Identifier: MPL-2.0
//! Owns census's user settings: the limit mode and the monthly budget, kept in `config.json` in
//! the census config directory. A missing file means plan mode.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Names the file in the config directory.
const FILE: &str = "config.json";

/// Which limits census reports.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    /// The subscription windows, 5 hours and weekly.
    #[default]
    Plan,
    /// The spend of the local calendar month against `monthly_budget_usd`.
    Budget,
}

impl Mode {
    /// Returns the name of the mode as it is written in the file and on the command line.
    pub fn name(self) -> &'static str {
        match self {
            Mode::Plan => "plan",
            Mode::Budget => "budget",
        }
    }
}

/// Lists what can go wrong with the config file.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("cannot read or write {}", .0.display())]
    Io(PathBuf, #[source] std::io::Error),
    #[error("{} is not a valid census config", .0.display())]
    Parse(PathBuf, #[source] serde_json::Error),
}

/// The settings in `config.json`. Missing keys keep their default.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub mode: Mode,
    /// The budget of one calendar month in USD; `None` until it is set.
    pub monthly_budget_usd: Option<f64>,
}

impl Config {
    /// Returns the path of the config file. Nothing is created.
    pub fn path() -> PathBuf {
        capsa_core::paths::config_dir("census").join(FILE)
    }

    /// Reads the config, and plan mode when the file is missing or unreadable. For the status line
    /// and the hook, which must not fail.
    pub fn load() -> Config {
        Config::load_from(&Config::path()).unwrap_or_default()
    }

    /// Reads the config at `path`; a missing file gives the default.
    ///
    /// # Errors
    /// Fails when the file exists but cannot be read or is not a census config.
    pub fn load_from(path: &Path) -> Result<Config, ConfigError> {
        match std::fs::read_to_string(path) {
            Ok(text) => Config::parse(&text).map_err(|e| ConfigError::Parse(path.to_path_buf(), e)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Config::default()),
            Err(e) => Err(ConfigError::Io(path.to_path_buf(), e)),
        }
    }

    /// Parses the JSON text of a config file.
    ///
    /// # Errors
    /// Fails when the text is not a JSON object of known types.
    pub fn parse(text: &str) -> Result<Config, serde_json::Error> {
        serde_json::from_str(text)
    }

    /// Returns the JSON text of the config file.
    pub fn to_json(&self) -> String {
        let mut text = serde_json::to_string_pretty(self).unwrap_or_default();
        text.push('\n');
        text
    }

    /// Writes the config to `path`, creating its directory.
    ///
    /// # Errors
    /// Fails when the directory or the file cannot be written.
    pub fn save_to(&self, path: &Path) -> Result<(), ConfigError> {
        let io = |e| ConfigError::Io(path.to_path_buf(), e);
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(io)?;
        }
        std::fs::write(path, self.to_json()).map_err(io)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_object_is_plan_mode_without_a_budget() {
        assert_eq!(Config::parse("{}").unwrap(), Config::default());
        assert_eq!(Config::default().mode, Mode::Plan);
    }

    #[test]
    fn both_keys_parse() {
        let c = Config::parse(r#"{"mode":"budget","monthly_budget_usd":200}"#).unwrap();
        assert_eq!(c.mode, Mode::Budget);
        assert_eq!(c.monthly_budget_usd, Some(200.0));
    }

    #[test]
    fn an_unknown_mode_is_refused() {
        assert!(Config::parse(r#"{"mode":"free"}"#).is_err());
    }

    #[test]
    fn serializing_round_trips() {
        let c = Config {
            mode: Mode::Budget,
            monthly_budget_usd: Some(150.5),
        };
        assert_eq!(Config::parse(&c.to_json()).unwrap(), c);
        assert!(c.to_json().contains(r#""mode": "budget""#));
    }

    #[test]
    fn a_missing_file_is_plan_mode_and_saving_creates_the_directory() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join(FILE);
        assert_eq!(Config::load_from(&path).unwrap(), Config::default());
        let c = Config {
            mode: Mode::Budget,
            monthly_budget_usd: Some(10.0),
        };
        c.save_to(&path).unwrap();
        assert_eq!(Config::load_from(&path).unwrap(), c);
    }

    #[test]
    fn an_invalid_file_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE);
        std::fs::write(&path, "not json").unwrap();
        assert!(matches!(
            Config::load_from(&path),
            Err(ConfigError::Parse(..))
        ));
    }
}
