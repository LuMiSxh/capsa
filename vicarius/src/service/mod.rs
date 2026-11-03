// SPDX-License-Identifier: MPL-2.0
//! Owns the launchd user agent that runs `vicarius serve`: its files, the `launchctl` calls behind
//! a trait, and the settings `env` keys and hook that point Claude Code at the service.

mod plist;
#[cfg(test)]
mod tests;

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, ensure};
use capsa_core::claude_settings::{self, Removal, Step};
use capsa_core::hint::{Hinted, ResultExt};
use vicarius::config::Config;

pub use plist::program_path;

/// The launchd label of the agent.
pub const LABEL: &str = "io.github.lumisxh.vicarius";
/// The settings `env` key that points Claude Code at the service.
pub const BASE_URL: &str = "ANTHROPIC_BASE_URL";
/// The settings `env` key that declares the context window of the DeepSeek models.
pub const MAX_CONTEXT: &str = "CLAUDE_CODE_MAX_CONTEXT_TOKENS";
/// The binary name that the settings hook runs, bare, as census does.
pub const TOOL: &str = "vicarius";
/// The arguments of the SessionStart hook command.
const HOOK_ARGS: &str = "hook session-start";

/// The files and the launchd domain of the agent.
#[derive(Debug, Clone)]
pub struct Install {
    /// The property list, in `~/Library/LaunchAgents`.
    pub plist: PathBuf,
    /// Where launchd sends the standard output and error of the service.
    pub log: PathBuf,
    /// Claude Code's settings file.
    pub settings: PathBuf,
    /// The config file that the agent reads at boot.
    pub config: PathBuf,
    /// The binary that the agent runs.
    pub exe: PathBuf,
    /// The launchd domain of the user, such as `gui/501`; `None` where launchd does not run.
    pub domain: Option<String>,
}

impl Install {
    /// Describes this user's install: the agent, its log, the settings and this binary.
    ///
    /// # Errors
    /// Fails without a home directory, without a path for this binary or, on macOS, when `id -u`
    /// fails.
    pub fn current() -> Result<Self> {
        Self::for_platform(cfg!(target_os = "macos"), user_id)
    }

    /// Describes the install on a platform where `launchd` is set, and runs `user_id` only then.
    ///
    /// # Errors
    /// Fails without a home directory, without a path for this binary or when `user_id` fails.
    pub fn for_platform(launchd: bool, user_id: impl FnOnce() -> Result<String>) -> Result<Self> {
        let home = std::env::home_dir().context("no home directory")?;
        let domain = if launchd {
            Some(format!("gui/{}", user_id()?))
        } else {
            None
        };
        Ok(Self {
            plist: home
                .join("Library/LaunchAgents")
                .join(format!("{LABEL}.plist")),
            log: capsa_core::paths::state_dir(TOOL).join("service.log"),
            settings: claude_settings::path(),
            config: Config::path(),
            exe: std::env::current_exe().context("cannot find this binary")?,
            domain,
        })
    }

    /// Returns the launchd domain and the target of the agent, such as `gui/501` and
    /// `gui/501/io.github.lumisxh.vicarius`, or `None` where launchd does not run.
    pub fn launchd(&self) -> Option<(&str, String)> {
        self.domain
            .as_deref()
            .map(|domain| (domain, format!("{domain}/{LABEL}")))
    }

    /// Returns the property list that this install should have now.
    pub fn plist_text(&self) -> String {
        plist::render(LABEL, &self.exe, &self.log)
    }
}

fn user_id() -> Result<String> {
    let out = Command::new("id")
        .arg("-u")
        .output()
        .context("cannot run `id -u`")?;
    ensure!(out.status.success(), "`id -u` failed");
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

/// The result of one `launchctl` call.
#[derive(Debug)]
pub struct Call {
    /// Whether `launchctl` exited with status 0.
    pub ok: bool,
    /// What `launchctl` wrote to standard error, for the error message.
    pub stderr: String,
}

/// Runs `launchctl`. Tests pass a fake, so that they never touch launchd.
pub trait Launchctl {
    /// Runs `launchctl` with `args`.
    ///
    /// # Errors
    /// Fails when `launchctl` cannot be started.
    fn run(&self, args: &[&str]) -> io::Result<Call>;
}

/// The `launchctl` of this machine.
pub struct System;

impl Launchctl for System {
    fn run(&self, args: &[&str]) -> io::Result<Call> {
        let out = Command::new("launchctl").args(args).output()?;
        Ok(Call {
            ok: out.status.success(),
            stderr: String::from_utf8_lossy(&out.stderr).trim().to_owned(),
        })
    }
}

/// Whether launchd has the agent loaded. `launchctl print` exits 0 for a loaded agent.
pub fn is_loaded(lctl: &dyn Launchctl, install: &Install) -> bool {
    install.launchd().is_some_and(|(_, target)| {
        lctl.run(&["print", target.as_str()])
            .is_ok_and(|call| call.ok)
    })
}

/// Returns whether the property list on disk is the one this install would write.
pub fn plist_is_current(install: &Install) -> bool {
    fs::read_to_string(&install.plist).is_ok_and(|text| text == install.plist_text())
}

/// Writes the property list unless the file already holds the current text, through a temporary
/// file that is renamed over it. Returns whether the file was written.
///
/// # Errors
/// Fails when the directories or the file cannot be written or renamed.
pub fn write_plist(install: &Install) -> io::Result<bool> {
    if plist_is_current(install) {
        return Ok(false);
    }
    for dir in [install.plist.parent(), install.log.parent()]
        .into_iter()
        .flatten()
    {
        fs::create_dir_all(dir)?;
    }
    let tmp = install.plist.with_extension("plist.tmp");
    fs::write(&tmp, install.plist_text())?;
    fs::rename(&tmp, &install.plist).inspect_err(|_| {
        let _ = fs::remove_file(&tmp);
    })?;
    Ok(true)
}

/// Returns the `ANTHROPIC_BASE_URL` value that points Claude Code at the service on `port`.
pub fn base_url(port: u16) -> String {
    format!("http://127.0.0.1:{port}")
}

/// Returns the `env` keys that the service needs, with the value each takes for `config`.
pub fn env_values(config: &Config) -> [(&'static str, String); 2] {
    [
        (BASE_URL, base_url(config.port)),
        (MAX_CONTEXT, config.max_context_tokens.to_string()),
    ]
}

/// Sets the `env` keys in the settings file, writing only when one of them changes.
///
/// # Errors
/// Fails when the settings file cannot be read or written.
pub fn set_env_keys(settings: &Path, config: &Config) -> Result<Vec<(&'static str, Step)>> {
    let mut file = claude_settings::read(settings).hinted()?;
    let steps: Vec<_> = env_values(config)
        .into_iter()
        .map(|(key, value)| (key, claude_settings::set_env(&mut file.value, key, &value)))
        .collect();
    if steps.iter().any(|(_, step)| step.writes()) {
        claude_settings::write(&file).hinted()?;
    }
    Ok(steps)
}

/// Removes the `env` keys that hold the values the service set. A key with another value, set by
/// hand, stays.
///
/// # Errors
/// Fails when the settings file cannot be read or written.
pub fn remove_env_keys(settings: &Path, config: &Config) -> Result<Vec<(&'static str, Removal)>> {
    let mut file = claude_settings::read(settings).hinted()?;
    let steps: Vec<_> = env_values(config)
        .into_iter()
        .map(|(key, value)| {
            (
                key,
                claude_settings::remove_env(&mut file.value, key, &value),
            )
        })
        .collect();
    if steps.iter().any(|(_, removal)| removal.writes()) {
        claude_settings::write(&file).hinted()?;
    }
    Ok(steps)
}

/// Refuses when a settings `env` key that the service sets holds another value. Such a value was
/// set by hand: the service would replace it and could not give it back.
///
/// # Errors
/// Fails when the settings file cannot be read, or when a key holds another value.
pub fn check_env_free(settings: &Path, config: &Config) -> Result<()> {
    let file = claude_settings::read(settings).hinted()?;
    for (key, value) in env_values(config) {
        let Some(current) = file.value.get("env").and_then(|env| env.get(key)) else {
            continue;
        };
        if current.as_str() != Some(value.as_str()) {
            return Err(Hinted::new(
                format!(
                    "{key} is set to {current} in {}, not to {value}",
                    settings.display()
                ),
                format!(
                    "remove or change {key} in {} first, then run this again",
                    settings.display()
                ),
            )
            .into());
        }
    }
    Ok(())
}

/// What `replace_env_value` did to an `env` key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Replacement {
    /// The key held the old value and now holds the new one.
    Replaced,
    /// The key already holds the new value; nothing changes.
    Same,
    /// The key is not in `env`; nothing is added.
    Absent,
    /// The key holds another value, set by hand, so it stays.
    Kept,
}

impl Replacement {
    /// Returns the report word for this replacement.
    pub fn text(self) -> &'static str {
        match self {
            Self::Replaced => "replaced",
            Self::Same => "already set",
            Self::Absent => "absent",
            Self::Kept => "kept",
        }
    }
}

/// Changes the `env` value of `key` from `old` to `new`, but only where it holds `old`, so a value
/// that vicarius did not write stays. A missing key is not added.
///
/// # Errors
/// Fails when the settings file cannot be read or written.
pub fn replace_env_value(settings: &Path, key: &str, old: &str, new: &str) -> Result<Replacement> {
    let mut file = claude_settings::read(settings).hinted()?;
    let replacement = match claude_settings::env_value(&file.value, key) {
        None => return Ok(Replacement::Absent),
        Some(current) if current != old => return Ok(Replacement::Kept),
        Some(_) if old == new => return Ok(Replacement::Same),
        Some(_) => Replacement::Replaced,
    };
    claude_settings::set_env(&mut file.value, key, new);
    claude_settings::write(&file).hinted()?;
    Ok(replacement)
}

/// Makes the SessionStart hook run `vicarius hook session-start`. Returns the step it took; with
/// `dry_run` nothing is written.
///
/// # Errors
/// Fails when the settings file cannot be read or written.
pub fn set_session_hook(settings: &Path, dry_run: bool) -> Result<Step> {
    let mut file = claude_settings::read(settings).hinted()?;
    let command = claude_settings::command_line(Path::new(TOOL), HOOK_ARGS);
    let step =
        claude_settings::set_hook(&mut file.value, "SessionStart", None, &command, hook_owns);
    if !dry_run && step.writes() {
        claude_settings::write(&file).hinted()?;
    }
    Ok(step)
}

/// Removes the SessionStart hook that `set_session_hook` installed. Returns whether it was there.
///
/// # Errors
/// Fails when the settings file cannot be read or written.
pub fn remove_session_hook(settings: &Path) -> Result<bool> {
    let mut file = claude_settings::read(settings).hinted()?;
    let removed = claude_settings::remove_hook(&mut file.value, "SessionStart", hook_owns);
    if removed {
        claude_settings::write(&file).hinted()?;
    }
    Ok(removed)
}

fn hook_owns(command: &str) -> bool {
    claude_settings::runs(command, TOOL, HOOK_ARGS)
}
