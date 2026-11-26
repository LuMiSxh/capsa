// SPDX-License-Identifier: MPL-2.0

use std::ffi::OsString;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use tempfile::TempDir;

use super::*;
use crate::render::{Doc, Profile, Render};

#[derive(Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Settings {
    name: String,
    limits: Limits,
    tags: Vec<String>,
}

#[derive(Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Limits {
    five_hour: u32,
    enabled: bool,
}

fn file_in(dir: &TempDir) -> ConfigFile {
    ConfigFile::at(dir.path().join("nested").join("config.toml"))
}

fn write_file(file: &ConfigFile, text: &str) {
    fs::create_dir_all(file.path().parent().expect("a parent")).expect("a directory");
    fs::write(file.path(), text).expect("a file");
}

fn rendered(shown: &Shown) -> String {
    let mut doc = Doc::default();
    shown.doc(&mut doc);
    let mut out = Vec::new();
    doc.write(&mut out, Profile::AGENT).expect("rendered");
    String::from_utf8(out).expect("utf-8")
}

#[test]
fn load_reads_the_default_or_the_files_values() {
    let dir = TempDir::new().unwrap();
    let loaded: Settings = file_in(&dir).load().unwrap();
    assert_eq!(loaded, Settings::default());

    let dir = TempDir::new().unwrap();
    let file = file_in(&dir);
    write_file(&file, "name = \"acus\"\n[limits]\nfive_hour = 80\n");
    let loaded: Settings = file.load().unwrap();
    assert_eq!(loaded.name, "acus");
    assert_eq!(loaded.limits.five_hour, 80);
}

#[test]
fn load_reports_a_parse_error_for_bad_syntax_or_an_unknown_key() {
    let dir = TempDir::new().unwrap();
    let file = file_in(&dir);
    write_file(&file, "name = \"acus\"\nlimits = [\n");
    let Some(ConfigError::Parse { path, line, .. }) = file.load::<Settings>().err() else {
        panic!("expected a parse error");
    };
    assert_eq!(path, file.path());
    assert!(line >= 2, "line was {line}");

    let dir = TempDir::new().unwrap();
    let file = file_in(&dir);
    write_file(&file, "bogus = 1\n");
    assert!(matches!(
        file.load::<Settings>(),
        Err(ConfigError::Parse { .. })
    ));
}

#[test]
fn save_round_trips_and_leaves_only_the_config_file() {
    let dir = TempDir::new().unwrap();
    let file = file_in(&dir);
    let value = Settings {
        name: "census".to_owned(),
        limits: Limits {
            five_hour: 42,
            enabled: true,
        },
        tags: vec!["a".to_owned(), "b".to_owned()],
    };
    file.save(&value).unwrap();
    let loaded: Settings = file.load().unwrap();
    assert_eq!(loaded, value);

    let parent = file.path().parent().expect("a parent");
    let names: Vec<OsString> = fs::read_dir(parent)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(names, [OsString::from("config.toml")]);
}

#[test]
fn set_writes_valid_dotted_keys_and_keeps_non_toml_text_as_a_string() {
    let dir = TempDir::new().unwrap();
    let file = file_in(&dir);
    file.set::<Settings>("limits.five_hour", "75").unwrap();
    file.set::<Settings>("limits.enabled", "true").unwrap();
    file.set::<Settings>("tags", "[\"x\", \"y\"]").unwrap();
    file.set::<Settings>("name", "vicarius").unwrap();
    let loaded: Settings = file.load().unwrap();
    assert_eq!(loaded.limits.five_hour, 75);
    assert!(loaded.limits.enabled);
    assert_eq!(loaded.tags, ["x", "y"]);
    assert_eq!(loaded.name, "vicarius");

    let dir = TempDir::new().unwrap();
    let file = file_in(&dir);
    file.set::<Settings>("name", "hello world").unwrap();
    let loaded: Settings = file.load().unwrap();
    assert_eq!(loaded.name, "hello world");
}

#[test]
fn set_rejects_an_invalid_value_and_leaves_the_file_alone() {
    let cases = [
        (Some(("name", "acus")), "limits.nope", "1", "limits.nope"),
        (None, "limits.five_hour", "\"many\"", "limits.five_hour"),
    ];
    for (prior, key, value, expected) in cases {
        let dir = TempDir::new().unwrap();
        let file = file_in(&dir);
        if let Some((prior_key, prior_value)) = prior {
            file.set::<Settings>(prior_key, prior_value).unwrap();
        }
        let before = fs::read_to_string(file.path()).ok();
        let err = file.set::<Settings>(key, value).err();
        assert!(
            matches!(err, Some(ConfigError::Invalid { ref key, .. }) if key == expected),
            "{key}={value}"
        );
        assert_eq!(
            fs::read_to_string(file.path()).ok(),
            before,
            "{key}={value}"
        );
    }
}

#[test]
fn set_rejects_a_key_that_cannot_be_addressed() {
    let cases = [
        (Some(("name", "acus")), "name.inner"),
        (None, "limits..five_hour"),
    ];
    for (prior, key) in cases {
        let dir = TempDir::new().unwrap();
        let file = file_in(&dir);
        if let Some((prior_key, prior_value)) = prior {
            file.set::<Settings>(prior_key, prior_value).unwrap();
        }
        let err = file.set::<Settings>(key, "1").err();
        assert!(matches!(err, Some(ConfigError::Key { .. })), "{key}");
    }
}

#[test]
fn override_path_wins_and_an_empty_one_is_ignored() {
    let dir = PathBuf::from("/config/acus");
    assert_eq!(
        choose_path(Some(OsString::from("/elsewhere/c.toml")), &dir),
        PathBuf::from("/elsewhere/c.toml")
    );
    assert_eq!(
        choose_path(Some(OsString::new()), &dir),
        dir.join("config.toml")
    );
    assert_eq!(choose_path(None, &dir), dir.join("config.toml"));
}

#[test]
fn legacy_json_is_found_only_when_it_exists() {
    let dir = TempDir::new().unwrap();
    assert_eq!(legacy_json_in(dir.path()), None);
    fs::write(dir.path().join("config.json"), "{}").unwrap();
    assert_eq!(
        legacy_json_in(dir.path()),
        Some(dir.path().join("config.json"))
    );
}

#[test]
fn show_renders_dotted_keys_and_flat_json() {
    let dir = TempDir::new().unwrap();
    let file = file_in(&dir);
    let value = Settings {
        name: "acus".to_owned(),
        limits: Limits {
            five_hour: 5,
            enabled: false,
        },
        tags: Vec::new(),
    };
    let text = rendered(&file.show("acus self config", &value).unwrap());
    assert!(text.contains("limits.five_hour"), "{text}");
    assert!(text.contains("limits.enabled"), "{text}");
    assert!(text.contains("tags"), "{text}");
    assert!(text.contains(&file.path().display().to_string()), "{text}");

    let shown = file.show("acus self config", &Settings::default()).unwrap();
    let json = serde_json::to_value(&shown).unwrap();
    assert_eq!(json["name"], "");
    assert_eq!(json["limits"]["five_hour"], 0);
    assert_eq!(json["path"], file.path().display().to_string());
}

#[test]
fn only_errors_the_user_can_fix_carry_a_hint() {
    let parse = ConfigError::Parse {
        path: PathBuf::from("c.toml"),
        line: 1,
        column: 1,
        message: String::new(),
    };
    assert!(parse.hint().is_some());
    let read = ConfigError::Read {
        path: PathBuf::from("c.toml"),
        source: io::Error::other("x"),
    };
    assert!(read.hint().is_none());
}
