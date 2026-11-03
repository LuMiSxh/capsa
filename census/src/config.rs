// SPDX-License-Identifier: MPL-2.0
//! Owns census's user settings: the limit mode, the monthly budget and how each provider bills,
//! kept in `config.json` in the census config directory. A missing file means plan mode.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Names the file in the config directory.
const FILE: &str = "config.json";

/// Which limits census reports.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    /// The subscription windows, 5 hours and weekly.
    #[default]
    Plan,
    /// The spend of the local calendar month against `monthly_budget_usd`.
    Budget,
}

impl Mode {
    /// Returns the name of the mode as it is written in the file and on the command line.
    pub fn name(self) -> &'static str {
        match self {
            Mode::Plan => "plan",
            Mode::Budget => "budget",
        }
    }
}

/// How a provider bills its requests: as the subscription, or as metered API spend.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Billing {
    Subscription,
    Api,
}

impl Billing {
    /// Returns the name of the billing as it is written in the file and on the command line.
    pub fn name(self) -> &'static str {
        match self {
            Billing::Subscription => "subscription",
            Billing::Api => "api",
        }
    }
}

/// Lists what can go wrong with the config file.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("cannot read or write {}", .0.display())]
    Io(PathBuf, #[source] std::io::Error),
    #[error("{} is not a valid census config", .0.display())]
    Parse(PathBuf, #[source] serde_json::Error),
}

/// The daily thresholds in percent of the daily limit, as they are when the file does not say.
pub const DEFAULT_DAILY_THRESHOLDS: [u32; 3] = [50, 80, 100];

/// The days before the cached price list is fetched again.
pub const DEFAULT_PRICES_REFRESH_DAYS: u64 = 7;

/// The billing the config file uses when it does not name one: the Claude subscription.
fn default_billing() -> BTreeMap<String, Billing> {
    BTreeMap::from([("anthropic".to_owned(), Billing::Subscription)])
}

/// The settings in `config.json`. Missing keys keep their default.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub mode: Mode,
    /// The budget of one calendar month in USD; `None` until it is set.
    pub monthly_budget_usd: Option<f64>,
    /// The daily limit in USD that replaces the one derived from the budget; `None` derives it.
    pub daily_limit_usd: Option<f64>,
    /// The shares of the daily limit in percent that are announced, once per local day each.
    pub daily_thresholds: Vec<u32>,
    /// Days before the cached LiteLLM price list is fetched again; a value below 1 counts as 1.
    pub prices_refresh_days: u64,
    /// How each provider bills its requests. A provider the map does not list is `subscription`
    /// for `anthropic` and `api` for every other one.
    pub billing: BTreeMap<String, Billing>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            mode: Mode::Plan,
            monthly_budget_usd: None,
            daily_limit_usd: None,
            daily_thresholds: DEFAULT_DAILY_THRESHOLDS.to_vec(),
            prices_refresh_days: DEFAULT_PRICES_REFRESH_DAYS,
            billing: default_billing(),
        }
    }
}

impl Config {
    /// Returns the path of the config file. Nothing is created.
    pub fn path() -> PathBuf {
        capsa_core::paths::config_dir("census").join(FILE)
    }

    /// Reads the config, and plan mode when the file is missing or unreadable. For the status line
    /// and the hook, which must not fail.
    pub fn load() -> Config {
        Config::load_from(&Config::path()).unwrap_or_default()
    }

    /// Reads the config at `path`; a missing file gives the default.
    ///
    /// # Errors
    /// Fails when the file exists but cannot be read or is not a census config.
    pub fn load_from(path: &Path) -> Result<Config, ConfigError> {
        match std::fs::read_to_string(path) {
            Ok(text) => Config::parse(&text).map_err(|e| ConfigError::Parse(path.to_path_buf(), e)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Config::default()),
            Err(e) => Err(ConfigError::Io(path.to_path_buf(), e)),
        }
    }

    /// Returns how `provider` is billed: the map's entry, else `Subscription` for `anthropic`
    /// and `Api` for every other provider.
    pub fn billing_of(&self, provider: &str) -> Billing {
        match self.billing.get(provider) {
            Some(b) => *b,
            None if provider == "anthropic" => Billing::Subscription,
            None => Billing::Api,
        }
    }

    /// Parses the JSON text of a config file.
    ///
    /// # Errors
    /// Fails when the text is not a JSON object of known types.
    pub fn parse(text: &str) -> Result<Config, serde_json::Error> {
        serde_json::from_str(text)
    }

    /// Returns the JSON text of the config file.
    pub fn to_json(&self) -> String {
        let mut text = serde_json::to_string_pretty(self).unwrap_or_default();
        text.push('\n');
        text
    }

    /// Writes the config to `path`, creating its directory.
    ///
    /// # Errors
    /// Fails when the directory or the file cannot be written.
    pub fn save_to(&self, path: &Path) -> Result<(), ConfigError> {
        let io = |e| ConfigError::Io(path.to_path_buf(), e);
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(io)?;
        }
        std::fs::write(path, self.to_json()).map_err(io)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_object_is_plan_mode_without_a_budget() {
        assert_eq!(Config::parse("{}").unwrap(), Config::default());
        assert_eq!(Config::default().mode, Mode::Plan);
    }

    #[test]
    fn both_keys_parse() {
        let c = Config::parse(r#"{"mode":"budget","monthly_budget_usd":200}"#).unwrap();
        assert_eq!(c.mode, Mode::Budget);
        assert_eq!(c.monthly_budget_usd, Some(200.0));
    }

    #[test]
    fn an_unknown_mode_is_refused() {
        assert!(Config::parse(r#"{"mode":"free"}"#).is_err());
    }

    #[test]
    fn the_price_list_is_refreshed_every_seven_days_unless_set() {
        assert_eq!(Config::parse("{}").unwrap().prices_refresh_days, 7);
        let c = Config::parse(r#"{"prices_refresh_days":3}"#).unwrap();
        assert_eq!(c.prices_refresh_days, 3);
    }

    #[test]
    fn daily_keys_parse_and_the_thresholds_default_to_fifty_eighty_hundred() {
        let c = Config::parse(r#"{"mode":"budget","daily_limit_usd":8.0}"#).unwrap();
        assert_eq!(c.daily_limit_usd, Some(8.0));
        assert_eq!(c.daily_thresholds, vec![50, 80, 100]);
        let c = Config::parse(r#"{"daily_thresholds":[25,90]}"#).unwrap();
        assert_eq!(c.daily_thresholds, vec![25, 90]);
    }

    #[test]
    fn billing_is_anthropic_subscription_and_every_other_provider_api_unless_set() {
        let c = Config::parse("{}").unwrap();
        assert_eq!(c.billing_of("anthropic"), Billing::Subscription);
        assert_eq!(c.billing_of("openai"), Billing::Api);
        assert_eq!(c.billing, default_billing());
    }

    #[test]
    fn a_billing_map_without_anthropic_keeps_it_a_subscription() {
        let c = Config::parse(r#"{"billing":{"openai":"subscription"}}"#).unwrap();
        assert_eq!(c.billing_of("openai"), Billing::Subscription);
        assert_eq!(c.billing_of("anthropic"), Billing::Subscription);
        let api = Config::parse(r#"{"billing":{"anthropic":"api"}}"#).unwrap();
        assert_eq!(api.billing_of("anthropic"), Billing::Api);
        assert!(Config::parse(r#"{"billing":{"openai":"free"}}"#).is_err());
    }

    #[test]
    fn serializing_round_trips() {
        let c = Config {
            mode: Mode::Budget,
            monthly_budget_usd: Some(150.5),
            daily_limit_usd: Some(8.0),
            daily_thresholds: vec![60, 100],
            prices_refresh_days: 3,
            billing: BTreeMap::from([("openai".to_owned(), Billing::Subscription)]),
        };
        assert_eq!(Config::parse(&c.to_json()).unwrap(), c);
        assert!(c.to_json().contains(r#""mode": "budget""#));
    }

    #[test]
    fn a_missing_file_is_plan_mode_and_saving_creates_the_directory() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join(FILE);
        assert_eq!(Config::load_from(&path).unwrap(), Config::default());
        let c = Config {
            mode: Mode::Budget,
            monthly_budget_usd: Some(10.0),
            ..Config::default()
        };
        c.save_to(&path).unwrap();
        assert_eq!(Config::load_from(&path).unwrap(), c);
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
}
