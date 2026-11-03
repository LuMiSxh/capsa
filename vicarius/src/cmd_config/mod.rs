// SPDX-License-Identifier: MPL-2.0
//! `vicarius self config`: shows the settings, and changes one scalar or the derived agents.

use anyhow::Result;
use capsa_core::exit::Outcome;
use capsa_core::format::Format;
use capsa_core::hint::{Hinted, ResultExt};
use capsa_core::render::{Doc, Render, emit};
use serde::Serialize;
use std::str::FromStr;
use std::time::Duration;
use vicarius::config::{Agent, Config, ConfigError, Effort};

use crate::cmd_service::{self, Item, RESTART_NOTE, WAIT};
use crate::service::{self, Install, Launchctl, Replacement, System};

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
            doc.row(format!("agent {}", agent.from)).text(format!(
                "{}-ds on {} (effort: {})",
                agent.from,
                agent.model,
                agent.effort.as_str()
            ));
        }
        for step in &self.steps {
            doc.row(step.name).text(step.status.as_str());
        }
        if let Some(note) = self.note {
            doc.line(note);
        }
        doc.row("file").dim(self.path.as_str());
    }
}

pub fn run(a: Args, format: Format) -> Result<Outcome> {
    let path = Config::path();
    let mut config = Config::load_from(&path).hinted()?;
    let mut steps = Vec::new();
    if let Some(action) = a.action {
        let old = config.clone();
        let key = match &action {
            Action::Set(set) => Some(set.key),
            Action::Agent(_) => None,
        };
        change(&mut config, action).hinted()?;
        match key.filter(|_| cfg!(target_os = "macos")) {
            Some(key) => {
                let install = Install::current()?;
                steps = apply_steps(&install, &System, key, &old, &config, WAIT)?;
            }
            None => config.save_to(&path).hinted()?,
        }
    }
    let changed = steps
        .iter()
        .any(|item| item.status == Replacement::Replaced.text());
    let body = ConfigOut {
        config: &config,
        path: path.display().to_string(),
        steps,
        note: changed.then_some(RESTART_NOTE),
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
        }),
        Action::Agent(AgentArgs {
            action: AgentAction::Remove { from },
        }) => config.remove_agent(&from),
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
    old: &Config,
    new: &Config,
    wait: Duration,
) -> Result<Vec<Item>> {
    if matches!(key, Key::Port) && old.port != new.port {
        return port_steps(install, lctl, old, new, wait);
    }
    new.save_to(&install.config).hinted()?;
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

/// Saves the new port before the restart, because the agent reads `config.json` at boot. A failure
/// after the save rolls the port back.
fn port_steps(
    install: &Install,
    lctl: &dyn Launchctl,
    old: &Config,
    new: &Config,
    wait: Duration,
) -> Result<Vec<Item>> {
    new.save_to(&install.config).hinted()?;
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

/// Puts the old port back after `cause`: restores `config.json`, then restarts the agent on the old
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
        return Hinted::new(
            format!(
                "{cause:#}; the port change to {} was not rolled back: {e}",
                new.port
            ),
            format!(
                "make {} writable, then run `vicarius self config set port {}`",
                install.config.display(),
                old.port
            ),
        )
        .into();
    }
    match cmd_service::restart(install, lctl, old, wait) {
        Ok(_) => cause.context(format!(
            "the port change to {} was rolled back: config.json is back on port {}, and the agent \
             runs there",
            new.port, old.port
        )),
        Err(e) => e.context(format!(
            "the port change to {} was rolled back in config.json, but the agent did not restart \
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
