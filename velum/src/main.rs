// SPDX-License-Identifier: MPL-2.0
//! Owns the `velum` command line: it parses the arguments and dispatches each action.

mod check;
mod claude_json;
mod claude_version;
mod doctor;
mod gitguard;
mod policy;
mod proc;
mod rundir;
mod safedir;
#[cfg(test)]
mod scratch;
mod setup;
mod shim;
mod srt;
mod uninstall;
mod writable;

use anyhow::{Context, Result};
use capsa_core::exit::{Outcome, finish, reset_sigpipe};
use capsa_core::format::{Format, resolve};
use capsa_core::hint::{Hinted, ResultExt};
use capsa_core::update::{UninstallArgs, UpdateArgs};
use clap::{Parser, Subcommand};
use policy::Policy;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::SystemTime;

/// Run commands under the srt sandbox with a filtered environment.
#[derive(Parser)]
#[command(name = "velum", version)]
struct Cli {
    /// Plain text for coding agents (the default).
    #[arg(long, global = true)]
    agent: bool,
    /// Coloured, structured text for people, on a terminal.
    #[arg(long, global = true)]
    human: bool,
    /// One stable JSON document.
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    action: Action,
}

#[derive(Subcommand)]
enum Action {
    /// Run a command under the sandbox policy.
    Run {
        /// Policy file (default: ~/.config/velum/policy.toml).
        #[arg(long, value_name = "PATH")]
        policy: Option<PathBuf>,
        /// Print the sandbox settings and environment names, then exit.
        #[arg(long)]
        dry_run: bool,
        /// Command to run, after `--`.
        #[arg(
            required = true,
            trailing_var_arg = true,
            allow_hyphen_values = true,
            value_name = "CMD"
        )]
        cmd: Vec<String>,
    },
    /// Installation and health of velum itself: `self doctor`, `self setup`, `self update`,
    /// `self uninstall`.
    #[command(name = "self")]
    SelfCmd {
        #[command(subcommand)]
        action: SelfAction,
    },
    /// Install, remove or inspect the `claude` shim.
    Shim {
        #[command(subcommand)]
        action: ShimAction,
    },
}

#[derive(Subcommand)]
enum SelfAction {
    /// Install srt if it is missing and write the default policy file.
    Setup {
        /// Package manager that installs srt.
        #[arg(long, value_enum, default_value_t = setup::PackageManager::Pnpm)]
        package_manager: setup::PackageManager,
        /// Print what would be written or installed, then exit.
        #[arg(long)]
        dry_run: bool,
    },
    /// Check srt, the policy, the settings, claude on PATH, the shim and TMPDIR.
    Doctor,
    /// Replace this binary with the latest GitHub release.
    Update(UpdateArgs),
    /// Remove the `claude` shim and this binary. The policy file stays unless `--purge`.
    Uninstall {
        #[command(flatten)]
        args: UninstallArgs,
        /// Also remove the policy file (~/.config/velum/policy.toml).
        #[arg(long)]
        purge: bool,
        /// Directory of the shim (default: ~/.local/bin).
        #[arg(long, value_name = "DIR")]
        dir: Option<PathBuf>,
    },
}

#[derive(Subcommand)]
enum ShimAction {
    /// Write the shim into DIR.
    Install {
        /// Directory for the shim (default: ~/.local/bin).
        #[arg(long, value_name = "DIR")]
        dir: Option<PathBuf>,
        /// Print the script and where it goes, then exit.
        #[arg(long)]
        dry_run: bool,
    },
    /// Remove the shim from DIR, if velum wrote it.
    Uninstall {
        /// Directory of the shim (default: ~/.local/bin).
        #[arg(long, value_name = "DIR")]
        dir: Option<PathBuf>,
        /// Print what would be removed, then exit.
        #[arg(long)]
        dry_run: bool,
    },
    /// Show whether the shim is installed and where its directory sits on PATH.
    Status {
        /// Directory of the shim (default: ~/.local/bin).
        #[arg(long, value_name = "DIR")]
        dir: Option<PathBuf>,
    },
}

fn main() -> ExitCode {
    reset_sigpipe();
    let cli = Cli::parse();
    let format = resolve(cli.human, cli.json, cli.agent);
    finish(real_main(cli.action, format), format)
}

fn real_main(action: Action, format: Format) -> Result<Outcome> {
    match action {
        Action::Run {
            policy,
            dry_run,
            cmd,
        } => run(policy, dry_run, &cmd),
        Action::SelfCmd { action } => self_action(action, format),
        Action::Shim { action } => shim_action(action, format),
    }
}

fn self_action(action: SelfAction, format: Format) -> Result<Outcome> {
    match action {
        SelfAction::Setup {
            package_manager,
            dry_run,
        } => {
            let home = home()?;
            setup::run(
                package_manager,
                dry_run,
                &policy::default_path(&home),
                &home,
            )?;
            Ok(Outcome::Found)
        }
        SelfAction::Doctor => doctor::run(format),
        SelfAction::Update(args) => {
            let report = capsa_core::update::update(&uninstall::TOOL, args.mode()).hinted()?;
            println!("{report}");
            Ok(report.outcome(args.check))
        }
        SelfAction::Uninstall { args, purge, dir } => {
            uninstall::run(&shim_dir(dir)?, &home()?, purge, args.dry_run)
        }
    }
}

/// Runs `cmd` under srt, or prints the settings with `--dry-run`. Returns the child's exit code.
fn run(policy: Option<PathBuf>, dry_run: bool, cmd: &[String]) -> Result<Outcome> {
    let cwd = std::env::current_dir().context("reading current directory")?;
    let home = home()?;
    policy::check_cwd(&cwd, &home).hinted()?;
    let policy_path = policy.unwrap_or_else(|| policy::default_path(&home));
    let mut policy = Policy::load(&policy_path, &cwd).hinted()?;
    // Hooks and config of the repositories that exist now are read-only; new ones are not.
    let git = gitguard::Start::capture(&cwd);
    let extra: Vec<String> = git
        .deny_entries()
        .into_iter()
        .filter(|entry| !policy.deny_write.contains(entry))
        .collect();
    policy.deny_write.extend(extra);
    git.warning().iter().for_each(|line| eprintln!("{line}"));
    let settings = srt::settings(&policy, &home).hinted()?;
    let mut env = srt::filtered_env(&policy);

    if dry_run {
        println!("{}", serde_json::to_string_pretty(&settings)?);
        println!("env:");
        for (name, _) in &env {
            println!("  {name}");
        }
        return Ok(Outcome::Found);
    }

    let path = std::env::var_os("PATH").unwrap_or_default();
    claude_version::enforce(cmd, &path, &home)?;
    let srt_bin = srt::find_srt(&home, &path).ok_or_else(srt_not_found)?;
    writable::ensure_not_writable(&settings, "srt", &srt_bin).hinted()?;
    rundir::remove_stale(&home, SystemTime::now(), rundir::STALE_AFTER);
    proc::install_forwarding();
    // The run directory is removed when `run_dir` drops, on every path out of here.
    let run_dir = rundir::RunDir::create(&home)?;
    run_dir.ensure_not_writable(&settings).hinted()?;
    srt::install_wrappers(&mut env, &run_dir)?;
    srt::install_git_env(&mut env, &run_dir)?;
    let guard = claude_json::Guard::snapshot(&home)?;
    let result = srt::run(&srt_bin, &run_dir, &settings, env, cmd);
    // After the child, and its surviving processes, are gone: undo and inspect what it did.
    match guard.restore() {
        Ok(report) => report
            .warnings()
            .iter()
            .for_each(|line| eprintln!("{line}")),
        Err(err) => eprintln!("warning: could not check ~/.claude.json: {err:#}"),
    }
    for line in git.check(&home, &run_dir.name()) {
        eprintln!("{line}");
    }
    let ran = result?;
    if ran.killed_survivors {
        eprintln!(
            "warning: processes of the sandboxed command were still running after it exited and have been killed"
        );
    }
    Ok(Outcome::Code(ran.code))
}

fn srt_not_found() -> anyhow::Error {
    Hinted::new(
        "srt not found on PATH or in the user bin directories",
        "run `velum self setup` to install it",
    )
    .into()
}

fn shim_action(action: ShimAction, format: Format) -> Result<Outcome> {
    let path = std::env::var_os("PATH").unwrap_or_default();
    match action {
        ShimAction::Install { dir, dry_run } => {
            let velum = std::env::current_exe()
                .and_then(std::fs::canonicalize)
                .context("locating the velum binary")?;
            let dir = shim_dir(dir)?;
            let home = home()?;
            let cwd = std::env::current_dir().context("reading current directory")?;
            let policy = Policy::load(&policy::default_path(&home), &cwd).hinted()?;
            let settings = srt::settings(&policy, &home).hinted()?;
            shim::ensure_targets_not_writable(&settings, &dir, &path, &velum)?;
            shim::install(&dir, &path, &velum, dry_run)
        }
        ShimAction::Uninstall { dir, dry_run } => shim::uninstall(&shim_dir(dir)?, dry_run),
        ShimAction::Status { dir } => {
            let home = home()?;
            let cwd = std::env::current_dir().context("reading current directory")?;
            let mut checks = shim::status(&shim_dir(dir)?, &path);
            checks.push(doctor::trust_check(&home, &cwd, &path));
            check::report(format, &checks)
        }
    }
}

/// Returns the shim directory: `--dir`, else `~/.local/bin`.
fn shim_dir(dir: Option<PathBuf>) -> Result<PathBuf> {
    match dir {
        Some(dir) => Ok(dir),
        None => Ok(shim::default_dir(&home()?)),
    }
}

/// Returns `$HOME`, which must be a non-empty absolute path.
pub fn home() -> Result<String> {
    let home = std::env::var("HOME").context("HOME is not set")?;
    policy::validate_home(&home).map(drop).hinted()?;
    Ok(home)
}
