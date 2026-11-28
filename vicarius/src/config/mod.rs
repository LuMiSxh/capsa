// SPDX-License-Identifier: MPL-2.0
//! Owns vicarius's settings, kept in `config.toml` in the vicarius config directory. A missing
//! file means the defaults, and a setting that breaks a rule is refused.

use capsa_core::config::ConfigFile;
use capsa_core::hint::Hint;
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

#[cfg(test)]
mod tests;

/// Names the file in the config directory.
const FILE: &str = "config.toml";

/// The smallest context window, in tokens, that the settings accept.
const MIN_CONTEXT_TOKENS: u64 = 1000;

/// The provider that the legacy top-level balance settings migrate to.
const LEGACY_PROVIDER: &str = "deepseek";

/// The DeepSeek model ids vicarius knows. An id outside the list is saved with a warning rather
/// than refused, because DeepSeek adds models without a vicarius release.
pub const KNOWN_MODELS: &[&str] = &["deepseek-flash", "deepseek-v4-pro"];

/// Returns whether `model` is one of `KNOWN_MODELS`.
pub fn known_model(model: &str) -> bool {
    KNOWN_MODELS.contains(&model)
}

/// The settings in `config.toml`. Missing keys keep their default.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// The local port the proxy listens on.
    #[serde(default = "default_port")]
    pub port: u16,
    /// The context window in tokens that Claude Code assumes for the routed models.
    #[serde(default = "default_max_context")]
    pub max_context_tokens: u64,
    /// The providers the proxy can route to, by name.
    #[serde(default)]
    pub providers: BTreeMap<String, Provider>,
    /// The agents that get a derived variant.
    #[serde(default = "default_agents")]
    pub agents: Vec<Agent>,
    /// The legacy minimum balance, read only when no `providers` table exists and migrated to
    /// `providers.deepseek.balance` on save.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_balance_usd: Option<f64>,
    /// The legacy refresh period, read only when no `providers` table exists and migrated to
    /// `providers.deepseek.balance` on save.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub balance_refresh_minutes: Option<u32>,
}

/// One upstream provider that the proxy routes matching model ids to.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Provider {
    /// The model-id prefixes this provider takes.
    pub prefixes: Vec<String>,
    /// The provider's Anthropic-compatible base URL, without a trailing slash.
    pub base_url: String,
    /// The environment variable that holds this provider's key.
    pub key_env: String,
    /// How the key is sent upstream.
    pub auth: Auth,
    /// The model ids that this provider serves.
    #[serde(default)]
    pub models: Vec<String>,
    /// The request rewrites this provider needs.
    #[serde(default)]
    pub compat: Compat,
    /// The balance endpoint and gate, when the provider has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub balance: Option<Balance>,
}

impl Provider {
    /// Returns the DeepSeek provider that the defaults and the legacy settings use.
    fn deepseek() -> Self {
        Provider {
            prefixes: vec!["deepseek-".to_owned()],
            base_url: "https://api.deepseek.com/anthropic".to_owned(),
            key_env: "DEEPSEEK_API_KEY".to_owned(),
            auth: Auth::XApiKey,
            models: vec!["deepseek-flash".to_owned(), "deepseek-v4-pro".to_owned()],
            compat: Compat {
                inline_tools: true,
                strip_cache_control: true,
                strip_thinking: true,
            },
            balance: Some(Balance {
                url: "https://api.deepseek.com/user/balance".to_owned(),
                min_usd: 1.0,
                refresh_minutes: 10,
            }),
        }
    }

    /// Checks the provider's table entry against its rules.
    fn validate(&self, name: &str, prefixes: &mut HashSet<String>) -> Result<(), ConfigError> {
        let provider = name.to_owned();
        if name.is_empty() {
            return Err(ConfigError::ProviderName);
        }
        if self.prefixes.is_empty() {
            return Err(ConfigError::NoPrefixes { provider });
        }
        if self.key_env.trim().is_empty() {
            return Err(ConfigError::KeyEnv { provider });
        }
        for prefix in &self.prefixes {
            if prefix.is_empty() {
                return Err(ConfigError::EmptyPrefix { provider });
            }
            if !prefixes.insert(prefix.clone()) {
                return Err(ConfigError::DuplicatePrefix(prefix.clone()));
            }
        }
        for model in &self.models {
            if !self.prefixes.iter().any(|prefix| model.starts_with(prefix)) {
                return Err(ConfigError::ModelOutsidePrefix {
                    provider,
                    model: model.clone(),
                });
            }
        }
        if let Some(balance) = &self.balance {
            balance.validate(name)?;
        }
        Ok(())
    }
}

/// How a provider expects its key in the request.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Auth {
    /// The key travels in `x-api-key`.
    #[default]
    XApiKey,
    /// The key travels in `Authorization: Bearer`.
    Bearer,
}

/// The request rewrites one provider needs. A new quirk adds a flag here, never a provider-name
/// branch in the handler.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Compat {
    /// Moves inline `tool_definition` blocks into the request's `tools`.
    #[serde(default)]
    pub inline_tools: bool,
    /// Removes every `cache_control` field from the request body.
    #[serde(default)]
    pub strip_cache_control: bool,
    /// Removes `thinking` fields and thinking content blocks from the request body.
    #[serde(default)]
    pub strip_thinking: bool,
}

/// A provider's balance endpoint and the gate it drives.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Balance {
    /// The full URL of the balance endpoint.
    pub url: String,
    /// The balance in USD below which the gate closes.
    pub min_usd: f64,
    /// The minutes between two reads of the balance.
    pub refresh_minutes: u32,
}

impl Balance {
    /// Checks the amount and the refresh period against their rules.
    fn validate(&self, provider: &str) -> Result<(), ConfigError> {
        if !(self.min_usd.is_finite() && self.min_usd >= 0.0) {
            return Err(ConfigError::MinBalance(self.min_usd));
        }
        if self.refresh_minutes == 0 {
            return Err(ConfigError::BalanceRefresh(self.refresh_minutes));
        }
        if self.url.trim().is_empty() {
            return Err(ConfigError::BalanceUrl {
                provider: provider.to_owned(),
            });
        }
        Ok(())
    }
}

/// The effort applied to a derived agent.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum Effort {
    /// Keep the source agent's effort unchanged.
    #[default]
    Keep,
    /// Use low effort.
    Low,
    /// Use high effort.
    High,
    /// Use maximum effort.
    Max,
}

impl Effort {
    /// Returns the config and frontmatter spelling of this effort.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Keep => "keep",
            Self::Low => "low",
            Self::High => "high",
            Self::Max => "max",
        }
    }
}

/// An agent with a variant named `<from>-ds`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Agent {
    /// The agent file in `~/.claude/agents` that the variant derives from, without `.md`.
    pub from: String,
    /// The routed model the variant runs on, such as `deepseek-flash`.
    pub model: String,
    /// The effort override, or `keep` to preserve the source agent's setting.
    #[serde(default)]
    pub effort: Effort,
    /// Whether the derived file is withdrawn while the entry stays configured.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub suspended: bool,
}

/// Lists what can go wrong with the config file or a change to it.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error(transparent)]
    File(#[from] capsa_core::config::ConfigError),
    #[error("port must not be 0")]
    PortZero,
    #[error("min_usd must be a finite amount of 0 or more, not {0}")]
    MinBalance(f64),
    #[error("refresh_minutes must be at least 1, not {0}")]
    BalanceRefresh(u32),
    #[error("the balance url of provider `{provider}` must not be empty")]
    BalanceUrl { provider: String },
    #[error("max_context_tokens must be at least 1000, not {0}")]
    MaxContext(u64),
    #[error("a provider name must not be empty")]
    ProviderName,
    #[error("provider `{provider}` has no prefixes")]
    NoPrefixes { provider: String },
    #[error("provider `{provider}` has an empty prefix")]
    EmptyPrefix { provider: String },
    #[error("prefix `{0}` is configured more than once")]
    DuplicatePrefix(String),
    #[error("provider `{provider}` needs a key_env")]
    KeyEnv { provider: String },
    #[error("model `{model}` of provider `{provider}` matches none of its prefixes")]
    ModelOutsidePrefix { provider: String, model: String },
    #[error(
        "min_balance_usd and balance_refresh_minutes cannot be used beside a `providers` table"
    )]
    LegacyBalance,
    #[error("provider `{provider}` has no balance block")]
    NoBalance { provider: String },
    #[error("agent name `{0}` may use only letters, digits, `-` and `_`")]
    AgentName(String),
    #[error("agent name `{0}` must not end in `-ds`, since the variant adds it")]
    DerivedName(String),
    #[error("model `{model}` of agent `{from}` must start with a provider prefix")]
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
            Self::File(e) => return e.hint(),
            Self::PortZero => "use a port from 1 to 65535, such as 3337".to_owned(),
            Self::MinBalance(_) => "use an amount of 0 or more, such as 1.0".to_owned(),
            Self::BalanceRefresh(_) => "use a whole number of minutes, such as 10".to_owned(),
            Self::BalanceUrl { .. } => "use the balance endpoint, such as https://api.deepseek.com/user/balance".to_owned(),
            Self::MaxContext(_) => "use at least 1000 tokens, such as 400000".to_owned(),
            Self::ProviderName => "name the provider in its `[providers.<name>]` table".to_owned(),
            Self::NoPrefixes { .. } => "list the model-id prefixes the provider takes, such as [\"deepseek-\"]".to_owned(),
            Self::EmptyPrefix { .. } => "remove the empty prefix or give it the text the model ids start with".to_owned(),
            Self::DuplicatePrefix(_) => "give each provider prefixes no other provider uses".to_owned(),
            Self::KeyEnv { .. } => "name the environment variable that holds the provider's key".to_owned(),
            Self::ModelOutsidePrefix { .. } => "add a prefix that the model starts with, or remove the model".to_owned(),
            Self::LegacyBalance => "move the values to `providers.deepseek.balance.min_usd` and `providers.deepseek.balance.refresh_minutes`".to_owned(),
            Self::NoBalance { .. } => "add a `[providers.deepseek.balance]` table with `url`, `min_usd` and `refresh_minutes`".to_owned(),
            Self::AgentName(_) => "name the agent file without `.md`, such as `senior-worker`".to_owned(),
            Self::DerivedName(_) => "name the source agent, without the `-ds` suffix".to_owned(),
            Self::AgentModel { .. } => "use a model id that starts with a provider prefix, such as `deepseek-flash`".to_owned(),
            Self::DuplicateAgent(from) => format!("keep one entry for `{from}` in config.toml"),
            Self::AgentExists(from) => format!("remove it first with `vicarius self config agent remove {from}`"),
            Self::NoSuchAgent { known, .. } if known.is_empty() => "add one with `vicarius self config agent add <from> --model <id>`".to_owned(),
            Self::NoSuchAgent { known, .. } => format!("configured agents: {}", known.join(", ")),
            Self::BadValue { expected, .. } => format!("pass {expected}"),
        };
        Some(Cow::Owned(text))
    }
}

fn default_port() -> u16 {
    3337
}

fn default_max_context() -> u64 {
    400_000
}

fn default_agents() -> Vec<Agent> {
    vec![Agent {
        from: "senior-worker".to_owned(),
        model: "deepseek-flash".to_owned(),
        effort: Effort::Keep,
        suspended: false,
    }]
}

impl Default for Config {
    fn default() -> Self {
        let mut providers = BTreeMap::new();
        providers.insert(LEGACY_PROVIDER.to_owned(), Provider::deepseek());
        Config {
            port: default_port(),
            max_context_tokens: default_max_context(),
            providers,
            agents: default_agents(),
            min_balance_usd: None,
            balance_refresh_minutes: None,
        }
    }
}

impl Config {
    /// Returns the path of the config file. Nothing is created.
    pub fn path() -> PathBuf {
        capsa_core::paths::config_dir("vicarius").join(FILE)
    }

    /// Returns the config file as `capsa_core::config` reads and writes it.
    pub fn file() -> ConfigFile {
        ConfigFile::at(Self::path())
    }

    /// Reads the config at `path`, which is the default config when the file is missing.
    ///
    /// # Errors
    /// Fails when the file cannot be read, is not a vicarius config or breaks a rule.
    pub fn load_from(path: &Path) -> Result<Config, ConfigError> {
        let mut config: Config = ConfigFile::at(path).load()?;
        config.normalize()?;
        config.validate()?;
        Ok(config)
    }

    /// Returns the balance block of `name`, when the provider has one.
    pub fn provider_balance(&self, name: &str) -> Option<&Balance> {
        self.providers.get(name)?.balance.as_ref()
    }

    /// Returns the mutable balance block of `name`, when the provider has one.
    pub fn provider_balance_mut(&mut self, name: &str) -> Option<&mut Balance> {
        self.providers.get_mut(name)?.balance.as_mut()
    }

    /// Moves the legacy top-level balance settings into the DeepSeek balance block.
    fn normalize(&mut self) -> Result<(), ConfigError> {
        let min = self.min_balance_usd.take();
        let refresh = self.balance_refresh_minutes.take();
        if self.providers.is_empty() {
            let mut provider = Provider::deepseek();
            if let Some(balance) = &mut provider.balance {
                balance.min_usd = min.unwrap_or(balance.min_usd);
                balance.refresh_minutes = refresh.unwrap_or(balance.refresh_minutes);
            }
            self.providers.insert(LEGACY_PROVIDER.to_owned(), provider);
        } else if min.is_some() || refresh.is_some() {
            return Err(ConfigError::LegacyBalance);
        }
        Ok(())
    }

    /// Checks every setting against its rule and returns the first one that breaks.
    ///
    /// # Errors
    /// Fails with the error of the first setting that breaks its rule.
    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.port == 0 {
            return Err(ConfigError::PortZero);
        }
        if self.max_context_tokens < MIN_CONTEXT_TOKENS {
            return Err(ConfigError::MaxContext(self.max_context_tokens));
        }
        let mut prefixes = HashSet::new();
        for (name, provider) in &self.providers {
            provider.validate(name, &mut prefixes)?;
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

    /// Suspends or activates the agent named `from`, keeping the rest of its entry.
    ///
    /// # Errors
    /// Fails when no agent of that name is configured.
    pub fn set_suspended(&mut self, from: &str, suspended: bool) -> Result<(), ConfigError> {
        let Some(agent) = self.agents.iter_mut().find(|a| a.from == from) else {
            return Err(ConfigError::NoSuchAgent {
                from: from.to_owned(),
                known: self.agents.iter().map(|a| a.from.clone()).collect(),
            });
        };
        agent.suspended = suspended;
        Ok(())
    }

    /// Suspends or activates every configured agent.
    pub fn set_all_suspended(&mut self, suspended: bool) {
        for agent in &mut self.agents {
            agent.suspended = suspended;
        }
    }

    /// Writes the config to `path`, creating its directory, through a temporary file in the same
    /// directory that is renamed over the file. Legacy balance settings are written as a provider
    /// balance block.
    ///
    /// # Errors
    /// Fails when the config cannot be encoded, or when the directory or the file cannot be
    /// written.
    pub fn save_to(&self, path: &Path) -> Result<(), ConfigError> {
        let mut config = self.clone();
        config.normalize()?;
        ConfigFile::at(path).save(&config)?;
        Ok(())
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
