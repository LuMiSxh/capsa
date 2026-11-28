// SPDX-License-Identifier: MPL-2.0

use super::*;

fn agent(from: &str, model: &str) -> Agent {
    Agent {
        from: from.to_owned(),
        model: model.to_owned(),
        effort: Effort::Keep,
        suspended: false,
    }
}

fn provider(prefixes: &[&str], models: &[&str]) -> Provider {
    Provider {
        prefixes: prefixes.iter().map(|p| (*p).to_owned()).collect(),
        base_url: "https://example.test/anthropic".to_owned(),
        key_env: "EXAMPLE_API_KEY".to_owned(),
        auth: Auth::Bearer,
        models: models.iter().map(|m| (*m).to_owned()).collect(),
        compat: Compat::default(),
        balance: None,
    }
}

fn load(text: &str) -> Result<Config, ConfigError> {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(FILE);
    std::fs::write(&path, text).unwrap();
    Config::load_from(&path)
}

fn save_and_read(config: &Config) -> String {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(FILE);
    config.save_to(&path).unwrap();
    std::fs::read_to_string(&path).unwrap()
}

type Change = fn(&mut Config);
type Rejection = (&'static str, Change, fn(&ConfigError) -> bool);
type Attempt = (&'static str, fn(&mut Config) -> Result<(), ConfigError>);

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
    assert_eq!(c.max_context_tokens, 400_000);
    assert_eq!(c.agents, vec![agent("senior-worker", "deepseek-flash")]);
    assert_eq!(c.providers.len(), 1);
    let deepseek = &c.providers["deepseek"];
    assert_eq!(deepseek.prefixes, ["deepseek-"]);
    assert_eq!(deepseek.base_url, "https://api.deepseek.com/anthropic");
    assert_eq!(deepseek.key_env, "DEEPSEEK_API_KEY");
    assert_eq!(deepseek.auth, Auth::XApiKey);
    assert_eq!(deepseek.models, ["deepseek-flash", "deepseek-v4-pro"]);
    assert_eq!(
        deepseek.compat,
        Compat {
            inline_tools: true,
            strip_cache_control: true,
            strip_thinking: true,
        }
    );
    assert_eq!(
        deepseek.balance,
        Some(Balance {
            url: "https://api.deepseek.com/user/balance".to_owned(),
            min_usd: 1.0,
            refresh_minutes: 10,
        })
    );
}

#[test]
fn missing_keys_keep_their_defaults() {
    let c = load("port = 4000\nagents = []\n").unwrap();
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
fn an_agent_config_defaults_effort_to_keep_and_is_active() {
    let config = load("[[agents]]\nfrom = \"worker\"\nmodel = \"deepseek-flash\"\n").unwrap();
    assert_eq!(config.agents[0].effort, Effort::Keep);
    assert!(!config.agents[0].suspended);
}

#[test]
fn a_suspended_agent_round_trips_and_an_active_one_leaves_the_key_out() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(FILE);
    let mut c = Config::default();
    c.save_to(&path).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(!text.contains("suspended"), "{text}");
    c.agents[0].suspended = true;
    c.save_to(&path).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("suspended = true"), "{text}");
    assert_eq!(Config::load_from(&path).unwrap(), c);
}

#[test]
fn saving_and_loading_round_trips_and_leaves_no_temporary_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(FILE);
    let mut c = Config {
        port: 4100,
        max_context_tokens: 200_000,
        agents: vec![agent("consultant", "deepseek-v4-pro")],
        ..Config::default()
    };
    let balance = c.provider_balance_mut("deepseek").unwrap();
    balance.min_usd = 2.5;
    balance.refresh_minutes = 5;
    c.save_to(&path).unwrap();
    assert_eq!(Config::load_from(&path).unwrap(), c);
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
}

#[test]
fn an_invalid_file_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(FILE);
    std::fs::write(&path, "port = ").unwrap();
    assert!(matches!(
        Config::load_from(&path),
        Err(ConfigError::File(
            capsa_core::config::ConfigError::Parse { .. }
        ))
    ));
}

#[test]
fn a_file_that_breaks_a_rule_is_refused_on_load() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(FILE);
    std::fs::write(&path, "port = 0\n").unwrap();
    assert!(matches!(
        Config::load_from(&path),
        Err(ConfigError::PortZero)
    ));
}

#[test]
fn a_provider_without_a_compat_block_uses_no_rewrites() {
    let c = load(
        "[providers.moonshot]\n\
         prefixes = [\"kimi-\", \"moonshot-\"]\n\
         base_url = \"https://api.moonshot.ai/anthropic\"\n\
         key_env = \"MOONSHOT_API_KEY\"\n\
         auth = \"bearer\"\n\
         models = [\"kimi-k2\"]\n",
    )
    .unwrap();
    let moonshot = &c.providers["moonshot"];
    assert_eq!(moonshot.auth, Auth::Bearer);
    assert_eq!(moonshot.compat, Compat::default());
    assert_eq!(moonshot.balance, None);
}

#[test]
fn the_compat_block_loads_each_flag() {
    let c = load(
        "[providers.moonshot]\n\
         prefixes = [\"kimi-\"]\n\
         base_url = \"https://api.moonshot.ai/anthropic\"\n\
         key_env = \"MOONSHOT_API_KEY\"\n\
         auth = \"bearer\"\n\
         models = [\"kimi-k2\"]\n\
         [providers.moonshot.compat]\n\
         inline_tools = false\n\
         strip_cache_control = true\n\
         strip_thinking = false\n",
    )
    .unwrap();
    assert_eq!(
        c.providers["moonshot"].compat,
        Compat {
            inline_tools: false,
            strip_cache_control: true,
            strip_thinking: false,
        }
    );
}

#[test]
fn legacy_balance_settings_load_into_the_deepseek_provider_and_migrate_on_save() {
    let c = load("min_balance_usd = 2.5\nbalance_refresh_minutes = 5\n").unwrap();
    let balance = c.provider_balance("deepseek").unwrap();
    assert_eq!((balance.min_usd, balance.refresh_minutes), (2.5, 5));
    let text = save_and_read(&c);
    assert!(text.contains("[providers.deepseek.balance]"), "{text}");
    assert!(text.contains("min_usd = 2.5"), "{text}");
    assert!(text.contains("refresh_minutes = 5"), "{text}");
    assert!(!text.contains("min_balance_usd"), "{text}");
    assert!(!text.contains("balance_refresh_minutes"), "{text}");
}

#[test]
fn legacy_balance_settings_beside_a_provider_table_are_refused() {
    let err = load(
        "min_balance_usd = 2.5\n\
         [providers.deepseek]\n\
         prefixes = [\"deepseek-\"]\n\
         base_url = \"https://api.deepseek.com/anthropic\"\n\
         key_env = \"DEEPSEEK_API_KEY\"\n\
         auth = \"x-api-key\"\n",
    )
    .unwrap_err();
    assert!(matches!(err, ConfigError::LegacyBalance));
}

#[test]
fn a_provider_table_that_breaks_a_rule_is_refused() {
    let refused: [Rejection; 7] = [
        (
            "a prefix another provider uses",
            |c| {
                c.providers
                    .insert("other".to_owned(), provider(&["deepseek-"], &[]));
            },
            |e| matches!(e, ConfigError::DuplicatePrefix(_)),
        ),
        (
            "an empty prefix",
            |c| {
                c.providers
                    .insert("other".to_owned(), provider(&["", "other-"], &[]));
            },
            |e| matches!(e, ConfigError::EmptyPrefix { .. }),
        ),
        (
            "no prefixes",
            |c| {
                c.providers.insert("other".to_owned(), provider(&[], &[]));
            },
            |e| matches!(e, ConfigError::NoPrefixes { .. }),
        ),
        (
            "a model outside every prefix",
            |c| {
                c.providers.insert(
                    "other".to_owned(),
                    provider(&["other-"], &["other-model", "gpt-5"]),
                );
            },
            |e| matches!(e, ConfigError::ModelOutsidePrefix { .. }),
        ),
        (
            "a negative minimum",
            |c| c.provider_balance_mut("deepseek").unwrap().min_usd = -0.5,
            |e| matches!(e, ConfigError::MinBalance(_)),
        ),
        (
            "no refresh minutes",
            |c| c.provider_balance_mut("deepseek").unwrap().refresh_minutes = 0,
            |e| matches!(e, ConfigError::BalanceRefresh(0)),
        ),
        (
            "an empty key environment",
            |c| {
                c.providers.insert(
                    "other".to_owned(),
                    Provider {
                        key_env: String::new(),
                        ..provider(&["other-"], &[])
                    },
                );
            },
            |e| matches!(e, ConfigError::KeyEnv { .. }),
        ),
    ];
    for (name, change, matches_error) in refused {
        let mut config = Config::default();
        change(&mut config);
        match config.validate() {
            Ok(()) => panic!("{name} was accepted"),
            Err(error) => assert!(matches_error(&error), "{name}: {error}"),
        }
    }
}

#[test]
fn a_setting_that_breaks_a_rule_is_refused() {
    let refused: [Rejection; 12] = [
        (
            "a port of 0",
            |c| c.port = 0,
            |e| matches!(e, ConfigError::PortZero),
        ),
        (
            "too few context tokens",
            |c| c.max_context_tokens = 999,
            |e| matches!(e, ConfigError::MaxContext(999)),
        ),
        (
            "an empty agent name",
            |c| c.agents = vec![agent("", "deepseek-flash")],
            |e| matches!(e, ConfigError::AgentName(_)),
        ),
        (
            "a dotted agent name",
            |c| c.agents = vec![agent("a.b", "deepseek-flash")],
            |e| matches!(e, ConfigError::AgentName(_)),
        ),
        (
            "a parent agent name",
            |c| c.agents = vec![agent("../x", "deepseek-flash")],
            |e| matches!(e, ConfigError::AgentName(_)),
        ),
        (
            "a slash in the agent name",
            |c| c.agents = vec![agent("a/b", "deepseek-flash")],
            |e| matches!(e, ConfigError::AgentName(_)),
        ),
        (
            "a space in the agent name",
            |c| c.agents = vec![agent("a b", "deepseek-flash")],
            |e| matches!(e, ConfigError::AgentName(_)),
        ),
        (
            "a name ending in -ds",
            |c| c.agents = vec![agent("senior-worker-ds", "deepseek-flash")],
            |e| matches!(e, ConfigError::DerivedName(_)),
        ),
        (
            "a model outside deepseek",
            |c| c.agents = vec![agent("consultant", "gpt-5")],
            |e| matches!(e, ConfigError::AgentModel { .. }),
        ),
        (
            "one agent configured twice",
            |c| {
                c.agents = vec![agent("a", "deepseek-flash"), agent("a", "deepseek-v4-pro")];
            },
            |e| matches!(e, ConfigError::DuplicateAgent(_)),
        ),
        (
            "a NaN minimum",
            |c| c.provider_balance_mut("deepseek").unwrap().min_usd = f64::NAN,
            |e| matches!(e, ConfigError::MinBalance(_)),
        ),
        (
            "an infinite minimum",
            |c| c.provider_balance_mut("deepseek").unwrap().min_usd = f64::INFINITY,
            |e| matches!(e, ConfigError::MinBalance(_)),
        ),
    ];
    for (name, change, matches_error) in refused {
        let mut config = Config::default();
        change(&mut config);
        match config.validate() {
            Ok(()) => panic!("{name} was accepted"),
            Err(error) => assert!(matches_error(&error), "{name}: {error}"),
        }
    }

    let accepted: [(&str, Change); 6] = [
        ("a zero minimum", |c| {
            c.provider_balance_mut("deepseek").unwrap().min_usd = 0.0;
        }),
        ("one refresh minute", |c| {
            c.provider_balance_mut("deepseek").unwrap().refresh_minutes = 1;
        }),
        ("1000 context tokens", |c| c.max_context_tokens = 1000),
        ("letters, digits, dashes and underscores in a name", |c| {
            c.agents = vec![agent("senior_worker-2", "deepseek-flash")];
        }),
        ("a known model", |c| {
            c.agents = vec![agent("consultant", "deepseek-v4-pro")];
        }),
        ("a second provider with its own prefixes", |c| {
            c.providers
                .insert("other".to_owned(), provider(&["other-"], &["other-model"]));
        }),
    ];
    for (name, change) in accepted {
        let mut config = Config::default();
        change(&mut config);
        assert!(config.validate().is_ok(), "{name}");
    }
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
fn an_unknown_agent_names_the_configured_ones() {
    let attempts: [Attempt; 2] = [
        ("remove", |c| c.remove_agent("nobody")),
        ("suspend", |c| c.set_suspended("nobody", true)),
    ];
    for (name, attempt) in attempts {
        let mut c = Config::default();
        let err = attempt(&mut c).unwrap_err();
        assert!(matches!(err, ConfigError::NoSuchAgent { .. }), "{name}");
        assert_eq!(
            err.hint().as_deref(),
            Some("configured agents: senior-worker"),
            "{name}"
        );
    }
}

#[test]
fn suspending_keeps_the_entry_and_activating_restores_it() {
    let mut c = Config::default();
    c.set_suspended("senior-worker", true).unwrap();
    assert!(c.agents[0].suspended);
    assert_eq!(c.agents[0].model, "deepseek-flash");
    c.set_suspended("senior-worker", false).unwrap();
    assert!(!c.agents[0].suspended);
}

#[test]
fn suspending_all_marks_every_agent() {
    let mut c = Config::default();
    c.add_agent(agent("consultant", "deepseek-v4-pro")).unwrap();
    c.set_all_suspended(true);
    assert!(c.agents.iter().all(|a| a.suspended));
    c.set_all_suspended(false);
    assert!(c.agents.iter().all(|a| !a.suspended));
}
