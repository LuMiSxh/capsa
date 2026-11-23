// SPDX-License-Identifier: MPL-2.0

use super::fs::decode;
use super::{ReadOptions, env, exists, masked, parse_toml, parse_yaml, read, stat, text_diff};
use capsa_core::claude_settings::permissions::Permissions;
use serde_json::{Value, json};
use std::path::PathBuf;

/// A project with a git root and a directory beside it, in a temporary directory.
struct Project {
    dir: tempfile::TempDir,
}

impl Project {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("repo/.git")).unwrap();
        std::fs::create_dir_all(dir.path().join("outside")).unwrap();
        Project { dir }
    }

    fn root(&self) -> PathBuf {
        self.dir.path().join("repo").canonicalize().unwrap()
    }

    fn write(&self, rel: &str, bytes: impl AsRef<[u8]>) {
        let file = self.dir.path().join(rel);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, bytes).unwrap();
    }

    /// The permissions of the project, with `settings` as its only settings file.
    fn permissions(&self, settings: &Value) -> Permissions {
        self.write("repo/.claude/settings.json", settings.to_string());
        let file = self.dir.path().join("repo/.claude/settings.json");
        Permissions::with_files(vec![file], self.root(), self.root(), None)
    }
}

#[test]
fn read_returns_the_whole_text_without_options() {
    let project = Project::new();
    project.write("repo/notes.txt", "one\ntwo\n");
    let perms = project.permissions(&json!({}));
    let text = read(
        &perms,
        &project.root(),
        "notes.txt",
        &ReadOptions::default(),
    )
    .unwrap();
    assert_eq!(text, json!("one\ntwo\n"));
}

#[test]
fn read_selects_lines_from_offset_and_limit() {
    let project = Project::new();
    project.write("repo/notes.txt", "a\nb\nc\nd\n");
    let perms = project.permissions(&json!({}));
    let options = ReadOptions {
        offset: Some(2),
        limit: Some(2),
        numbered: false,
    };
    let text = read(&perms, &project.root(), "notes.txt", &options).unwrap();
    assert_eq!(text, json!("b\nc\n"));
}

#[test]
fn read_numbers_lines_by_their_place_in_the_file() {
    let project = Project::new();
    project.write("repo/notes.txt", "a\nb\nc\nd\n");
    let perms = project.permissions(&json!({}));
    let options = ReadOptions {
        offset: Some(3),
        limit: None,
        numbered: true,
    };
    let text = read(&perms, &project.root(), "notes.txt", &options).unwrap();
    assert_eq!(text, json!("3\tc\n4\td\n"));
}

#[test]
fn read_refuses_an_offset_of_zero_with_a_hint() {
    let project = Project::new();
    project.write("repo/notes.txt", "a\n");
    let perms = project.permissions(&json!({}));
    let options = ReadOptions {
        offset: Some(0),
        ..ReadOptions::default()
    };
    let err = read(&perms, &project.root(), "notes.txt", &options).unwrap_err();
    assert!(err.hint.is_some());
}

#[test]
fn read_returns_images_and_binary_files_as_metadata() {
    let project = Project::new();
    project.write("repo/pic.png", b"\x89PNG\x00\x01");
    project.write("repo/data.bin", [0u8, 1, 2]);
    let perms = project.permissions(&json!({}));
    let root = project.root();
    let image = read(&perms, &root, "pic.png", &ReadOptions::default()).unwrap();
    assert_eq!(image["kind"], json!("image"));
    assert_eq!(image["bytes"], json!(6));
    let binary = read(&perms, &root, "data.bin", &ReadOptions::default()).unwrap();
    assert_eq!(binary["kind"], json!("binary"));
    assert_eq!(binary["bytes"], json!(3));
    assert!(binary["hint"].as_str().unwrap().contains("Read tool"));
}

#[test]
fn read_refuses_a_path_outside_the_roots() {
    let project = Project::new();
    project.write("outside/notes.txt", "secret\n");
    let perms = project.permissions(&json!({}));
    let err = read(
        &perms,
        &project.root(),
        "../outside/notes.txt",
        &ReadOptions::default(),
    )
    .unwrap_err();
    assert!(err.message.contains("outside the allowed roots"));
}

#[test]
fn read_allows_a_directory_listed_in_additional_directories() {
    let project = Project::new();
    project.write("outside/notes.txt", "shared\n");
    let extra = project.dir.path().join("outside").display().to_string();
    let perms = project.permissions(&json!({"permissions": {"additionalDirectories": [extra]}}));
    let text = read(
        &perms,
        &project.root(),
        "../outside/notes.txt",
        &ReadOptions::default(),
    )
    .unwrap();
    assert_eq!(text, json!("shared\n"));
}

#[test]
fn read_refuses_a_path_the_read_rules_deny() {
    let project = Project::new();
    project.write("repo/secret.txt", "x");
    let perms = project.permissions(&json!({"permissions": {"deny": ["Read(./secret.txt)"]}}));
    let err = read(
        &perms,
        &project.root(),
        "secret.txt",
        &ReadOptions::default(),
    )
    .unwrap_err();
    assert!(err.message.contains("is denied"), "{}", err.message);
}

#[test]
fn exists_reports_a_missing_path_inside_the_roots_as_false() {
    let project = Project::new();
    project.write("repo/notes.txt", "x");
    let perms = project.permissions(&json!({}));
    let root = project.root();
    assert!(exists(&perms, &root, "notes.txt").unwrap());
    assert!(!exists(&perms, &root, "missing.txt").unwrap());
}

#[test]
fn exists_refuses_a_path_outside_the_roots() {
    let project = Project::new();
    project.write("outside/notes.txt", "secret\n");
    let perms = project.permissions(&json!({}));
    assert!(exists(&perms, &project.root(), "../outside/notes.txt").is_err());
}

#[test]
fn stat_reports_size_lines_and_modification_time_of_a_text_file() {
    let project = Project::new();
    project.write("repo/notes.txt", "one\ntwo");
    let perms = project.permissions(&json!({}));
    let info = stat(&perms, &project.root(), "notes.txt").unwrap();
    assert_eq!(info["exists"], json!(true));
    assert_eq!(info["dir"], json!(false));
    assert_eq!(info["bytes"], json!(7));
    assert_eq!(info["lines"], json!(2));
    let modified = info["modified"].as_str().unwrap();
    assert_eq!(modified.len(), 20);
    assert!(modified.ends_with("Z"));
}

#[test]
fn stat_leaves_lines_out_for_a_directory() {
    let project = Project::new();
    let perms = project.permissions(&json!({}));
    let info = stat(&perms, &project.root(), ".").unwrap();
    assert_eq!(info["dir"], json!(true));
    assert!(info.get("lines").is_none());
}

#[test]
fn stat_reports_a_missing_path_as_absent() {
    let project = Project::new();
    let perms = project.permissions(&json!({}));
    let info = stat(&perms, &project.root(), "missing.txt").unwrap();
    assert_eq!(info, json!({"exists": false, "dir": false}));
}

#[test]
fn env_masks_the_value_of_a_secret_name() {
    assert_eq!(
        masked("GITHUB_TOKEN", Some("abc".to_owned())),
        json!("<set>")
    );
    assert_eq!(
        masked("db_password", Some("abc".to_owned())),
        json!("<set>")
    );
    assert_eq!(masked("ACUS_LOG", Some("debug".to_owned())), json!("debug"));
    assert_eq!(masked("SOME_KEY", None), Value::Null);
}

#[test]
fn env_returns_null_for_an_unset_or_invalid_name() {
    assert_eq!(env("ACUS_HELPERS_TEST_UNSET_VARIABLE"), Value::Null);
    assert_eq!(env(""), Value::Null);
    assert_eq!(env("A=B"), Value::Null);
}

#[test]
fn parse_toml_converts_tables_arrays_and_datetimes() {
    let text = r#"
title = "x"
count = 3
ratio = 0.5
on = true
when = 1979-05-27T07:32:00Z
tags = ["a", "b"]

[owner]
name = "me"
"#;
    let value = parse_toml(text).unwrap();
    assert_eq!(value["title"], json!("x"));
    assert_eq!(value["count"], json!(3));
    assert_eq!(value["ratio"], json!(0.5));
    assert_eq!(value["on"], json!(true));
    assert_eq!(value["tags"], json!(["a", "b"]));
    assert_eq!(value["owner"]["name"], json!("me"));
    assert!(
        value["when"]
            .as_str()
            .unwrap()
            .starts_with("1979-05-27T07:32:00")
    );
}

#[test]
fn parse_toml_refuses_text_that_is_not_toml() {
    assert!(parse_toml("a = ").is_err());
}

#[test]
fn parse_yaml_converts_the_first_document_and_ignores_the_rest() {
    let text = "title: x\ncount: 3\ntags:\n  - a\n  - b\nowner:\n  name: me\n---\nlater: 1\n";
    let value = parse_yaml(text).unwrap();
    assert_eq!(value["title"], json!("x"));
    assert_eq!(value["count"], json!(3));
    assert_eq!(value["tags"], json!(["a", "b"]));
    assert_eq!(value["owner"]["name"], json!("me"));
    assert!(value.get("later").is_none());
}

#[test]
fn parse_yaml_refuses_text_that_is_not_yaml_with_a_hint() {
    let err = parse_yaml("a: [").unwrap_err();
    assert!(err.hint.is_some());
}

#[test]
fn text_diff_shows_changed_lines_with_context() {
    let diff = text_diff("one\ntwo\nthree\n", "one\n2\nthree\n", Some(1));
    assert!(diff.contains("\n-two\n"), "{diff}");
    assert!(diff.contains("\n+2\n"), "{diff}");
    assert!(diff.contains("\n one\n"), "{diff}");
}

#[test]
fn text_diff_of_equal_texts_is_empty() {
    assert_eq!(text_diff("same\n", "same\n", None), "");
}

#[test]
fn decode_refuses_nul_bytes_and_invalid_utf8() {
    assert_eq!(decode(b"text".to_vec()), Some("text".to_owned()));
    assert_eq!(decode(b"a\x00".to_vec()), None);
    assert_eq!(decode(vec![0xff]), None);
}
