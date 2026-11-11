// SPDX-License-Identifier: MPL-2.0
//! Owns each tool's `config.toml`: loading, atomic saving, dotted-key `set` and the `show` view.
//! A tool keeps its own typed struct, which must use `#[serde(deny_unknown_fields)]`, so that
//! `set` rejects an unknown key before it reaches disk.

mod show;
#[cfg(test)]
mod tests;

pub use show::Shown;

use std::borrow::Cow;
use std::ffi::OsString;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::fs::write_atomic;
use crate::hint::Hint;
use crate::paths;

const FILE_NAME: &str = "config.toml";
const LEGACY_FILE_NAME: &str = "config.json";

/// Why a config file could not be read, parsed, changed or written.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("cannot read {}: {source}", path.display())]
    Read {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("cannot write {}: {source}", path.display())]
    Write {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("invalid config {}: line {line}, column {column}: {message}", path.display())]
    Parse {
        path: PathBuf,
        line: usize,
        column: usize,
        message: String,
    },
    #[error("cannot encode the config: {0}")]
    Encode(#[source] toml::ser::Error),
    #[error("invalid key `{key}`: {message}")]
    Key { key: String, message: String },
    #[error("invalid value for `{key}`: {message}")]
    Invalid { key: String, message: String },
}

impl Hint for ConfigError {
    fn hint(&self) -> Option<Cow<'_, str>> {
        let text = match self {
            Self::Parse { .. } => "fix the TOML at the position shown, or move the file aside",
            Self::Key { .. } => "use a dotted key such as `limits.five_hour`",
            Self::Invalid { .. } => "run `self config show` to see the settings and their values",
            Self::Read { .. } | Self::Write { .. } | Self::Encode(_) => return None,
        };
        Some(Cow::Borrowed(text))
    }
}

/// The config file of one tool, at a known path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigFile {
    path: PathBuf,
}

impl ConfigFile {
    /// Locates a tool's config file: `$env_var` when it is set and not empty, else
    /// `config.toml` in the tool's config directory. Nothing is read or created.
    pub fn locate(tool: &str, env_var: &str) -> Self {
        let dir = paths::config_dir(tool);
        Self {
            path: choose_path(std::env::var_os(env_var), &dir),
        }
    }

    /// Uses the config file at `path`.
    pub fn at(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// Returns the file's path, whether or not it exists.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Loads the file as `T`. A missing file gives `T::default()`.
    ///
    /// # Errors
    /// Fails when the file cannot be read, or when its TOML is invalid or does not fit `T`.
    pub fn load<T: DeserializeOwned + Default>(&self) -> Result<T, ConfigError> {
        match self.read_text()? {
            None => Ok(T::default()),
            Some(text) => toml::from_str(&text).map_err(|e| self.parse_error(&text, &e)),
        }
    }

    /// Writes `value` to the file atomically, creating its directory.
    ///
    /// # Errors
    /// Fails when `value` cannot be encoded as TOML, or when the file cannot be written.
    pub fn save<T: Serialize>(&self, value: &T) -> Result<(), ConfigError> {
        let text = toml::to_string_pretty(value).map_err(ConfigError::Encode)?;
        self.write_text(&text)
    }

    /// Sets the dotted `key` (such as `limits.five_hour`) to `value`, then saves the file.
    ///
    /// `value` is read as a TOML value (bool, integer, float, array or inline table) and is kept
    /// as a string when it is not one. The whole document is checked against `T` before the
    /// file is written, so an unknown key or a wrong type leaves the file as it was.
    ///
    /// # Errors
    /// Fails when the key is malformed or passes through a non-table value, when the file is
    /// invalid, when the result does not fit `T`, or when the file cannot be written.
    pub fn set<T: DeserializeOwned>(&self, key: &str, value: &str) -> Result<(), ConfigError> {
        let mut table = match self.read_text()? {
            Some(text) => {
                toml::from_str::<toml::Table>(&text).map_err(|e| self.parse_error(&text, &e))?
            }
            None => toml::Table::new(),
        };
        insert(&mut table, key, parse_value(value))?;
        let text = toml::to_string_pretty(&table).map_err(ConfigError::Encode)?;
        toml::from_str::<T>(&text).map_err(|e| ConfigError::Invalid {
            key: key.to_owned(),
            message: e.message().to_owned(),
        })?;
        self.write_text(&text)
    }

    /// Returns the file's text, or `None` when the file does not exist.
    fn read_text(&self) -> Result<Option<String>, ConfigError> {
        match fs::read_to_string(&self.path) {
            Ok(text) => Ok(Some(text)),
            Err(source) if source.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(source) => Err(ConfigError::Read {
                path: self.path.clone(),
                source,
            }),
        }
    }

    fn write_text(&self, text: &str) -> Result<(), ConfigError> {
        let write_error = |source| ConfigError::Write {
            path: self.path.clone(),
            source,
        };
        if let Some(dir) = self.path.parent().filter(|dir| !dir.as_os_str().is_empty()) {
            fs::create_dir_all(dir).map_err(write_error)?;
        }
        write_atomic(&self.path, text.as_bytes()).map_err(write_error)
    }

    fn parse_error(&self, text: &str, error: &toml::de::Error) -> ConfigError {
        let offset = error.span().map_or(0, |span| span.start);
        let before = text.get(..offset).unwrap_or(text);
        let line = before.matches('\n').count() + 1;
        let column = before
            .rsplit('\n')
            .next()
            .map_or(0, |last| last.chars().count())
            + 1;
        ConfigError::Parse {
            path: self.path.clone(),
            line,
            column,
            message: error.message().to_owned(),
        }
    }
}

/// Returns the path of a tool's legacy `config.json`, when that file still exists.
///
/// A tool written before TOML keeps its settings in this file. Nothing is migrated: a doctor
/// reports the file so that the user can move the settings to `config.toml` by hand.
pub fn legacy_json(tool: &str) -> Option<PathBuf> {
    legacy_json_in(&paths::config_dir(tool))
}

fn legacy_json_in(dir: &Path) -> Option<PathBuf> {
    let path = dir.join(LEGACY_FILE_NAME);
    path.is_file().then_some(path)
}

fn choose_path(override_path: Option<OsString>, dir: &Path) -> PathBuf {
    override_path
        .filter(|path| !path.is_empty())
        .map_or_else(|| dir.join(FILE_NAME), PathBuf::from)
}

/// Reads `raw` as a TOML value, or as a string when it is not one. A value that would add a
/// second key, such as one with a newline, is a string too.
fn parse_value(raw: &str) -> toml::Value {
    toml::from_str::<toml::Table>(&format!("v = {raw}"))
        .ok()
        .filter(|table| table.len() == 1)
        .and_then(|mut table| table.remove("v"))
        .unwrap_or_else(|| toml::Value::String(raw.to_owned()))
}

fn insert(table: &mut toml::Table, key: &str, value: toml::Value) -> Result<(), ConfigError> {
    let key_error = |message: String| ConfigError::Key {
        key: key.to_owned(),
        message,
    };
    if key.split('.').any(str::is_empty) {
        return Err(key_error("a key segment is empty".to_owned()));
    }
    let mut parts: Vec<&str> = key.split('.').collect();
    let last = parts.pop().unwrap_or_default();
    let mut current = table;
    for part in parts {
        if !current.contains_key(part) {
            current.insert(part.to_owned(), toml::Value::Table(toml::Table::new()));
        }
        current = current
            .get_mut(part)
            .and_then(toml::Value::as_table_mut)
            .ok_or_else(|| key_error(format!("`{part}` is not a table")))?;
    }
    current.insert(last.to_owned(), value);
    Ok(())
}
