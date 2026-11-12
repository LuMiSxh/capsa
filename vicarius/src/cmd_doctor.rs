// SPDX-License-Identifier: MPL-2.0
//! `vicarius self doctor`: checks the binary and the launchd agent, the service, the settings, the
//! derived agent files and the config. It exits 1 when a check fails; warnings do not change the
//! exit code.

use std::borrow::Cow;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::Result;
use capsa_core::claude_settings::{self, Settings, SettingsError};
use capsa_core::doctor::{check, dir_check, emit_report, log_checks, ok, same_file};
use capsa_core::exit::Outcome;
use capsa_core::format::Format;
use capsa_core::hint::Hint;
use capsa_core::hooks::{Agent, HookFile, HookState};
use capsa_core::render::{Check, Doctor, Group, Status};
use capsa_core::which;
use serde_json::Value;
use vicarius::config::Config;
use vicarius::proxy::LOG_LIMIT;

use crate::service::{self, Install, Launchctl, System};
use crate::status_client::{self, AgentStatus, Fetched};

/// The tool name as the doctor report and the PATH lookup name it.
const TOOL: &str = "vicarius";
/// The probe timeout of the status endpoint.
const PROBE: Duration = Duration::from_secs(1);

#[derive(clap::Args)]
pub struct Args {
    /// Repairs the SessionStart hook when it is outdated, or missing from an installed agent. The
    /// property list, the env keys and the service keep their hints, since the list starts the
    /// service at login and `self setup` and `self start` own those.
    #[arg(long)]
    pub fix: bool,
}

pub fn run(format: Format, fix: bool) -> Result<Outcome> {
    let install = Install::current()?;
    let fixed = fix.then(|| repair(&install)).transpose()?;
    let config = Config::load_from(&Config::path());
    let port = config.as_ref().map_or(Config::default().port, |c| c.port);
    let fetched = status_client::fetch(port, PROBE);
    let settings = claude_settings::read(&install.settings);
    let expected = config
        .as_ref()
        .map_or_else(|_| Vec::new(), |c| service::env_values(c).to_vec());
    let base = format!("http://127.0.0.1:{port}");
    let points_here = settings
        .as_ref()
        .ok()
        .and_then(|s| claude_settings::env_value(&s.value, service::BASE_URL))
        == Some(base.as_str());
    let (plist, program) = plist_check(&install);
    let path = path_check(&install, program);
    let mut service_checks = vec![
        plist,
        loaded_check(&install, &System),
        endpoint_check(port, fetched.is_some(), points_here, &install),
        gate_check(fetched.as_ref()),
    ];
    service_checks.extend(refresh_check(fetched.as_ref()));
    let groups = vec![
        install_group(&install, path),
        log_group(),
        Group {
            name: "service",
            checks: service_checks,
        },
        settings_group(&settings, &expected),
        agents_group(fetched.as_ref()),
        config_group(&config),
    ];
    emit_report(
        &Doctor::new(TOOL, env!("CARGO_PKG_VERSION"), groups),
        fixed,
        format,
    )
}

fn install_group(install: &Install, path: Check) -> Group {
    let binary = ok("binary", install.exe.display().to_string());
    Group {
        name: "install",
        checks: vec![binary, path],
    }
}

/// Checks the proxy log: that its directory takes files, that it takes lines, its size against
/// the rotation limit and the age of its last entry.
fn log_group() -> Group {
    let path = vicarius::proxy_log_file();
    let mut checks = Vec::new();
    if let Some(dir) = path.parent() {
        checks.push(dir_check(
            "log directory",
            dir,
            "make the log directory writable, or remove it so vicarius recreates it",
        ));
    }
    checks.extend(log_checks("proxy", &path, LOG_LIMIT));
    Group {
        name: "log",
        checks,
    }
}

/// Reads the program that the property list runs. The `PATH` check compares against it.
fn plist_check(install: &Install) -> (Check, Option<PathBuf>) {
    match fs::read_to_string(&install.plist) {
        Ok(text) => {
            let program = service::program_path(&text);
            let current = text == install.plist_text();
            let found = if current {
                ok("plist", install.plist.display().to_string())
            } else {
                check(
                    "plist",
                    Status::Warn,
                    "the property list is out of date",
                    Some("run `vicarius self setup` to rewrite it"),
                )
            };
            (found, program)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (
            check(
                "plist",
                Status::Warn,
                "the launchd agent is not installed",
                Some("run `vicarius self setup`"),
            ),
            None,
        ),
        Err(e) => (
            check(
                "plist",
                Status::Fail,
                format!("cannot read the property list: {e}"),
                None,
            ),
            None,
        ),
    }
}

fn path_check(install: &Install, program: Option<PathBuf>) -> Check {
    let expected = program.unwrap_or_else(|| install.exe.clone());
    match which::find(TOOL) {
        Some(found) if same_file(&found, &expected) => ok("PATH", found.display().to_string()),
        Some(found) => check(
            "PATH",
            Status::Warn,
            format!(
                "vicarius on PATH is {}, the agent runs {}",
                found.display(),
                expected.display()
            ),
            Some("remove the other copy, or run `vicarius self setup` from the one on PATH"),
        ),
        None => check(
            "PATH",
            Status::Warn,
            "vicarius is not on PATH",
            Some("the hook runs bare `vicarius`; add the directory of this binary to PATH"),
        ),
    }
}

fn loaded_check(install: &Install, lctl: &dyn Launchctl) -> Check {
    match install.launchd() {
        Some((_, target)) if service::is_loaded(lctl, install) => {
            ok("launchd", format!("loaded as {target}"))
        }
        _ => check(
            "launchd",
            Status::Warn,
            "the agent is not loaded",
            Some("run `vicarius self start`"),
        ),
    }
}

fn endpoint_check(port: u16, answering: bool, points_here: bool, install: &Install) -> Check {
    let addr = format!("127.0.0.1:{port}");
    if answering {
        ok("status", format!("answers on {addr}"))
    } else if points_here {
        let hint = format!(
            "read {} and run `vicarius self start`",
            install.log.display()
        );
        check(
            "status",
            Status::Fail,
            format!("the settings point at {addr}, which does not answer"),
            Some(hint.as_str()),
        )
    } else {
        check(
            "status",
            Status::Warn,
            format!("does not answer on {addr}"),
            Some("run `vicarius self start`"),
        )
    }
}

fn gate_check(fetched: Option<&Fetched>) -> Check {
    let Some(fetched) = fetched else {
        return check(
            "gate",
            Status::Warn,
            "unknown: the service does not answer",
            None,
        );
    };
    let gate = &fetched.status.gate;
    if gate.open {
        let detail = gate.balance.map_or_else(
            || "open".to_owned(),
            |amount| {
                format!(
                    "open, balance {}",
                    status_client::money(amount, gate.currency.as_deref())
                )
            },
        );
        return ok("gate", detail);
    }
    let reason = gate.reason_text.as_deref().unwrap_or("closed");
    let hint = match gate.reason.as_deref() {
        Some("no_key") => Some(
            "put DEEPSEEK_API_KEY in ~/.config/secrets.env (zsh sources it for the service), then run `vicarius self start`",
        ),
        Some("not_checked") => None,
        _ => Some("top up the DeepSeek balance or fix the key; the next refresh reopens the route"),
    };
    check("gate", Status::Warn, format!("closed: {reason}"), hint)
}

fn refresh_check(fetched: Option<&Fetched>) -> Option<Check> {
    let refresh = fetched?.status.refresh.as_ref()?;
    Some(match &refresh.last_error {
        Some(error) => check(
            "refresh",
            Status::Warn,
            format!("the last refresh failed: {error}"),
            Some("check the network and the DeepSeek key; the next refresh retries"),
        ),
        None => {
            let detail = refresh.last_ok_at.map_or_else(
                || "no refresh yet".to_owned(),
                |at| format!("last ok {}", status_client::relative(at)),
            );
            ok("refresh", detail)
        }
    })
}

fn settings_group(
    settings: &Result<Settings, SettingsError>,
    expected: &[(&'static str, String)],
) -> Group {
    let checks = match settings {
        Err(e) => vec![check(
            "settings",
            Status::Fail,
            e.to_string(),
            e.hint().as_deref(),
        )],
        Ok(settings) => {
            let mut checks: Vec<Check> = expected
                .iter()
                .map(|(key, value)| env_check(key, &settings.value, value))
                .collect();
            checks.push(hook_check(&settings.path));
            checks
        }
    };
    Group {
        name: "settings",
        checks,
    }
}

fn env_check(key: &'static str, settings: &Value, expected: &str) -> Check {
    match claude_settings::env_value(settings, key) {
        Some(value) if value == expected => ok(key, value.to_owned()),
        Some(value) => check(
            key,
            Status::Warn,
            format!("set to {value}, expected {expected}"),
            Some("remove or change it in ~/.claude/settings.json, then run `vicarius self start`"),
        ),
        None => check(
            key,
            Status::Warn,
            "not set",
            Some("run `vicarius self start`"),
        ),
    }
}

fn hook_check(settings: &Path) -> Check {
    match HookFile::open_at(Agent::ClaudeCode, settings) {
        Ok(file) => file
            .state(&service::SESSION_HOOK)
            .check(&service::SESSION_HOOK, "vicarius self setup"),
        Err(e) => check("SessionStart hook", Status::Fail, e.to_string(), None),
    }
}

/// Rewrites the SessionStart hook when it is outdated, or when it is missing from an agent that
/// was set up. Returns the check for the hook, or nothing when the hook was left alone.
fn repair(install: &Install) -> Result<Vec<Check>> {
    let Ok(file) = HookFile::open_at(Agent::ClaudeCode, &install.settings) else {
        return Ok(Vec::new());
    };
    let wanted = match file.state(&service::SESSION_HOOK) {
        HookState::Outdated { .. } => true,
        HookState::Missing => install.plist.exists(),
        HookState::Installed | HookState::CustomMatcher { .. } => false,
    };
    if !wanted {
        return Ok(Vec::new());
    }
    let step = service::set_session_hook(&install.settings, false)?;
    let detail = format!("{} in {}", step.text(false), install.settings.display());
    Ok(vec![ok("SessionStart hook", detail)])
}

fn agents_group(fetched: Option<&Fetched>) -> Group {
    let checks = match fetched {
        None => vec![check(
            "agents",
            Status::Warn,
            "not checked: the service does not answer",
            None,
        )],
        Some(fetched) => match &fetched.status.agents {
            None => vec![check(
                "agents",
                Status::Warn,
                "not reported by this service",
                None,
            )],
            Some(agents) if agents.entries.is_empty() => {
                vec![ok("agents", "none configured")]
            }
            Some(agents) => {
                let problems: Vec<Check> =
                    agents.entries.iter().filter_map(agent_problem).collect();
                if problems.is_empty() {
                    let detail = format!("{} agent files ok", agents.entries.len());
                    vec![ok("agents", detail)]
                } else {
                    problems
                }
            }
        },
    };
    Group {
        name: "agents",
        checks,
    }
}

fn agent_problem(entry: &AgentStatus) -> Option<Check> {
    let agent = entry.agent.as_str();
    let (detail, hint) = match entry.outcome_name() {
        "conflict" => (
            "a hand-written file holds the name".to_owned(),
            format!(
                "rename or delete {}; vicarius never overwrites it",
                entry.file
            ),
        ),
        "unparsable" => (
            format!(
                "cannot derive from {agent}.md: {}",
                entry.outcome_detail().unwrap_or("")
            ),
            format!("fix the frontmatter of {agent}.md, then run `vicarius self start`"),
        ),
        "source_missing" => (
            format!("{agent}.md is missing from ~/.claude/agents"),
            format!("create it, or run `vicarius self config agent remove {agent}`"),
        ),
        "error" => (
            format!("the sync failed: {}", entry.outcome_detail().unwrap_or("")),
            "check the permissions of ~/.claude/agents, then read service.log".to_owned(),
        ),
        _ => return None,
    };
    Some(check(
        format!("agent {agent}"),
        Status::Warn,
        detail,
        Some(hint.as_str()),
    ))
}

fn config_group(config: &Result<Config, vicarius::config::ConfigError>) -> Group {
    let mut checks = Vec::new();
    match config {
        Ok(_) => checks.push(ok("config", Config::path().display().to_string())),
        Err(e) => {
            let hint = e.hint().map(Cow::into_owned);
            checks.push(check(
                "config",
                Status::Fail,
                e.to_string(),
                hint.as_deref(),
            ));
        }
    }
    if let Some(old) = capsa_core::config::legacy_json("vicarius") {
        let hint = format!(
            "move its settings with `vicarius self config set`; the config is now {}",
            Config::path().display()
        );
        checks.push(check(
            "config.json",
            Status::Warn,
            format!("{} is no longer read", old.display()),
            Some(hint.as_str()),
        ));
    }
    Group {
        name: "config",
        checks,
    }
}
