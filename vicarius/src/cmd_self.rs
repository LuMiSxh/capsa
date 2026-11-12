// SPDX-License-Identifier: MPL-2.0
//! `vicarius self`: the settings, and the update and removal of the binary.

use anyhow::Result;
use capsa_core::exit::Outcome;
use capsa_core::format::Format;
#[cfg(feature = "cmd-update")]
use capsa_core::update::{UninstallArgs, UpdateArgs};

/// This binary as the self-update and uninstall code sees it.
#[cfg(feature = "cmd-update")]
const TOOL: capsa_core::update::Tool = capsa_core::update::Tool {
    name: "vicarius",
    version: env!("CARGO_PKG_VERSION"),
};

#[derive(clap::Args)]
pub struct Args {
    #[command(subcommand)]
    action: Action,
}

#[derive(clap::Subcommand)]
enum Action {
    /// Replace this binary with the latest release.
    Update(UpdateArgs),
    /// Stop the service, remove its agent and hook, then remove this binary. The config directory
    /// is kept.
    Uninstall(UninstallArgs),
    /// Install the launchd agent and the SessionStart hook, then start the service.
    Setup(crate::cmd_service::SetupArgs),
    /// Start the service and point Claude Code at it.
    Start,
    /// Stop the service and remove the settings keys that it set.
    Stop,
    /// Check the agent, the service, the settings, the agent files and the config.
    Doctor(crate::cmd_doctor::Args),
    /// Show the settings, or change them with `set`, `agent add` and `agent remove`.
    Config(crate::cmd_config::Args),
}

#[cfg(not(feature = "cmd-update"))]
#[derive(clap::Args)]
pub struct UpdateArgs {}

#[cfg(not(feature = "cmd-update"))]
#[derive(clap::Args)]
pub struct UninstallArgs {
    /// Print what would be removed and remove nothing.
    #[arg(long)]
    dry_run: bool,
}

pub fn run(a: Args, format: Format) -> Result<Outcome> {
    match a.action {
        Action::Update(a) => update(a, format),
        Action::Uninstall(a) => uninstall(&a, format),
        Action::Config(a) => crate::cmd_config::run(a, format),
        Action::Setup(a) => crate::cmd_service::setup(&a, format),
        Action::Start => crate::cmd_service::start(format),
        Action::Stop => crate::cmd_service::stop(format),
        Action::Doctor(a) => crate::cmd_doctor::run(format, a.fix),
    }
}

#[cfg(not(feature = "cmd-update"))]
fn update(_: UpdateArgs, _: Format) -> Result<Outcome> {
    Err(capsa_core::hint::update_unavailable("vicarius", "vicarius"))
}

#[cfg(feature = "cmd-update")]
fn update(a: UpdateArgs, format: Format) -> Result<Outcome> {
    capsa_core::update::run(&TOOL, a, format, |_| Ok(()))
}

#[cfg(not(feature = "cmd-update"))]
fn uninstall(_: &UninstallArgs, _: Format) -> Result<Outcome> {
    Err(capsa_core::hint::uninstall_unavailable("vicarius"))
}

#[cfg(feature = "cmd-update")]
fn uninstall(a: &UninstallArgs, format: Format) -> Result<Outcome> {
    if !a.dry_run {
        crate::cmd_service::remove_integration()?;
        vicarius::bulletin::withdraw(&capsa_core::bulletin::dir())?;
    }
    capsa_core::update::uninstall_run(&TOOL, a.dry_run, format, |_| Ok(()))
}
