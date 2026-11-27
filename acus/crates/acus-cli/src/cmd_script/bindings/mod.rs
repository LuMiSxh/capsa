// SPDX-License-Identifier: MPL-2.0
//! The functions a script can call: each turns its arguments into the command line of the acus
//! command of the same name, parses it with that command's own clap definition and returns the
//! command's computed report as a structured value.

mod args;
mod edits;
mod io;
mod query;
mod run;
#[cfg(test)]
mod tests;

use super::mcp::{self, Mcp};
use super::memo::Memo;
use super::store::Store;
use crate::config::Config;
use acus_script::{Host, HostError};
use anyhow::Context;
use capsa_core::claude_settings::permissions::Permissions;
use capsa_core::hint::find_hint;
use serde_json::Value;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

const NAMES: &[&str] = &[
    "find",
    "show",
    "outline",
    "files",
    "follow",
    "diff",
    "log",
    "blame",
    "patch",
    "write",
    "edit",
    "run",
    "read",
    "stat",
    "exists",
    "env",
    "fetch",
    "readJson",
    "textDiff",
    "parse_toml",
    "parse_yaml",
    "parse_csv",
    "sleep",
    mcp::CALL,
];

/// The calls that change files: each waits for the calls before it and holds back the ones after.
const EXCLUSIVE: &[&str] = &["patch", "write", "edit"];

/// The read-only calls whose result a script run remembers. `run`, `sleep`, MCP and anything that
/// reaches outside the repository are left out: their results can differ between identical calls.
const MEMOIZED: &[&str] = &[
    "find", "show", "outline", "files", "follow", "diff", "log", "blame", "read", "stat", "exists",
];

pub(super) struct Bindings {
    cfg: Config,
    cwd: PathBuf,
    store: Store,
    mcp: Arc<Mcp>,
    /// The Claude Code permission rules every command and file read of a script obeys.
    permissions: Arc<Permissions>,
    /// Set when the script is only being checked for saving: no server may start.
    checking: bool,
    /// Host calls the script has made, read into its log record after the run.
    calls: Arc<AtomicU64>,
    /// Results of the read-only calls, dropped whenever a call changes files.
    memo: Memo,
}

impl Bindings {
    pub(super) fn new(cfg: Config) -> anyhow::Result<Self> {
        let cwd = std::env::current_dir().context("cannot read the current directory")?;
        let store = Store::open(&cfg, &cwd);
        let deadline = Instant::now() + super::limits(&cfg).deadline;
        let permissions = Arc::new(crate::permission_mode::permissions(
            cwd.clone(),
            cfg.script.strict,
        ));
        let mcp = Arc::new(Mcp::new(
            cfg.script.mcp.clone(),
            cfg.path.clone(),
            cwd.clone(),
            deadline,
            Arc::clone(&permissions),
        ));
        Ok(Bindings {
            cfg,
            cwd,
            store,
            mcp,
            permissions,
            checking: false,
            calls: Arc::default(),
            memo: Memo::default(),
        })
    }

    /// Returns bindings for checking a script before it is saved: its `mcp:` imports load empty
    /// modules, so no server starts.
    pub(super) fn for_check(cfg: Config) -> anyhow::Result<Self> {
        Ok(Bindings {
            checking: true,
            ..Self::new(cfg)?
        })
    }

    /// Returns the MCP servers, so the caller can end them after the run.
    pub(super) fn mcp(&self) -> Arc<Mcp> {
        Arc::clone(&self.mcp)
    }

    /// Returns the count of host calls, so the caller can read it after the run.
    pub(super) fn calls(&self) -> Arc<AtomicU64> {
        Arc::clone(&self.calls)
    }
}

impl Host for Bindings {
    fn names(&self) -> &[&str] {
        NAMES
    }

    /// The calls that write wait for the calls before them and hold back the calls after them, so
    /// the order of the script's awaits is the order of the effects.
    fn exclusive(&self, name: &str) -> bool {
        EXCLUSIVE.contains(&name)
    }

    fn module(&self, specifier: &str) -> Result<String, HostError> {
        if let Some(server) = specifier.strip_prefix("mcp:") {
            return if self.checking {
                self.mcp.stub(server)
            } else {
                self.mcp.module(server)
            };
        }
        self.store.import(specifier).map_err(|e| host_error(&e))
    }

    fn call(&self, name: &str, args: Vec<Value>, deadline: Instant) -> Result<Value, HostError> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        if self.cfg.commands.disabled.iter().any(|d| d == name) {
            return Err(HostError::new(format!(
                "{name} is disabled by configuration"
            )));
        }
        if name == mcp::CALL {
            let result = self.mcp.call(&args);
            self.memo.clear();
            return result;
        }
        if name == "sleep" {
            return sleep(&args, deadline);
        }
        if EXCLUSIVE.contains(&name) {
            let result = match name {
                "write" => self.write(&args),
                "edit" => self.edit(&args),
                _ => self.patch(args, deadline),
            };
            self.memo.clear();
            return result;
        }
        if MEMOIZED.contains(&name) {
            return self
                .memo
                .get_or_run(name, &args, || self.query(name, args.clone(), deadline));
        }
        let result = self.query(name, args, deadline);
        if name == "run" {
            // A command can change files or git state the memoized reads came from.
            self.memo.clear();
        }
        result
    }
}

pub(super) fn host_error(error: &anyhow::Error) -> HostError {
    let failure = HostError::new(format!("{error:#}"));
    match find_hint(error) {
        Some(hint) => failure.with_hint(hint),
        None => failure,
    }
}

/// Waits the milliseconds given, or until the script's deadline when that comes first. The
/// engine then ends the script as timed out, so a sleep never outlives the script.
fn sleep(args: &[Value], deadline: Instant) -> Result<Value, HostError> {
    let ms = match args {
        [ms] => ms.as_u64(),
        _ => None,
    };
    let Some(ms) = ms else {
        return Err(HostError::new("sleep() takes one number of milliseconds")
            .with_hint("sleep(500) waits half a second"));
    };
    let wake = Instant::now()
        .checked_add(Duration::from_millis(ms))
        .map_or(deadline, |wake| wake.min(deadline));
    std::thread::sleep(wake.saturating_duration_since(Instant::now()));
    Ok(Value::Null)
}
