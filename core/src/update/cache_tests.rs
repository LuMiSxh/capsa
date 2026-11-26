// SPDX-License-Identifier: MPL-2.0

use std::cell::Cell;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use super::cache::{Release, UpdateCheck, check_with, disabled_by, release_status_in, remember_in};
use super::{Tool, UpdateError};
use crate::bulletin::{self, Stamped};
use crate::render::Status;

const RUNNING: Tool = Tool {
    name: "acus",
    version: "0.2.0",
};
const NOW: u64 = 1_000_000;
const DAY: u64 = 24 * 60 * 60;

fn saved(dir: &Path, checked_at: u64, latest: &str) -> PathBuf {
    let path = dir.join("latest.json");
    let body = format!(r#"{{"checked_at":{checked_at},"latest":"{latest}"}}"#);
    std::fs::write(&path, body).unwrap();
    path
}

fn no_fetch(_: &Tool) -> Result<String, UpdateError> {
    Err(UpdateError::NoRelease { tool: "acus" })
}

fn verdict(current: &'static str, latest: &str) -> UpdateCheck {
    let dir = tempfile::tempdir().unwrap();
    let path = saved(dir.path(), NOW, latest);
    let tool = Tool {
        name: "acus",
        version: current,
    };
    check_with(&tool, &path, dir.path(), NOW, no_fetch)
}

#[test]
fn a_saved_version_is_used_while_fresh_and_fetched_once_stale() {
    let cases = [
        (NOW - 60, "0.3.0", "9.9.9", 0, "0.3.0", None),
        (NOW - DAY, "0.1.0", "0.3.0", 1, "0.3.0", Some("0.3.0")),
        (NOW + 60, "0.1.0", "0.3.0", 1, "0.3.0", None),
    ];
    for (checked_at, saved_latest, fetched, expected_calls, expected, rewritten) in cases {
        let dir = tempfile::tempdir().unwrap();
        let path = saved(dir.path(), checked_at, saved_latest);
        let calls = Cell::new(0);
        let result = check_with(&RUNNING, &path, dir.path(), NOW, |_| {
            calls.set(calls.get() + 1);
            Ok(fetched.to_owned())
        });
        assert_eq!(calls.get(), expected_calls, "{checked_at}");
        assert_eq!(
            result,
            UpdateCheck::Available(expected.to_owned()),
            "{checked_at}"
        );
        if let Some(latest) = rewritten {
            let text = std::fs::read_to_string(&path).unwrap();
            assert!(text.contains(&format!(r#""checked_at":{NOW}"#)), "{text}");
            assert!(text.contains(&format!(r#""latest":"{latest}""#)), "{text}");
        }
    }
}

#[test]
fn an_unanswered_check_or_a_corrupt_saved_file_is_unknown_and_saves_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("latest.json");
    let result = check_with(&RUNNING, &path, dir.path(), NOW, no_fetch);
    assert_eq!(result, UpdateCheck::Unknown);
    assert!(!path.exists());
    assert!(!dir.path().join("acus-release.json").exists());

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("latest.json");
    std::fs::write(&path, r#"{"checked_at":"#).unwrap();
    let result = check_with(&RUNNING, &path, dir.path(), NOW, no_fetch);
    assert_eq!(result, UpdateCheck::Unknown);
    let line = crate::doctor::update_check(&RUNNING, &result);
    assert_eq!(line.status, Status::Ok);
    assert_eq!(line.detail, "could not check for updates");
}

#[test]
fn a_newer_release_is_available_and_a_version_that_is_not_semver_is_unknown() {
    assert_eq!(
        verdict("0.2.0", "0.3.0"),
        UpdateCheck::Available("0.3.0".to_owned())
    );
    assert_eq!(
        verdict("0.2.0-rc.1", "0.2.0"),
        UpdateCheck::Available("0.2.0".to_owned())
    );
    assert_eq!(verdict("0.2.0", "0.2.0"), UpdateCheck::Current);
    assert_eq!(verdict("0.2.0", "0.2.0-rc.1"), UpdateCheck::Current);
    assert_eq!(verdict("0.3.0", "0.2.0"), UpdateCheck::Current);

    assert_eq!(verdict("0.2.0", "latest"), UpdateCheck::Unknown);
    assert_eq!(verdict("dev", "0.3.0"), UpdateCheck::Unknown);
}

#[test]
fn only_the_value_off_switches_the_lookup_off() {
    assert!(disabled_by(Some(OsStr::new("off"))));
    assert!(!disabled_by(Some(OsStr::new("on"))));
    assert!(!disabled_by(Some(OsStr::new(""))));
    assert!(!disabled_by(None));
}

#[test]
fn check_with_publishes_or_rewrites_the_release_bulletin() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("latest.json");
    check_with(&RUNNING, &path, dir.path(), NOW, |_| Ok("0.3.0".to_owned()));
    let stamped: Stamped<Release> = bulletin::read_in(dir.path(), "acus-release").unwrap();
    assert_eq!(stamped.at, NOW);
    assert_eq!(stamped.ttl, DAY);
    assert_eq!(
        stamped.value,
        Release {
            version: "0.2.0".to_owned(),
            latest: "0.3.0".to_owned(),
        }
    );

    let dir = tempfile::tempdir().unwrap();
    let path = saved(dir.path(), NOW - 60, "0.3.0");
    let tool = Tool {
        name: "acus",
        version: "0.1.0",
    };
    let result = check_with(&tool, &path, dir.path(), NOW, no_fetch);
    assert_eq!(result, UpdateCheck::Available("0.3.0".to_owned()));
    let stamped: Stamped<Release> = bulletin::read_in(dir.path(), "acus-release").unwrap();
    assert_eq!(stamped.value.version, "0.1.0");
    assert_eq!(stamped.value.latest, "0.3.0");
}

#[test]
fn a_release_is_named_only_while_its_bulletin_is_current_and_it_is_newer() {
    let dir = tempfile::tempdir().unwrap();
    let release = Stamped {
        at: NOW,
        ttl: DAY,
        value: Release {
            version: "0.2.0".to_owned(),
            latest: "0.3.0".to_owned(),
        },
    };
    bulletin::write_in(dir.path(), "acus-release", &release).unwrap();
    assert_eq!(
        release_status_in(dir.path(), "acus", NOW),
        Some("0.3.0".to_owned())
    );
    assert_eq!(release_status_in(dir.path(), "acus", NOW + DAY), None);
    assert_eq!(release_status_in(dir.path(), "census", NOW), None);

    for (version, latest) in [("0.3.0", "0.3.0"), ("0.4.0", "0.3.0")] {
        let release = Stamped {
            at: NOW,
            ttl: DAY,
            value: Release {
                version: version.to_owned(),
                latest: latest.to_owned(),
            },
        };
        bulletin::write_in(dir.path(), "acus-release", &release).unwrap();
        assert_eq!(
            release_status_in(dir.path(), "acus", NOW),
            None,
            "{version} then {latest}"
        );
    }
}

#[test]
fn a_remembered_release_stays_named_until_it_is_installed() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("latest.json");
    remember_in(&path, dir.path(), "acus", "0.2.0", NOW, "0.3.0");
    assert_eq!(
        release_status_in(dir.path(), "acus", NOW).as_deref(),
        Some("0.3.0")
    );
    remember_in(&path, dir.path(), "acus", "0.3.0", NOW, "0.3.0");
    assert_eq!(release_status_in(dir.path(), "acus", NOW), None);
}
