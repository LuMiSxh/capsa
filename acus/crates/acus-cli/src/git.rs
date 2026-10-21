// SPDX-License-Identifier: MPL-2.0
//! Git helpers shared by the commands that read history.

use anyhow::{Result, bail};
use std::process::Command;

pub fn git(args: &[&str]) -> Result<String> {
    let o = Command::new("git").args(args).output()?;
    if !o.status.success() {
        bail!(
            "git {}: {}",
            args.iter()
                .find(|a| !a.starts_with('-') && !a.contains('='))
                .unwrap_or(&""),
            // git appends its whole usage text to some errors; the first line says it all.
            String::from_utf8_lossy(&o.stderr)
                .lines()
                .find(|l| !l.trim().is_empty())
                .unwrap_or("failed")
                .trim_start_matches("warning: ")
        );
    }
    Ok(String::from_utf8_lossy(&o.stdout).into_owned())
}
