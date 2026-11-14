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
use crate::bulletin::{self, Stamped};
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

/// The release bulletin of a tool: the version it runs and the newest version it last saw.
///
/// Other tools read it through [`release_status`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Release {
    pub version: String,
    pub latest: String,
}

/// Returns whether the running `tool` is the newest release.
///
/// The newest version comes from the state directory's `latest.json` when the file is less than a
/// day old, and from GitHub otherwise, which then rewrites the file. Once a version is known, the
/// `<tool>-release` bulletin is rewritten too, see [`release_status`]. It never fails: a release
/// that cannot be read is [`UpdateCheck::Unknown`].
///
/// With `CAPSA_UPDATE_CHECK=off` in the environment the check returns `Unknown` without reading the
/// file, writing a bulletin or asking GitHub. The tools' tests set it, so that no test run reaches
/// the network.
pub fn check(tool: &Tool) -> UpdateCheck {
    if disabled_by(std::env::var_os(SWITCH).as_deref()) {
        return UpdateCheck::Unknown;
    }
    let path = cache_path(tool.name);
    check_with(tool, &path, &bulletin::dir(), bulletin::now(), |running| {
        let token = release::token();
        release::newest(running, token.as_deref(), Some(TIMEOUT))
            .map(|found| found.version(running).to_owned())
    })
}

/// Saves `latest` as the newest version of `tool`, so that the next check need not ask GitHub, and
/// writes the release bulletin with `version` as the installed version. A file that cannot be
/// written only costs one more request, so the errors are dropped.
pub(super) fn remember(tool: &Tool, version: &str, latest: &str) {
    remember_in(
        &cache_path(tool.name),
        &bulletin::dir(),
        tool.name,
        version,
        bulletin::now(),
        latest,
    );
}

pub(super) fn remember_in(
    path: &Path,
    bulletins: &Path,
    tool: &str,
    version: &str,
    now: u64,
    latest: &str,
) {
    let _ = save(path, now, latest);
    publish(bulletins, tool, version, now, latest);
}

/// Judges `tool` against the version saved at `path` when it is fresh at `now`; otherwise it
/// judges against what `fetch` returns and saves that. Either way the release bulletin in
/// `bulletins` is rewritten, so a binary that was just installed corrects it at its next check.
pub(super) fn check_with(
    tool: &Tool,
    path: &Path,
    bulletins: &Path,
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
    publish(bulletins, tool.name, tool.version, now, &latest);
    judge(tool.version, &latest)
}

/// Whether `value`, the value of [`SWITCH`], switches the check off.
///
/// Only the exact value `off` does; any other value, or none, leaves the check on.
pub(super) fn disabled_by(value: Option<&OsStr>) -> bool {
    value == Some(OsStr::new("off"))
}

/// Returns the name of the release bulletin of `tool`, which is `<tool>-release`.
pub(super) fn release_bulletin(tool: &str) -> String {
    format!("{tool}-release")
}

/// Returns the newest version of `tool` that its last check saw, when the release bulletin is
/// current and that release is newer than the running version it recorded.
pub fn release_status(tool: &str) -> Option<String> {
    release_status_in(&bulletin::dir(), tool, bulletin::now())
}

pub(super) fn release_status_in(bulletins: &Path, tool: &str, now: u64) -> Option<String> {
    let stamped: Stamped<Release> = bulletin::read_in(bulletins, &release_bulletin(tool))?;
    let release = stamped.current(now)?;
    is_older(&release.version, &release.latest).then(|| release.latest.clone())
}

/// Writes the release bulletin of `tool` into `bulletins`, with its running `version` and
/// `latest`. A failed write leaves the previous bulletin, so the error is dropped.
fn publish(bulletins: &Path, tool: &str, version: &str, now: u64, latest: &str) {
    let release = Stamped {
        at: now,
        ttl: MAX_AGE_SECS,
        value: Release {
            version: version.to_owned(),
            latest: latest.to_owned(),
        },
    };
    let _ = bulletin::write_in(bulletins, &release_bulletin(tool), &release);
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
