// SPDX-License-Identifier: MPL-2.0
//! Entry point: runs the ACP proxy, or one `pons self` action.

mod cli;
mod doctor;
mod engine;
mod failsafe;
mod fold;
mod launcher;
mod log;
mod notification;
mod permission;
mod proxy;
mod roster;
mod setup;
mod state;

use std::process::ExitCode;
use std::time::Duration;

use anyhow::Result;
use capsa_core::exit::{self, Outcome};
use capsa_core::hint::ResultExt;
use capsa_core::pkg::Manager;
use capsa_core::update::{self, Tool};
use capsa_core::which::{Env, RealFs};
use clap::Parser;

use crate::cli::{Cli, SelfAction, Sub};
use crate::engine::{Config, NoticeMode};
use crate::log::{Dir, Logger};

fn main() -> ExitCode {
    // Errors are rendered on stderr with `capsa_core::exit`; stdout is the ACP
    // wire in proxy mode, so nothing is printed there on failure.
    let result = run_on_runtime();
    exit::finish(result, capsa_core::format::resolve(false, false, false))
}

fn run_on_runtime() -> Result<Outcome> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let result = runtime.block_on(run());
    // Dropping the runtime would wait for the blocking read of stdin, which
    // never returns while the client keeps its end open (the agent died, say).
    runtime.shutdown_background();
    result
}

/// This binary as the self-update and uninstall code sees it.
const TOOL: Tool = Tool {
    name: "pons",
    version: env!("CARGO_PKG_VERSION"),
};

/// Runs a `pons self` action.
fn subcommand(sub: Sub) -> Result<Outcome> {
    let Sub::SelfCmd { action } = sub;
    match action {
        SelfAction::Doctor {
            json,
            human,
            agent,
            log,
            log_dir,
            agent_cmd,
        } => {
            let format = capsa_core::format::resolve(human, json, agent);
            print!(
                "{}",
                doctor::run(agent_cmd, log, log_dir.as_deref(), format)
            );
        }
        SelfAction::Setup {
            json,
            human,
            agent,
            package_manager,
            dry_run,
        } => {
            let format = capsa_core::format::resolve(human, json, agent);
            let step = setup::run(package_manager.map(Manager::from), dry_run)?;
            print!("{}", setup::render(&step, format));
        }
        SelfAction::Update(args) => {
            let report = update::update(&TOOL, args.mode()).hinted()?;
            println!("{report}");
            return Ok(report.outcome(args.check));
        }
        SelfAction::Uninstall(args) => {
            println!("{}", update::uninstall(&TOOL, args.dry_run).hinted()?);
        }
    }
    Ok(Outcome::Found)
}

async fn run() -> Result<Outcome> {
    let cli = Cli::parse();
    if let Some(sub) = cli.sub {
        return subcommand(sub);
    }
    let log_path = log::target(cli.log.as_deref(), cli.no_log, cli.log_dir.as_deref());
    // Only an explicit file rotates; default per-start files are bounded by
    // how many are kept instead.
    let max_bytes = if cli.log.is_some() {
        cli.log_max_bytes
    } else {
        0
    };
    let logger = Logger::open(log_path.as_deref(), cli.log_level, max_bytes)?;
    logger.log_panics();
    let cfg = Config {
        cancel_filter: !cli.no_cancel_filter,
        fold: !cli.no_fold,
        cancel_defer: Duration::from_millis(cli.cancel_defer_ms),
        interrupt: cli.interrupt_mode,
        stop_window: Duration::from_millis(cli.stop_window_ms),
        subagent_idle: Duration::from_millis(cli.subagent_idle_ms),
        subagent_max_quiet: Duration::from_millis(cli.subagent_max_quiet_ms),
        shutdown_grace: Duration::from_millis(cli.shutdown_grace_ms),
        finish_entry: !cli.no_finish_entry,
        entry_gap: Duration::from_millis(cli.entry_gap_ms),
        notify_settle: Duration::from_millis(cli.notify_settle_ms),
        roster: !cli.no_roster,
        drop_task_notifications: !cli.keep_task_notifications,
        drop_interrupt_markers: !cli.keep_interrupt_markers,
        notices: if cli.no_notice {
            NoticeMode::Never
        } else if cli.notices {
            NoticeMode::Always
        } else {
            NoticeMode::Auto
        },
        force_features: cli.force_features,
        subagent_text: !cli.no_subagent_text,
        max_failures: cli.max_failures,
        inject_failure: cli.inject_failure,
        turn_stall: Duration::from_millis(cli.turn_stall_ms),
        close_ghost_calls: !cli.no_close_ghost_calls,
        steer_hold: Duration::from_millis(cli.steer_hold_ms),
        auto_approve: cli.auto_approve,
        auto_approve_tools: cli.auto_approve_tools,
    };
    let note_error =
        |e: &anyhow::Error| logger.log(Dir::Pons, format!("exiting: error: {e:#}").as_bytes());
    let spawn = launcher::plan(&Env::from_process(), &RealFs, &cli.agent)
        .map_err(anyhow::Error::msg)
        .inspect_err(note_error)?;
    logger.log(Dir::Pons, format!("agent: {}", spawn.how).as_bytes());
    if cli.quiet_ms.is_some() {
        logger.log(
            Dir::Pons,
            b"--quiet-ms is deprecated and ignored: a cancel no longer depends on main agent activity (see --cancel-defer-ms)",
        );
    }
    let code = proxy::run(cfg, spawn, logger.clone())
        .await
        .inspect_err(note_error)?;
    Ok(Outcome::Code(u8::try_from(code).unwrap_or(1)))
}
