// SPDX-License-Identifier: MPL-2.0
//! `velum self uninstall`: removes the shim, optionally the policy file, then the binary.

use crate::{policy, shim};
use anyhow::{Context, Result};
use capsa_core::exit::Outcome;
use capsa_core::hint::ResultExt;
use capsa_core::update::{Tool, uninstall};
use std::path::Path;

/// This binary as the self-update and uninstall code sees it.
pub const TOOL: Tool = Tool {
    name: "velum",
    version: env!("CARGO_PKG_VERSION"),
};

/// Removes the `claude` shim in `shim_dir`, the policy file with `purge`, and the binary. Each
/// step names what it removed, or with `dry_run` what it would.
///
/// The binary is checked first, so a refusal there leaves everything in place. A `claude` in
/// `shim_dir` that velum did not write is left alone: it is usually the real Claude Code.
pub fn run(shim_dir: &Path, home: &str, purge: bool, dry_run: bool) -> Result<Outcome> {
    uninstall(&TOOL, true).hinted()?;
    let shim_file = shim_dir.join("claude");
    if shim_file.symlink_metadata().is_ok() && !shim::is_shim(&shim_file) {
        println!("left {}: not a velum shim", shim_file.display());
    } else {
        shim::uninstall(shim_dir, dry_run)?;
    }
    policy_file(&policy::default_path(home), purge, dry_run)?;
    println!("{}", uninstall(&TOOL, dry_run).hinted()?);
    Ok(Outcome::Found)
}

/// Removes the policy file with `purge`; otherwise says it stays, because it is the user's.
fn policy_file(path: &Path, purge: bool, dry_run: bool) -> Result<()> {
    if path.symlink_metadata().is_err() {
        return Ok(());
    }
    if !purge {
        println!("left {} (--purge removes it)", path.display());
    } else if dry_run {
        println!("would remove {}", path.display());
    } else {
        std::fs::remove_file(path).with_context(|| format!("removing {}", path.display()))?;
        println!("removed {}", path.display());
        // Only an empty directory goes.
        if let Some(dir) = path.parent() {
            let _ = std::fs::remove_dir(dir);
        }
    }
    Ok(())
}
