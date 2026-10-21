// SPDX-License-Identifier: MPL-2.0
//! Reads git config files for keys that make git run a program, and removes the two that hook
//! a repository into every git command.

use anyhow::{Context, Result};
use std::fs;
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::Path;

/// Config files larger than this are not parsed.
const MAX_CONFIG_BYTES: u64 = 1 << 20;

/// One `key = value` entry of a config file, with the lines it occupies.
struct Entry {
    section: String,
    key: String,
    value: String,
    first: usize,
    last: usize,
    /// The section header that shares the first line, kept when the entry is removed.
    header: Option<String>,
}

impl Entry {
    fn name(&self) -> String {
        format!("{}.{}", self.section, self.key)
    }

    /// Whether git runs a program or reads more config because of this entry.
    fn runs_code(&self) -> bool {
        let value = self.value.trim_start_matches('"');
        match self.section.as_str() {
            "core" => matches!(
                self.key.as_str(),
                "hookspath"
                    | "fsmonitor"
                    | "sshcommand"
                    | "editor"
                    | "pager"
                    | "gitproxy"
                    | "askpass"
            ),
            "filter" => true,
            "diff" => matches!(self.key.as_str(), "textconv" | "command" | "external"),
            "merge" => self.key == "driver",
            "remote" => matches!(self.key.as_str(), "uploadpack" | "receivepack"),
            "uploadpack" => self.key == "packobjectshook",
            "credential" => self.key == "helper",
            "gpg" => self.key == "program",
            "alias" => value.starts_with('!'),
            other => other.starts_with("include"),
        }
    }

    /// Whether the entry is removed from a repository made during the run.
    fn is_removed(&self) -> bool {
        self.section == "core" && matches!(self.key.as_str(), "hookspath" | "fsmonitor")
    }
}

/// Whether a config line continues on the next one (an odd number of trailing backslashes).
fn continues(line: &str) -> bool {
    line.bytes().rev().take_while(|b| *b == b'\\').count() % 2 == 1
}

fn parse(text: &str) -> Vec<Entry> {
    let lines: Vec<&str> = text.lines().collect();
    let mut entries = Vec::new();
    let mut section = String::new();
    let mut index = 0;
    while index < lines.len() {
        let first = index;
        index += 1;
        let mut line = lines[first].trim();
        let mut header = None;
        if line.is_empty() || line.starts_with(['#', ';']) {
            continue;
        }
        if let Some(rest) = line.strip_prefix('[') {
            let mut quoted = false;
            let close = rest.find(|c: char| {
                quoted ^= c == '"';
                c == ']' && !quoted
            });
            let Some(close) = close else {
                continue;
            };
            section = rest[..close]
                .split(|c: char| c.is_whitespace() || c == '"' || c == '.')
                .next()
                .unwrap_or("")
                .to_ascii_lowercase();
            header = Some(format!("[{}]", &rest[..close]));
            line = rest[close + 1..].trim();
            if line.is_empty() {
                continue;
            }
        }
        let mut last = first;
        let mut logical = line.to_string();
        while continues(lines[last]) && last + 1 < lines.len() {
            last += 1;
            logical.push_str(lines[last].trim());
            index = last + 1;
        }
        let (key, value) = logical.split_once('=').unwrap_or((&logical, ""));
        entries.push(Entry {
            section: section.clone(),
            key: key.trim().to_ascii_lowercase(),
            value: value.trim().to_string(),
            first,
            last,
            header,
        });
    }
    entries
}

/// Returns the config keys in `text` that make git run a program or read more config, as
/// `section.key`.
pub(super) fn config_hits(text: &str) -> Vec<String> {
    parse(text)
        .iter()
        .filter(|entry| entry.runs_code())
        .map(Entry::name)
        .collect()
}

/// Reads the config of `git_dir` when it is a regular file of a parsable size.
fn read_config(git_dir: &Path) -> Result<Option<String>, String> {
    let config = git_dir.join("config");
    let Ok(meta) = config.symlink_metadata() else {
        return Ok(None);
    };
    if !meta.is_file() || meta.len() > MAX_CONFIG_BYTES {
        return Err(format!("{} (not a regular config file)", config.display()));
    }
    Ok(fs::read_to_string(&config).ok())
}

/// Lists the entries in the config of `git_dir` that run code, as `<path>: <section.key>`.
pub(super) fn config_findings(git_dir: &Path) -> Vec<String> {
    let config = git_dir.join("config");
    match read_config(git_dir) {
        Err(why) => vec![why],
        Ok(None) => Vec::new(),
        Ok(Some(text)) => config_hits(&text)
            .into_iter()
            .map(|key| format!("{}: {key}", config.display()))
            .collect(),
    }
}

/// Removes `core.hooksPath` and `core.fsmonitor` from the config of `git_dir` by writing a
/// new file beside it and renaming it over the old one, so a link is replaced and never
/// followed. Does nothing for a config that is not a regular file. Returns the removed
/// entries as `<path>: <section.key>`.
pub(super) fn neutralise(git_dir: &Path) -> Result<Vec<String>> {
    let config = git_dir.join("config");
    let Ok(Some(text)) = read_config(git_dir) else {
        return Ok(Vec::new());
    };
    let doomed: Vec<Entry> = parse(&text).into_iter().filter(Entry::is_removed).collect();
    if doomed.is_empty() {
        return Ok(Vec::new());
    }
    let mut kept = String::new();
    for (number, line) in text.lines().enumerate() {
        match doomed
            .iter()
            .find(|entry| (entry.first..=entry.last).contains(&number))
        {
            None => kept.extend([line, "\n"]),
            Some(entry) if number == entry.first => {
                if let Some(header) = &entry.header {
                    kept.extend([header.as_str(), "\n"]);
                }
            }
            Some(_) => {}
        }
    }
    let mode = config.symlink_metadata()?.permissions().mode() & 0o777;
    let temp = git_dir.join(format!("config.velum-{}", std::process::id()));
    let written = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(mode)
        .open(&temp)
        .and_then(|mut file| {
            file.write_all(kept.as_bytes())
                .and_then(|()| file.sync_all())
        })
        .and_then(|()| fs::rename(&temp, &config));
    if let Err(err) = written {
        let _ = fs::remove_file(&temp);
        return Err(err).with_context(|| format!("rewriting {}", config.display()));
    }
    Ok(doomed
        .iter()
        .map(|entry| format!("{}: {}", config.display(), entry.name()))
        .collect())
}
