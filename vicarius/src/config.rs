// SPDX-License-Identifier: MPL-2.0
//! Owns vicarius's settings, kept in `config.json` in the vicarius config directory. A missing
//! file means the defaults, and a setting that breaks a rule is refused.

use capsa_core::hint::Hint;
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// Names the file in the config directory.
const FILE: &str = "config.json";

/// The smallest context window, in tokens, that the settings accept.
const MIN_CONTEXT_TOKENS: u64 = 1000;

/// The settings in `config.json`. Missing keys keep their default.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// The local port the proxy listens on.
    pub port: u16,
    /// The DeepSeek balance in USD below which the derived agents are withdrawn.
    pub min_balance_usd: f64,
    /// The minutes between two refreshes of the DeepSeek balance.
    pub balance_refresh_minutes: u32,
    /// The context window in tokens that Claude Code assumes for the DeepSeek models.
    pub max_context_tokens: u64,
    /// The agents that get a DeepSeek variant.
    pub agents: Vec<Agent>,
}

/// An agent with a DeepSeek variant named `<from>-ds`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Agent {
    /// The agent file in `~/.claude/agents` that the variant derives from, without `.md`.
    pub from: String,
    /// The DeepSeek model the variant runs on, such as `deepseek-flash`.
    pub model: String,
}

/// Lists what can go wrong with the config file or a change to it.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("cannot read or write {}", .0.display())]
    Io(PathBuf, #[source] std::io::Error),
    #[error("{} is not a valid vicarius config", .0.display())]
    Parse(PathBuf, #[source] serde_json::Error),
    #[error("port must not be 0")]
    PortZero,
    #[error("min_balance_usd must be a finite amount of 0 or more, not {0}")]
    MinBalance(f64),
    #[error("balance_refresh_minutes must be at least 1, not {0}")]
    BalanceRefresh(u32),
    #[error("max_context_tokens must be at least 1000, not {0}")]
    MaxContext(u64),
    #[error("agent name `{0}` may use only letters, digits, `-` and `_`")]
    AgentName(String),
    #[error("agent name `{0}` must not end in `-ds`, since the variant adds it")]
    DerivedName(String),
    #[error("model `{model}` of agent `{from}` must start with `deepseek-`")]
    AgentModel { from: String, model: String },
    #[error("agent `{0}` is configured more than once")]
    DuplicateAgent(String),
    #[error("agent `{0}` is already configured")]
    AgentExists(String),
    #[error("no agent named `{from}` is configured")]
    NoSuchAgent { from: String, known: Vec<String> },
    #[error("`{value}` is not {expected}")]
    BadValue {
        value: String,
        expected: &'static str,
    },
}

impl Hint for ConfigError {
    fn hint(&self) -> Option<Cow<'_, str>> {
        let text = match self {
            Self::Io(..) => return None,
            Self::Parse(path, _) => format!(
                "fix the JSON in {}, or move the file away to use the defaults",
                path.display()
            ),
            Self::PortZero => "use a port from 1 to 65535, such as 3337".to_owned(),
            Self::MinBalance(_) => "use an amount of 0 or more, such as 1.0".to_owned(),
            Self::BalanceRefresh(_) => "use a whole number of minutes, such as 10".to_owned(),
            Self::MaxContext(_) => "use at least 1000 tokens, such as 400000".to_owned(),
            Self::AgentName(_) => {
                "name the agent file without `.md`, such as `senior-worker`".to_owned()
            }
            Self::DerivedName(_) => "name the source agent, without the `-ds` suffix".to_owned(),
            Self::AgentModel { .. } => {
                "use a DeepSeek model id, such as `deepseek-flash`".to_owned()
            }
            Self::DuplicateAgent(from) => format!("keep one entry for `{from}` in config.json"),
            Self::AgentExists(from) => {
                format!("remove it first with `vicarius self config agent remove {from}`")
            }
            Self::NoSuchAgent { known, .. } if known.is_empty() => {
                "add one with `vicarius self config agent add <from> --model <id>`".to_owned()
            }
            Self::NoSuchAgent { known, .. } => format!("configured agents: {}", known.join(", ")),
            Self::BadValue { expected, .. } => format!("pass {expected}"),
        };
        Some(Cow::Owned(text))
    }
}

impl Default for Config {
    fn default() -> Self {
        Config {
            port: 3337,
            min_balance_usd: 1.0,
            balance_refresh_minutes: 10,
            max_context_tokens: 400_000,
            agents: vec![Agent {
                from: "senior-worker".to_owned(),
                model: "deepseek-flash".to_owned(),
            }],
        }
    }
}

impl Config {
    /// Returns the path of the config file. Nothing is created.
    pub fn path() -> PathBuf {
        capsa_core::paths::config_dir("vicarius").join(FILE)
    }

    /// Reads the config at `path`, which is the default config when the file is missing.
    ///
    /// # Errors
    /// Fails when the file cannot be read, is not a vicarius config or breaks a rule.
    pub fn load_from(path: &Path) -> Result<Config, ConfigError> {
        let config = match std::fs::read_to_string(path) {
            Ok(text) => {
                Config::parse(&text).map_err(|e| ConfigError::Parse(path.to_path_buf(), e))?
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Config::default(),
            Err(e) => return Err(ConfigError::Io(path.to_path_buf(), e)),
        };
        config.validate()?;
        Ok(config)
    }

    /// Parses the JSON text of a config file. Missing keys keep their defaults.
    ///
    /// # Errors
    /// Fails when the text is not a JSON object of the known types.
    pub fn parse(text: &str) -> Result<Config, serde_json::Error> {
        serde_json::from_str(text)
    }

    /// Checks every setting against its rule and returns the first one that breaks.
    ///
    /// # Errors
    /// Fails with the error of the first setting that breaks its rule.
    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.port == 0 {
            return Err(ConfigError::PortZero);
        }
        if !(self.min_balance_usd.is_finite() && self.min_balance_usd >= 0.0) {
            return Err(ConfigError::MinBalance(self.min_balance_usd));
        }
        if self.balance_refresh_minutes == 0 {
            return Err(ConfigError::BalanceRefresh(self.balance_refresh_minutes));
        }
        if self.max_context_tokens < MIN_CONTEXT_TOKENS {
            return Err(ConfigError::MaxContext(self.max_context_tokens));
        }
        let mut names = HashSet::new();
        for agent in &self.agents {
            agent.validate()?;
            if !names.insert(agent.from.as_str()) {
                return Err(ConfigError::DuplicateAgent(agent.from.clone()));
            }
        }
        Ok(())
    }

    /// Adds `agent` once its name and model pass their rules.
    ///
    /// # Errors
    /// Fails when the agent breaks a rule or its name is already configured.
    pub fn add_agent(&mut self, agent: Agent) -> Result<(), ConfigError> {
        agent.validate()?;
        if self.agents.iter().any(|a| a.from == agent.from) {
            return Err(ConfigError::AgentExists(agent.from));
        }
        self.agents.push(agent);
        Ok(())
    }

    /// Removes the agent named `from`.
    ///
    /// # Errors
    /// Fails when no agent of that name is configured.
    pub fn remove_agent(&mut self, from: &str) -> Result<(), ConfigError> {
        let Some(index) = self.agents.iter().position(|a| a.from == from) else {
            return Err(ConfigError::NoSuchAgent {
                from: from.to_owned(),
                known: self.agents.iter().map(|a| a.from.clone()).collect(),
            });
        };
        self.agents.remove(index);
        Ok(())
    }

    /// Writes the config to `path`, creating its directory. The text goes to a temporary file in
    /// the same directory first and is renamed over the file, so a reader sees the old config or
    /// the new one, never a part of either.
    ///
    /// # Errors
    /// Fails when the directory or the file cannot be written or renamed.
    pub fn save_to(&self, path: &Path) -> Result<(), ConfigError> {
        let io = |e| ConfigError::Io(path.to_path_buf(), e);
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(io)?;
        }
        let temp = path.with_extension("json.tmp");
        let written =
            std::fs::write(&temp, self.to_json()).and_then(|()| std::fs::rename(&temp, path));
        if written.is_err() {
            let _ = std::fs::remove_file(&temp);
        }
        written.map_err(io)
    }

    /// Returns the JSON text of the config file.
    #[expect(
        clippy::expect_used,
        reason = "a config of numbers, strings and a list always serializes"
    )]
    fn to_json(&self) -> String {
        let mut text = serde_json::to_string_pretty(self).expect("config serializes");
        text.push('\n');
        text
    }
}

impl Agent {
    /// Checks the name and the model against their rules.
    fn validate(&self) -> Result<(), ConfigError> {
        let plain = |c: char| c.is_ascii_alphanumeric() || c == '_' || c == '-';
        if self.from.is_empty() || !self.from.chars().all(plain) {
            return Err(ConfigError::AgentName(self.from.clone()));
        }
        if self.from.ends_with("-ds") {
            return Err(ConfigError::DerivedName(self.from.clone()));
        }
        if !self.model.starts_with("deepseek-") {
            return Err(ConfigError::AgentModel {
                from: self.from.clone(),
                model: self.model.clone(),
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with(change: impl FnOnce(&mut Config)) -> Result<(), ConfigError> {
        let mut config = Config::default();
        change(&mut config);
        config.validate()
    }

    fn agent(from: &str, model: &str) -> Agent {
        Agent {
            from: from.to_owned(),
            model: model.to_owned(),
        }
    }

    #[test]
    fn a_missing_file_gives_the_defaults_and_creates_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join(FILE);
        assert_eq!(Config::load_from(&path).unwrap(), Config::default());
        assert!(!path.exists());
    }

    #[test]
    fn the_defaults_are_the_documented_settings() {
        let c = Config::default();
        assert_eq!(c.port, 3337);
        assert_eq!(c.min_balance_usd, 1.0);
        assert_eq!(c.balance_refresh_minutes, 10);
        assert_eq!(c.max_context_tokens, 400_000);
        assert_eq!(c.agents, vec![agent("senior-worker", "deepseek-flash")]);
    }

    #[test]
    fn missing_keys_keep_their_defaults() {
        let c = Config::parse(r#"{"port":4000,"agents":[]}"#).unwrap();
        assert_eq!(
            c,
            Config {
                port: 4000,
                agents: Vec::new(),
                ..Config::default()
            }
        );
    }

    #[test]
    fn saving_and_loading_round_trips_and_leaves_no_temporary_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE);
        let c = Config {
            port: 4100,
            min_balance_usd: 2.5,
            balance_refresh_minutes: 5,
            max_context_tokens: 200_000,
            agents: vec![agent("consultant", "deepseek-v4-pro")],
        };
        c.save_to(&path).unwrap();
        assert_eq!(Config::load_from(&path).unwrap(), c);
        assert!(!dir.path().join("config.json.tmp").exists());
    }

    #[test]
    fn an_invalid_file_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE);
        std::fs::write(&path, "not json").unwrap();
        assert!(matches!(
            Config::load_from(&path),
            Err(ConfigError::Parse(..))
        ));
    }

    #[test]
    fn a_file_that_breaks_a_rule_is_refused_on_load() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE);
        std::fs::write(&path, r#"{"port":0}"#).unwrap();
        assert!(matches!(
            Config::load_from(&path),
            Err(ConfigError::PortZero)
        ));
    }

    #[test]
    fn port_zero_is_refused() {
        assert!(matches!(with(|c| c.port = 0), Err(ConfigError::PortZero)));
    }

    #[test]
    fn min_balance_must_be_a_finite_amount_of_zero_or_more() {
        assert!(with(|c| c.min_balance_usd = 0.0).is_ok());
        for bad in [-0.5, f64::NAN, f64::INFINITY] {
            let result = with(|c| c.min_balance_usd = bad);
            assert!(matches!(result, Err(ConfigError::MinBalance(_))), "{bad}");
        }
    }

    #[test]
    fn balance_refresh_must_be_at_least_one_minute() {
        let zero = with(|c| c.balance_refresh_minutes = 0);
        assert!(matches!(zero, Err(ConfigError::BalanceRefresh(0))));
        assert!(with(|c| c.balance_refresh_minutes = 1).is_ok());
    }

    #[test]
    fn max_context_tokens_must_be_at_least_one_thousand() {
        let small = with(|c| c.max_context_tokens = 999);
        assert!(matches!(small, Err(ConfigError::MaxContext(999))));
        assert!(with(|c| c.max_context_tokens = 1000).is_ok());
    }

    #[test]
    fn agent_names_use_only_letters_digits_dash_and_underscore() {
        for bad in ["", "a.b", "../x", "a/b", "a b"] {
            let result = with(|c| c.agents = vec![agent(bad, "deepseek-flash")]);
            assert!(matches!(result, Err(ConfigError::AgentName(_))), "{bad}");
        }
        let good = vec![agent("senior_worker-2", "deepseek-flash")];
        assert!(with(|c| c.agents = good).is_ok());
    }

    #[test]
    fn agent_names_may_not_end_in_ds() {
        let names = vec![agent("senior-worker-ds", "deepseek-flash")];
        let result = with(|c| c.agents = names);
        assert!(matches!(result, Err(ConfigError::DerivedName(_))));
    }

    #[test]
    fn agent_models_start_with_deepseek() {
        let gpt = vec![agent("consultant", "gpt-5")];
        assert!(matches!(
            with(|c| c.agents = gpt),
            Err(ConfigError::AgentModel { .. })
        ));
        let v4 = vec![agent("consultant", "deepseek-v4-pro")];
        assert!(with(|c| c.agents = v4).is_ok());
    }

    #[test]
    fn an_agent_name_configured_twice_is_refused() {
        let twice = vec![agent("a", "deepseek-flash"), agent("a", "deepseek-v4-pro")];
        let result = with(|c| c.agents = twice);
        assert!(matches!(result, Err(ConfigError::DuplicateAgent(_))));
    }

    #[test]
    fn adding_an_agent_checks_it_and_refuses_a_name_in_use() {
        let mut c = Config::default();
        let again = c.add_agent(agent("senior-worker", "deepseek-v4-pro"));
        assert!(matches!(again, Err(ConfigError::AgentExists(_))));
        let bad = c.add_agent(agent("x.y", "deepseek-flash"));
        assert!(matches!(bad, Err(ConfigError::AgentName(_))));
        assert!(c.add_agent(agent("consultant", "deepseek-v4-pro")).is_ok());
        assert_eq!(c.agents.len(), 2);
    }

    #[test]
    fn removing_an_unknown_agent_names_the_configured_ones() {
        let mut c = Config::default();
        let err = c.remove_agent("nobody").unwrap_err();
        assert!(matches!(err, ConfigError::NoSuchAgent { .. }));
        assert_eq!(
            err.hint().as_deref(),
            Some("configured agents: senior-worker")
        );
    }
}
