// SPDX-License-Identifier: MPL-2.0
//! Judges the running version against the newest release, reusing the version that a tool last
//! saw when it is less than a day old.

use std::ffi::OsStr;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::{Tool, UpdateError, release};
use crate::bulletin;
use crate::fs::write_cache;
use crate::paths::state_dir;
use crate::version::{is_older, is_version};

/// How long a saved latest version is reused before GitHub is asked again.
const MAX_AGE_SECS: u64 = 24 * 60 * 60;
/// How long a check waits for GitHub before it reports the release as unknown.
const TIMEOUT: Duration = Duration::from_secs(3);
/// The file in a tool's state directory that holds the newest version the tool has seen.
const CACHE_FILE: &str = "latest.json";
/// The environment variable that switches the check off when its value is `off`.
const SWITCH: &str = "CAPSA_UPDATE_CHECK";

/// Says whether the running tool is the newest release, or that the check could not tell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateCheck {
    /// The running version is the newest release.
    Current,
    /// A newer release exists, with its version.
    Available(String),
    /// GitHub did not answer, or the answer is not a version.
    Unknown,
}

/// The shape of `latest.json`; a file that does not read as this counts as missing.
#[derive(Serialize, Deserialize)]
struct Saved {
    checked_at: u64,
    latest: String,
}

/// Returns whether the running `tool` is the newest release.
///
/// The newest version comes from the state directory's `latest.json` when the file is less than a
/// day old, and from GitHub otherwise, which then rewrites the file. It never fails: a release
/// that cannot be read is [`UpdateCheck::Unknown`].
///
/// With `CAPSA_UPDATE_CHECK=off` in the environment the check returns `Unknown` without reading the
/// file or asking GitHub. The tools' tests set it, so that no test run reaches the network.
pub fn check(tool: &Tool) -> UpdateCheck {
    if disabled_by(std::env::var_os(SWITCH).as_deref()) {
        return UpdateCheck::Unknown;
    }
    let path = cache_path(tool.name);
    check_with(tool, &path, bulletin::now(), |running| {
        let token = release::token();
        release::newest(running, token.as_deref(), Some(TIMEOUT))
            .map(|found| found.version(running).to_owned())
    })
}

/// Saves `latest` as the newest version of `tool`, so that the next check need not ask GitHub. A
/// file that cannot be written only costs one more request, so the error is dropped.
pub(super) fn remember(tool: &Tool, latest: &str) {
    let _ = save(&cache_path(tool.name), bulletin::now(), latest);
}

/// Judges `tool` against the version saved at `path` when it is fresh at `now`; otherwise it
/// judges against what `fetch` returns and saves that.
pub(super) fn check_with(
    tool: &Tool,
    path: &Path,
    now: u64,
    fetch: impl FnOnce(&Tool) -> Result<String, UpdateError>,
) -> UpdateCheck {
    let latest = match read_fresh(path, now) {
        Some(latest) => latest,
        None => match fetch(tool) {
            Ok(latest) => {
                let _ = save(path, now, &latest);
                latest
            }
            Err(_) => return UpdateCheck::Unknown,
        },
    };
    judge(tool.version, &latest)
}

/// Whether `value`, the value of [`SWITCH`], switches the check off.
///
/// Only the exact value `off` does; any other value, or none, leaves the check on.
pub(super) fn disabled_by(value: Option<&OsStr>) -> bool {
    value == Some(OsStr::new("off"))
}

fn judge(current: &str, latest: &str) -> UpdateCheck {
    if !is_version(current) || !is_version(latest) {
        UpdateCheck::Unknown
    } else if is_older(current, latest) {
        UpdateCheck::Available(latest.to_owned())
    } else {
        UpdateCheck::Current
    }
}

fn cache_path(tool: &str) -> PathBuf {
    state_dir(tool).join(CACHE_FILE)
}

fn read_fresh(path: &Path, now: u64) -> Option<String> {
    let saved: Saved = serde_json::from_slice(&fs::read(path).ok()?).ok()?;
    let age = now.checked_sub(saved.checked_at)?;
    (age < MAX_AGE_SECS).then_some(saved.latest)
}

fn save(path: &Path, now: u64, latest: &str) -> io::Result<()> {
    let body = serde_json::to_vec(&Saved {
        checked_at: now,
        latest: latest.to_owned(),
    })
    .map_err(io::Error::other)?;
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    write_cache(path, &body)
}
