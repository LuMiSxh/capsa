// SPDX-License-Identifier: MPL-2.0
//! Owns census's user settings: the limit mode, the monthly budget and how each provider bills,
//! kept in `config.toml` in the census config directory. A missing file means plan mode.

use capsa_core::config::ConfigFile;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Names the file in the config directory.
const FILE: &str = "config.toml";

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

/// The daily thresholds in percent of the daily limit, as they are when the file does not say.
pub const DEFAULT_DAILY_THRESHOLDS: [u32; 3] = [50, 80, 100];

/// The days before the cached price list is fetched again.
pub const DEFAULT_PRICES_REFRESH_DAYS: u64 = 7;

/// The billing the config file uses when it does not name one: the Claude subscription.
fn default_billing() -> BTreeMap<String, Billing> {
    BTreeMap::from([("anthropic".to_owned(), Billing::Subscription)])
}

/// The settings in `config.toml`. Missing keys keep their default; an unknown key is an error.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
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
    /// Returns the config file in census's config directory. Nothing is read or created.
    pub fn file() -> ConfigFile {
        ConfigFile::at(capsa_core::paths::config_dir("census").join(FILE))
    }

    /// Reads the config, and plan mode when the file is missing or unreadable. For the status line
    /// and the hook, which must not fail.
    pub fn load() -> Config {
        Config::file().load().unwrap_or_default()
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use capsa_core::config::ConfigError;

    /// Loads `text` as the config file, as the commands do.
    fn load(text: &str) -> Result<Config, ConfigError> {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE);
        std::fs::write(&path, text).unwrap();
        ConfigFile::at(path).load()
    }

    #[test]
    fn an_empty_file_is_plan_mode_without_a_budget() {
        assert_eq!(load("").unwrap(), Config::default());
        assert_eq!(Config::default().mode, Mode::Plan);
    }

    #[test]
    fn both_keys_parse() {
        let c = load("mode = \"budget\"\nmonthly_budget_usd = 200.0\n").unwrap();
        assert_eq!(c.mode, Mode::Budget);
        assert_eq!(c.monthly_budget_usd, Some(200.0));
    }

    #[test]
    fn an_unknown_mode_is_refused() {
        assert!(load("mode = \"free\"").is_err());
    }

    #[test]
    fn an_unknown_key_is_refused() {
        assert!(load("modee = \"plan\"").is_err());
    }

    #[test]
    fn the_price_list_is_refreshed_every_seven_days_unless_set() {
        assert_eq!(load("").unwrap().prices_refresh_days, 7);
        let c = load("prices_refresh_days = 3\n").unwrap();
        assert_eq!(c.prices_refresh_days, 3);
    }

    #[test]
    fn daily_keys_parse_and_the_thresholds_default_to_fifty_eighty_hundred() {
        let c = load("mode = \"budget\"\ndaily_limit_usd = 8.0\n").unwrap();
        assert_eq!(c.daily_limit_usd, Some(8.0));
        assert_eq!(c.daily_thresholds, vec![50, 80, 100]);
        let c = load("daily_thresholds = [25, 90]\n").unwrap();
        assert_eq!(c.daily_thresholds, vec![25, 90]);
    }

    #[test]
    fn billing_is_anthropic_subscription_and_every_other_provider_api_unless_set() {
        let c = load("").unwrap();
        assert_eq!(c.billing_of("anthropic"), Billing::Subscription);
        assert_eq!(c.billing_of("openai"), Billing::Api);
        assert_eq!(c.billing, default_billing());
    }

    #[test]
    fn a_billing_table_without_anthropic_keeps_it_a_subscription() {
        let c = load("[billing]\nopenai = \"subscription\"\n").unwrap();
        assert_eq!(c.billing_of("openai"), Billing::Subscription);
        assert_eq!(c.billing_of("anthropic"), Billing::Subscription);
        let api = load("[billing]\nanthropic = \"api\"\n").unwrap();
        assert_eq!(api.billing_of("anthropic"), Billing::Api);
        assert!(load("[billing]\nopenai = \"free\"\n").is_err());
    }

    #[test]
    fn saving_round_trips_and_writes_toml() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE);
        let c = Config {
            mode: Mode::Budget,
            monthly_budget_usd: Some(150.5),
            daily_limit_usd: Some(8.0),
            daily_thresholds: vec![60, 100],
            prices_refresh_days: 3,
            billing: BTreeMap::from([("openai".to_owned(), Billing::Subscription)]),
        };
        let file = ConfigFile::at(&path);
        file.save(&c).unwrap();
        assert_eq!(file.load::<Config>().unwrap(), c);
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("mode = \"budget\""), "{text}");
    }

    #[test]
    fn a_missing_file_is_plan_mode_and_saving_creates_the_directory() {
        let dir = tempfile::tempdir().unwrap();
        let file = ConfigFile::at(dir.path().join("nested").join(FILE));
        assert_eq!(file.load::<Config>().unwrap(), Config::default());
        let c = Config {
            mode: Mode::Budget,
            monthly_budget_usd: Some(10.0),
            ..Config::default()
        };
        file.save(&c).unwrap();
        assert_eq!(file.load::<Config>().unwrap(), c);
    }

    #[test]
    fn an_invalid_file_is_an_error() {
        assert!(matches!(load("not toml"), Err(ConfigError::Parse { .. })));
    }
}
