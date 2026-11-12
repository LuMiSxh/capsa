// SPDX-License-Identifier: MPL-2.0
//! `vicarius self config`: shows the settings, and changes one scalar or the derived agents,
//! syncing them at once.

use anyhow::Result;
use capsa_core::config::ConfigFile;
use capsa_core::exit::Outcome;
use capsa_core::format::Format;
use capsa_core::hint::{ResultExt, hinted};
use capsa_core::render::{Doc, Render, emit};
use serde::Serialize;
use std::str::FromStr;
use std::time::Duration;
use vicarius::agents::{self, Entry, Outcome as SyncOutcome};
use vicarius::config::{Agent, Config, ConfigError, Effort, KNOWN_MODELS, known_model};
use vicarius::refresh;

use crate::cmd_service::{self, Item, RESTART_NOTE, WAIT};
use crate::service::{self, Install, Launchctl, Replacement, System};
use crate::status_client;

/// The timeout of the status probe that decides whether the gate is open.
const SYNC_TIMEOUT: Duration = Duration::from_millis(300);
/// The note when the service does not answer, so the files wait for its next sync.
const SERVICE_DOWN: &str =
    "The service is not running; the derived files are written when it starts.";

#[cfg(test)]
mod tests;

#[derive(clap::Args)]
pub struct Args {
    #[command(subcommand)]
    action: Option<Action>,
}

#[derive(clap::Subcommand)]
enum Action {
    /// Change a scalar setting: `set <port|min-balance|balance-refresh|max-context> <value>`.
    Set(SetArgs),
    /// Add or remove a derived agent: `agent add <from> --model <id>` or `agent remove <from>`.
    Agent(AgentArgs),
}

#[derive(clap::Args)]
struct SetArgs {
    /// The setting to change.
    #[arg(value_enum)]
    key: Key,
    /// The new value.
    #[arg(allow_hyphen_values = true)]
    value: String,
}

#[derive(Clone, Copy, clap::ValueEnum)]
enum Key {
    Port,
    MinBalance,
    BalanceRefresh,
    MaxContext,
}

impl Key {
    /// The TOML key of this setting.
    const fn field(self) -> &'static str {
        match self {
            Key::Port => "port",
            Key::MinBalance => "min_balance_usd",
            Key::BalanceRefresh => "balance_refresh_minutes",
            Key::MaxContext => "max_context_tokens",
        }
    }

    /// Describes the values the key takes, for the error of a value it cannot read.
    fn expected(self) -> &'static str {
        match self {
            Key::Port => "a port number from 1 to 65535",
            Key::MinBalance => "an amount in USD, such as 1.0",
            Key::BalanceRefresh => "a whole number of minutes, such as 10",
            Key::MaxContext => "a whole number of tokens, such as 400000",
        }
    }
}

#[derive(clap::Args)]
struct AgentArgs {
    #[command(subcommand)]
    action: AgentAction,
}

#[derive(clap::Subcommand)]
enum AgentAction {
    /// Add the DeepSeek variant `<from>-ds`, which runs on `--model` and can override effort.
    Add {
        /// The agent file in `~/.claude/agents` to derive from, without `.md`.
        from: String,
        /// The DeepSeek model id, such as `deepseek-flash`.
        #[arg(long)]
        model: String,
        /// The effort to use, or `keep` to preserve the source effort.
        #[arg(long, value_enum, default_value = "keep")]
        effort: Effort,
    },
    /// Remove the derived agent of `<from>`.
    Remove {
        /// The source agent of the entry to remove.
        from: String,
    },
    /// Withdraw the derived agent of `<from>`, or every agent with `--all`, keeping the settings.
    Suspend(TargetArgs),
    /// Restore a suspended derived agent of `<from>`, or every agent with `--all`.
    Activate(TargetArgs),
}

#[derive(clap::Args)]
struct TargetArgs {
    /// The source agent of the entry.
    #[arg(required_unless_present = "all", conflicts_with = "all")]
    from: Option<String>,
    /// Apply to every configured agent.
    #[arg(long, required_unless_present = "from", conflicts_with = "from")]
    all: bool,
}

#[derive(Serialize)]
struct ConfigOut<'a> {
    #[serde(flatten)]
    config: &'a Config,
    path: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    steps: Vec<Item>,
    #[serde(skip_serializing_if = "Option::is_none")]
    note: Option<&'static str>,
}

impl Render for ConfigOut<'_> {
    fn doc(&self, doc: &mut Doc) {
        let c = self.config;
        doc.title("vicarius self config");
        doc.row("port").text(c.port.to_string());
        doc.row("min balance")
            .text(format!("${:.2}", c.min_balance_usd));
        doc.row("refresh")
            .text(format!("every {} min", c.balance_refresh_minutes));
        doc.row("context").text(format!(
            "{} tokens for the DeepSeek models",
            c.max_context_tokens
        ));
        if c.agents.is_empty() {
            doc.row("agents").dim("none");
        }
        for agent in &c.agents {
            let mut text = format!(
                "{}-ds on {} (effort: {}",
                agent.from,
                agent.model,
                agent.effort.as_str()
            );
            if agent.suspended {
                text.push_str(", suspended");
            }
            text.push(')');
            doc.row(format!("agent {}", agent.from)).text(text);
        }
        for step in &self.steps {
            doc.row(step.name.as_str()).text(step.status.as_str());
        }
        if let Some(note) = self.note {
            doc.line(note);
        }
        doc.row("file").dim(self.path.as_str());
    }
}

pub fn run(a: Args, format: Format) -> Result<Outcome> {
    let file = Config::file();
    let mut config = Config::load_from(file.path()).hinted()?;
    if a.action.is_none() {
        return emit(
            &file.show("vicarius self config", &config).hinted()?,
            format,
        );
    }
    let mut steps = Vec::new();
    let mut note = None;
    if let Some(action) = a.action {
        let old = config.clone();
        let agent_action = matches!(&action, Action::Agent(_));
        let new_model = match &action {
            Action::Agent(AgentArgs {
                action: AgentAction::Add { model, .. },
            }) => Some(model.clone()),
            _ => None,
        };
        let set = match &action {
            Action::Set(set) => Some((set.key, set.value.clone())),
            Action::Agent(_) => None,
        };
        change(&mut config, action).hinted()?;
        match set {
            Some((key, value)) if cfg!(target_os = "macos") => {
                let install = Install::current()?;
                steps = apply_steps(&install, &System, key, &value, &old, &config, WAIT)?;
            }
            Some((key, value)) => {
                file.set::<Config>(key.field(), &value).hinted()?;
            }
            None => {
                config.save_to(file.path()).hinted()?;
                if agent_action {
                    (steps, note) = sync_agents(&config);
                }
            }
        }
        if let Some(model) = new_model.filter(|model| !known_model(model)) {
            steps.push(Item::new(
                "model",
                format!(
                    "unknown model id `{model}`; known: {}",
                    KNOWN_MODELS.join(", ")
                ),
            ));
        }
    }
    let changed = steps
        .iter()
        .any(|item| item.status == Replacement::Replaced.text());
    let body = ConfigOut {
        config: &config,
        path: file.path().display().to_string(),
        steps,
        note: note.or_else(|| changed.then_some(RESTART_NOTE)),
    };
    emit(&body, format)
}

/// Applies `action` to `config`. The caller saves the config only when this returns `Ok`.
fn change(config: &mut Config, action: Action) -> Result<(), ConfigError> {
    match action {
        Action::Set(set) => {
            set_scalar(config, set.key, &set.value)?;
            config.validate()
        }
        Action::Agent(AgentArgs {
            action:
                AgentAction::Add {
                    from,
                    model,
                    effort,
                },
        }) => config.add_agent(Agent {
            from,
            model,
            effort,
            suspended: false,
        }),
        Action::Agent(AgentArgs {
            action: AgentAction::Remove { from },
        }) => config.remove_agent(&from),
        Action::Agent(AgentArgs {
            action: AgentAction::Suspend(target),
        }) => set_suspended(config, &target, true),
        Action::Agent(AgentArgs {
            action: AgentAction::Activate(target),
        }) => set_suspended(config, &target, false),
    }
}

/// Suspends or activates the agent `target` names, or every agent with `--all`.
fn set_suspended(
    config: &mut Config,
    target: &TargetArgs,
    suspended: bool,
) -> Result<(), ConfigError> {
    match &target.from {
        Some(from) => config.set_suspended(from, suspended),
        None => {
            config.set_all_suspended(suspended);
            Ok(())
        }
    }
}

/// Syncs the derived files now, as the service's tick would. Returns the steps and, when the
/// service does not answer, the note that says so.
fn sync_agents(config: &Config) -> (Vec<Item>, Option<&'static str>) {
    let Some(fetched) = status_client::fetch(config.port, SYNC_TIMEOUT) else {
        return (Vec::new(), Some(SERVICE_DOWN));
    };
    let report = agents::sync(
        &refresh::claude_agents_dir(),
        &config.agents,
        fetched.status.gate.open,
    );
    (report.entries.iter().map(sync_step).collect(), None)
}

/// Turns one sync entry into the step that shows it.
fn sync_step(entry: &Entry) -> Item {
    let name = if entry.agent.is_empty() {
        entry.file.clone()
    } else {
        format!("{}-ds", entry.agent)
    };
    Item::new(name, outcome_text(&entry.outcome))
}

fn outcome_text(outcome: &SyncOutcome) -> String {
    match outcome {
        SyncOutcome::Written => "written".to_owned(),
        SyncOutcome::Unchanged => "unchanged".to_owned(),
        SyncOutcome::Removed => "removed".to_owned(),
        SyncOutcome::SourceMissing => "source missing".to_owned(),
        SyncOutcome::Unparsable(e) => format!("unparsable: {e}"),
        SyncOutcome::Conflict => "conflict: not written".to_owned(),
        SyncOutcome::Error(e) => format!("error: {e}"),
    }
}

fn set_scalar(config: &mut Config, key: Key, value: &str) -> Result<(), ConfigError> {
    match key {
        Key::Port => config.port = read(value, key)?,
        Key::MinBalance => config.min_balance_usd = read(value, key)?,
        Key::BalanceRefresh => config.balance_refresh_minutes = read(value, key)?,
        Key::MaxContext => config.max_context_tokens = read(value, key)?,
    }
    Ok(())
}

/// Saves the changed config and brings the settings `env` in line with the changed `key`, from
/// `old` to `new`. A changed port takes the rollback path of `port_steps`.
fn apply_steps(
    install: &Install,
    lctl: &dyn Launchctl,
    key: Key,
    value: &str,
    old: &Config,
    new: &Config,
    wait: Duration,
) -> Result<Vec<Item>> {
    if matches!(key, Key::Port) && old.port != new.port {
        return port_steps(install, lctl, value, old, new, wait);
    }
    ConfigFile::at(install.config.clone())
        .set::<Config>(key.field(), value)
        .hinted()?;
    let (name, old_value, new_value) = match key {
        Key::Port => (
            service::BASE_URL,
            service::base_url(old.port),
            service::base_url(new.port),
        ),
        Key::MaxContext => (
            service::MAX_CONTEXT,
            old.max_context_tokens.to_string(),
            new.max_context_tokens.to_string(),
        ),
        Key::MinBalance | Key::BalanceRefresh => return Ok(Vec::new()),
    };
    let replacement = service::replace_env_value(&install.settings, name, &old_value, &new_value)?;
    Ok(vec![Item::new(name, replacement.text())])
}

/// Saves the new port before the restart, because the agent reads `config.toml` at boot. A failure
/// after the save rolls the port back.
fn port_steps(
    install: &Install,
    lctl: &dyn Launchctl,
    value: &str,
    old: &Config,
    new: &Config,
    wait: Duration,
) -> Result<Vec<Item>> {
    ConfigFile::at(install.config.clone())
        .set::<Config>("port", value)
        .hinted()?;
    repoint(install, lctl, old, new, wait)
        .map_err(|cause| roll_back_port(install, lctl, old, new, wait, cause))
}

/// Restarts a loaded agent on the new port, then points the settings at it.
fn repoint(
    install: &Install,
    lctl: &dyn Launchctl,
    old: &Config,
    new: &Config,
    wait: Duration,
) -> Result<Vec<Item>> {
    let mut items = Vec::new();
    if let Some(verb) = cmd_service::restart_if_loaded(install, lctl, new, wait)? {
        items.push(Item::new("agent", verb));
    }
    let replacement = service::replace_env_value(
        &install.settings,
        service::BASE_URL,
        &service::base_url(old.port),
        &service::base_url(new.port),
    )?;
    items.push(Item::new(service::BASE_URL, replacement.text()));
    Ok(items)
}

/// Puts the old port back after `cause`: restores `config.toml`, then restarts the agent on the old
/// port. The error says what the rollback left; its hint comes from `cause`, unless the config
/// could not be restored.
fn roll_back_port(
    install: &Install,
    lctl: &dyn Launchctl,
    old: &Config,
    new: &Config,
    wait: Duration,
    cause: anyhow::Error,
) -> anyhow::Error {
    if let Err(e) = old.save_to(&install.config) {
        return hinted(
            format!(
                "{cause:#}; the port change to {} was not rolled back: {e}",
                new.port
            ),
            format!(
                "make {} writable, then run `vicarius self config set port {}`",
                install.config.display(),
                old.port
            ),
        );
    }
    match cmd_service::restart(install, lctl, old, wait) {
        Ok(_) => cause.context(format!(
            "the port change to {} was rolled back: config.toml is back on port {}, and the agent \
             runs there",
            new.port, old.port
        )),
        Err(e) => e.context(format!(
            "the port change to {} was rolled back in config.toml, but the agent did not restart \
             on port {}; the change failed with: {cause:#}",
            new.port, old.port
        )),
    }
}

/// Reads `value` as the number that `key` takes.
fn read<T: FromStr>(value: &str, key: Key) -> Result<T, ConfigError> {
    value.parse().map_err(|_| ConfigError::BadValue {
        value: value.to_owned(),
        expected: key.expected(),
    })
}
