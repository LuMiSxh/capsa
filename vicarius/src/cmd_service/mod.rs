// SPDX-License-Identifier: MPL-2.0
//! `vicarius self setup`, `start` and `stop`, and the removal that `self uninstall` runs before the
//! binary goes. The service is started first and the settings keys are written only once its
//! status answers, so Claude Code is never pointed at a proxy that is not up.

#[cfg(test)]
mod tests;

use std::fs;
use std::io;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use capsa_core::exit::Outcome;
use capsa_core::format::Format;
use capsa_core::hint::{Hinted, ResultExt};
use capsa_core::render::{Doc, Render, emit};
use serde::Serialize;
use vicarius::config::Config;

use crate::service::{self, Call, Install, Launchctl, System};
use crate::status_client;

/// How long `start` waits for the status endpoint to answer.
pub(crate) const WAIT: Duration = Duration::from_secs(5);
/// The timeout of one status probe while `start` waits.
const PROBE: Duration = Duration::from_millis(500);
/// The timeout of the balance refresh, which waits for DeepSeek.
const REFRESH: Duration = Duration::from_secs(20);
/// The note after a change to the settings, which running sessions do not see.
pub(crate) const RESTART_NOTE: &str =
    "Running Claude Code sessions keep their old environment until they are restarted.";
/// How long `launch` waits for launchd to finish unloading the agent before it bootstraps it.
const UNLOAD_WAIT: Duration = Duration::from_secs(5);
/// The interval between two checks of the unload.
const UNLOAD_POLL: Duration = Duration::from_millis(100);
/// The error text where launchd does not run.
const NO_LAUNCHD: &str = "launchd does not run on this platform";

#[derive(clap::Args)]
pub struct SetupArgs {
    /// Reports what would change and runs nothing.
    #[arg(long)]
    pub dry_run: bool,
}

/// One step of a command and what it did.
#[derive(Debug, Serialize)]
pub(crate) struct Item {
    pub(crate) name: String,
    pub(crate) status: String,
}

impl Item {
    pub(crate) fn new(name: impl Into<String>, status: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            status: status.into(),
        }
    }
}

/// The steps of one command, in the order they ran.
#[derive(Debug, Serialize)]
struct Report {
    title: &'static str,
    dry_run: bool,
    steps: Vec<Item>,
    note: Option<&'static str>,
}

impl Render for Report {
    fn doc(&self, doc: &mut Doc) {
        doc.title(self.title);
        for step in &self.steps {
            doc.row(step.name.as_str()).text(step.status.as_str());
        }
        if self.dry_run {
            doc.line("dry run: nothing was written or run");
        }
        if let Some(note) = self.note {
            doc.line(note);
        }
    }
}

/// Runs `vicarius self setup`: the property list, the SessionStart hook, then `start`.
pub fn setup(a: &SetupArgs, format: Format) -> Result<Outcome> {
    ensure_macos()?;
    let config = load_config()?;
    let install = Install::current()?;
    let report = setup_report(&install, &System, &config, a.dry_run)?;
    emit(&report, format)
}

/// Runs `vicarius self start`.
pub fn start(format: Format) -> Result<Outcome> {
    ensure_macos()?;
    let config = load_config()?;
    let install = Install::current()?;
    let report = start_report(&install, &System, &config)?;
    emit(&report, format)
}

/// Runs `vicarius self stop`: the settings keys first, then the agent.
pub fn stop(format: Format) -> Result<Outcome> {
    ensure_macos()?;
    let config = load_config()?;
    let install = Install::current()?;
    let report = Report {
        title: "vicarius self stop",
        dry_run: false,
        steps: stop_steps(&install, &System, &config)?,
        note: Some(RESTART_NOTE),
    };
    emit(&report, format)
}

/// Removes what setup installed, for `self uninstall`: the settings keys and the agent, then the
/// property list and the SessionStart hook. Where launchd does not run, only the keys and the
/// hook go. A config that does not load still lets the keys that it can match go; the others are
/// kept.
///
/// # Errors
/// Fails when the agent cannot be stopped or a file cannot be written or removed.
pub fn remove_integration() -> Result<()> {
    let install = Install::current()?;
    let config = Config::load_from(&Config::path()).unwrap_or_default();
    remove_integration_with(&install, &System, &config)
}

fn remove_integration_with(install: &Install, lctl: &dyn Launchctl, config: &Config) -> Result<()> {
    if install.launchd().is_some() {
        stop_steps(install, lctl, config)?;
        remove_plist(install)?;
    } else {
        service::remove_env_keys(&install.settings, config)?;
    }
    service::remove_session_hook(&install.settings)?;
    Ok(())
}

fn remove_plist(install: &Install) -> Result<()> {
    match fs::remove_file(&install.plist) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e).with_context(|| format!("cannot remove {}", install.plist.display())),
    }
}

fn setup_report(
    install: &Install,
    lctl: &dyn Launchctl,
    config: &Config,
    dry_run: bool,
) -> Result<Report> {
    service::check_env_free(&install.settings, config)?;
    let (plist, written) = plist_step(install, dry_run)?;
    let mut steps = vec![
        Item::new("plist", plist),
        Item::new("SessionStart hook", hook_step(install, dry_run)?),
    ];
    if dry_run {
        steps.push(Item::new(
            "agent",
            "would start, then set the settings keys once the status answers",
        ));
    } else {
        steps.extend(start_steps(install, lctl, config, WAIT, written)?);
    }
    Ok(Report {
        title: "vicarius self setup",
        dry_run,
        steps,
        note: (!dry_run).then_some(RESTART_NOTE),
    })
}

/// Runs the steps of `self start`. A hand-set settings key stops the run before anything is written.
fn start_report(install: &Install, lctl: &dyn Launchctl, config: &Config) -> Result<Report> {
    service::check_env_free(&install.settings, config)?;
    let (plist, written) = plist_step(install, false)?;
    let mut steps = vec![Item::new("plist", plist)];
    steps.extend(start_steps(install, lctl, config, WAIT, written)?);
    Ok(Report {
        title: "vicarius self start",
        dry_run: false,
        steps,
        note: Some(RESTART_NOTE),
    })
}

/// Writes the property list. Returns the report word and whether the file changed.
fn plist_step(install: &Install, dry_run: bool) -> Result<(&'static str, bool)> {
    if dry_run {
        let word = if service::plist_is_current(install) {
            "unchanged"
        } else {
            "would write"
        };
        return Ok((word, false));
    }
    let written = service::write_plist(install)
        .with_context(|| format!("cannot write {}", install.plist.display()))?;
    Ok((if written { "written" } else { "unchanged" }, written))
}

fn hook_step(install: &Install, dry_run: bool) -> Result<&'static str> {
    Ok(service::set_session_hook(&install.settings, dry_run)?.text(dry_run))
}

/// Starts the agent, waits for its status, then sets the settings keys. Nothing is set when the
/// status does not answer within `wait`. `reload` says that the property list changed, so a loaded
/// agent must be booted out first: kickstart would keep the old definition.
fn start_steps(
    install: &Install,
    lctl: &dyn Launchctl,
    config: &Config,
    wait: Duration,
    reload: bool,
) -> Result<Vec<Item>> {
    let verb = launch(install, lctl, reload)?;
    wait_or_fail(install, config.port, wait)?;
    let mut items = vec![
        Item::new("agent", verb),
        Item::new("status", format!("answers on 127.0.0.1:{}", config.port)),
        Item::new("refresh", refresh(config.port)),
    ];
    items.extend(set_env_items(install, config)?);
    Ok(items)
}

/// Bootstraps the agent, or kickstarts it when launchd has it loaded. Returns the report word.
fn launch(install: &Install, lctl: &dyn Launchctl, reload: bool) -> Result<&'static str> {
    let (domain, target) = install.launchd().context(NO_LAUNCHD)?;
    let plist = install.plist.display().to_string();
    let loaded = service::is_loaded(lctl, install);
    if loaded && reload {
        bootout(install, lctl)?;
        wait_unloaded(install, lctl, UNLOAD_WAIT, UNLOAD_POLL)?;
    }
    let call = if loaded && !reload {
        lctl.run(&["kickstart", "-k", target.as_str()])
    } else {
        lctl.run(&["bootstrap", domain, plist.as_str()])
    };
    match call {
        Ok(Call { ok: true, .. }) => Ok(if !loaded {
            "started"
        } else if reload {
            "reloaded"
        } else {
            "restarted"
        }),
        Ok(call) => Err(Hinted::new(
            format!("launchctl refused the agent: {}", call.stderr),
            log_hint(install),
        )
        .into()),
        Err(e) => Err(Hinted::new(format!("cannot run launchctl: {e}"), log_hint(install)).into()),
    }
}

/// Restarts the agent on the port of `config` when launchd has it loaded, and waits for its status
/// there. Returns the report word, or `None` when the agent is not loaded.
///
/// # Errors
/// Fails when launchctl refuses the restart or the status does not answer within `wait`.
pub(crate) fn restart_if_loaded(
    install: &Install,
    lctl: &dyn Launchctl,
    config: &Config,
    wait: Duration,
) -> Result<Option<&'static str>> {
    if !service::is_loaded(lctl, install) {
        return Ok(None);
    }
    restart(install, lctl, config, wait).map(Some)
}

/// Restarts the agent on the port of `config` and waits for its status there, whether or not
/// launchd has it loaded. Returns the report word.
///
/// # Errors
/// Fails when launchctl refuses the restart or the status does not answer within `wait`.
pub(crate) fn restart(
    install: &Install,
    lctl: &dyn Launchctl,
    config: &Config,
    wait: Duration,
) -> Result<&'static str> {
    let verb = launch(install, lctl, true)?;
    wait_or_fail(install, config.port, wait)?;
    Ok(verb)
}

/// Polls launchd until it no longer has the agent loaded, for `timeout` at most. A job that is
/// still being torn down refuses a bootstrap with an I/O error (launchctl exit 5).
fn wait_unloaded(
    install: &Install,
    lctl: &dyn Launchctl,
    timeout: Duration,
    poll: Duration,
) -> Result<()> {
    let deadline = Instant::now() + timeout;
    while service::is_loaded(lctl, install) {
        if Instant::now() >= deadline {
            return Err(Hinted::new(
                format!(
                    "launchd still has the agent loaded {:.1} s after bootout",
                    timeout.as_secs_f32()
                ),
                "wait a few seconds, then run `vicarius self start` again",
            )
            .into());
        }
        thread::sleep(poll);
    }
    Ok(())
}

/// Waits for the status on `port`, and fails with a hint to the log and `doctor` when it does not
/// answer within `wait`.
fn wait_or_fail(install: &Install, port: u16, wait: Duration) -> Result<()> {
    if wait_for_status(port, wait) {
        return Ok(());
    }
    Err(Hinted::new(
        format!(
            "vicarius does not answer on 127.0.0.1:{port} within {:.1} s",
            wait.as_secs_f32()
        ),
        log_hint(install),
    )
    .into())
}

/// Polls the status endpoint until it answers or `wait` has passed.
fn wait_for_status(port: u16, wait: Duration) -> bool {
    let deadline = Instant::now() + wait;
    loop {
        if status_client::fetch(port, PROBE).is_some() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        thread::sleep(Duration::from_millis(100));
    }
}

/// Asks the service for one balance refresh. A service without the endpoint is not an error.
fn refresh(port: u16) -> String {
    match status_client::request(port, "POST", status_client::REFRESH, REFRESH) {
        Ok(reply) if reply.status == 200 => "balance and agents refreshed".to_owned(),
        Ok(reply) if reply.status == 404 => "not available in this build".to_owned(),
        Ok(reply) => format!("answered {}", reply.status),
        Err(e) => format!("failed: {e}"),
    }
}

fn set_env_items(install: &Install, config: &Config) -> Result<Vec<Item>> {
    let steps = service::set_env_keys(&install.settings, config)?;
    Ok(steps
        .into_iter()
        .map(|(name, step)| Item::new(name, step.text(false)))
        .collect())
}

/// Removes the settings keys the service set, then unloads the agent.
fn stop_steps(install: &Install, lctl: &dyn Launchctl, config: &Config) -> Result<Vec<Item>> {
    let mut items: Vec<Item> = service::remove_env_keys(&install.settings, config)?
        .into_iter()
        .map(|(name, removal)| Item::new(name, removal.text(false)))
        .collect();
    items.push(Item::new("agent", bootout(install, lctl)?));
    Ok(items)
}

/// Unloads the agent when launchd has it loaded. Returns the report word.
fn bootout(install: &Install, lctl: &dyn Launchctl) -> Result<&'static str> {
    if !service::is_loaded(lctl, install) {
        return Ok("not loaded");
    }
    let (_, target) = install.launchd().context(NO_LAUNCHD)?;
    match lctl.run(&["bootout", target.as_str()]) {
        Ok(Call { ok: true, .. }) => Ok("stopped"),
        Ok(call) => Err(Hinted::new(
            format!("launchctl could not stop the agent: {}", call.stderr),
            log_hint(install),
        )
        .into()),
        Err(e) => Err(Hinted::new(format!("cannot run launchctl: {e}"), log_hint(install)).into()),
    }
}

fn ensure_macos() -> Result<()> {
    if cfg!(target_os = "macos") {
        return Ok(());
    }
    Err(Hinted::new(
        "the vicarius service is set up on macOS only so far",
        "run `vicarius serve` by hand; Linux and Windows come later",
    )
    .into())
}

fn load_config() -> Result<Config> {
    Config::load_from(&Config::path()).hinted()
}

fn log_hint(install: &Install) -> String {
    format!(
        "read {} and run `vicarius self doctor`",
        install.log.display()
    )
}
