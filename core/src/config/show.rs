// SPDX-License-Identifier: MPL-2.0
//! The effective config as a `Render` value: one row per setting, named by its dotted key.

use serde::Serialize;

use super::{ConfigError, ConfigFile};
use crate::render::{Doc, Render};

/// The effective config of a tool, shown with the file it comes from.
///
/// JSON holds the settings flattened at the top level, next to `path`, so a script reads
/// `limits.five_hour` as `"limits": {"five_hour": …}`.
#[derive(Debug, Serialize)]
pub struct Shown {
    #[serde(skip)]
    title: &'static str,
    #[serde(flatten)]
    settings: toml::Table,
    path: String,
}

impl ConfigFile {
    /// Builds the view of `value`, the effective config, with `title` above it.
    ///
    /// # Errors
    /// Fails when `value` cannot be represented as TOML.
    pub fn show<T: Serialize>(&self, title: &'static str, value: &T) -> Result<Shown, ConfigError> {
        let settings = toml::Table::try_from(value).map_err(ConfigError::Encode)?;
        Ok(Shown {
            title,
            settings,
            path: self.path().display().to_string(),
        })
    }
}

impl Render for Shown {
    fn doc(&self, doc: &mut Doc) {
        doc.title(self.title);
        rows(&self.settings, "", doc);
        doc.row("file").dim(self.path.as_str());
    }
}

fn rows(table: &toml::Table, prefix: &str, doc: &mut Doc) {
    for (key, value) in table {
        let name = if prefix.is_empty() {
            key.clone()
        } else {
            format!("{prefix}.{key}")
        };
        match value {
            toml::Value::Table(inner) if !inner.is_empty() => rows(inner, &name, doc),
            _ => {
                doc.row(name).text(value.to_string());
            }
        }
    }
}
