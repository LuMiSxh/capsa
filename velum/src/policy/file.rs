// SPDX-License-Identifier: MPL-2.0
//! Owns the policy file format: its TOML sections and how they extend the built-in defaults.

use super::Policy;
use anyhow::Result;
use serde::Deserialize;
use std::path::Path;

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct PolicyFile {
    fs: FsSection,
    env: EnvSection,
    net: NetSection,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct FsSection {
    write: Vec<String>,
    deny_read: Vec<String>,
    deny_write: Vec<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct EnvSection {
    pass: Vec<String>,
    pass_secret: Vec<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct NetSection {
    allow: Vec<String>,
}

impl Policy {
    /// Parses policy TOML and extends the built-in defaults with it.
    pub fn from_toml(cwd: &Path, text: &str) -> Result<Self> {
        let file: PolicyFile = toml::from_str(text)?;
        let mut policy = Self::builtin(cwd);
        extend(&mut policy.write, file.fs.write);
        extend(&mut policy.deny_read, file.fs.deny_read);
        extend(&mut policy.deny_write, file.fs.deny_write);
        extend(&mut policy.env_pass, file.env.pass);
        extend(&mut policy.env_pass_secret, file.env.pass_secret);
        extend(&mut policy.net_allow, file.net.allow);
        Ok(policy)
    }
}

fn extend(list: &mut Vec<String>, more: Vec<String>) {
    for item in more {
        if !list.contains(&item) {
            list.push(item);
        }
    }
}
