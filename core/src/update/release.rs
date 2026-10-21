// SPDX-License-Identifier: MPL-2.0
//! Finds a tool's newest GitHub release and downloads its assets.

use std::process::Command;

use serde_json::Value;

use super::{REPO, Tool, UpdateError};

/// Releases of other tools can fill the first pages of the release list.
const MAX_PAGES: u32 = 10;
/// Release archives are a few MB; ureq's default body limit is 10 MB.
const MAX_DOWNLOAD: u64 = 64 << 20;

/// Holds a published release: its tag and the API object that lists its assets.
pub(super) struct Release {
    pub(super) tag: String,
    json: Value,
}

impl Release {
    /// Returns the version in the tag, without the `<tool>-v` prefix.
    pub(super) fn version(&self, tool: &Tool) -> &str {
        self.tag
            .strip_prefix(&prefix(tool.name))
            .unwrap_or(&self.tag)
    }

    /// Downloads the asset `name`.
    pub(super) fn download(&self, name: &str, token: &str) -> Result<Vec<u8>, UpdateError> {
        let url = asset_url(&self.json, name).ok_or_else(|| UpdateError::MissingAsset {
            tag: self.tag.clone(),
            name: name.to_owned(),
        })?;
        // The API URL returns the bytes only when asked for `application/octet-stream`.
        send(&url, token, "application/octet-stream")?
            .body_mut()
            .with_config()
            .limit(MAX_DOWNLOAD)
            .read_to_vec()
            .map_err(|source| http(&url, source))
    }
}

fn prefix(tool: &str) -> String {
    format!("{tool}-v")
}

fn http(url: &str, source: ureq::Error) -> UpdateError {
    UpdateError::Http {
        url: url.to_owned(),
        source: Box::new(source),
    }
}

/// Returns the token from `GITHUB_TOKEN`, else from `gh auth token`, as the installers
/// resolve it.
///
/// `gh auth token` is the only program this module starts.
pub(super) fn token() -> Result<String, UpdateError> {
    std::env::var("GITHUB_TOKEN")
        .ok()
        .map(|t| t.trim().to_owned())
        .filter(|t| !t.is_empty())
        .or_else(gh_token)
        .ok_or(UpdateError::NoToken)
}

fn gh_token() -> Option<String> {
    let out = Command::new("gh").args(["auth", "token"]).output().ok()?;
    let token = String::from_utf8(out.stdout).ok()?;
    let token = token.trim();
    (out.status.success() && !token.is_empty()).then(|| token.to_owned())
}

/// Returns the newest release of `tool` that is neither a draft nor a prerelease.
pub(super) fn newest(tool: &Tool, token: &str) -> Result<Release, UpdateError> {
    for page in 1..=MAX_PAGES {
        let url = format!("https://api.github.com/repos/{REPO}/releases?per_page=100&page={page}");
        let releases: Vec<Value> = send(&url, token, "application/vnd.github+json")?
            .body_mut()
            .read_json()
            .map_err(|source| http(&url, source))?;
        if releases.is_empty() {
            break;
        }
        if let Some(found) = select_release(releases, tool.name) {
            return Ok(found);
        }
    }
    Err(UpdateError::NoRelease { tool: tool.name })
}

/// Picks the published release of `tool` with the highest version, whatever order GitHub lists
/// them in. A release with the draft or prerelease flag set is skipped; on equal versions the
/// one listed first wins.
pub(super) fn select_release(releases: Vec<Value>, tool: &str) -> Option<Release> {
    let prefix = prefix(tool);
    let mut best: Option<(Vec<u64>, Release)> = None;
    for json in releases {
        let Some(tag) = json["tag_name"].as_str().map(str::to_owned) else {
            continue;
        };
        let published = !json["draft"].as_bool().unwrap_or(true)
            && !json["prerelease"].as_bool().unwrap_or(true);
        let Some(rest) = tag.strip_prefix(&prefix) else {
            continue;
        };
        if !published {
            continue;
        }
        let ver = version(rest);
        if best.as_ref().is_none_or(|(top, _)| ver > *top) {
            best = Some((ver, Release { tag, json }));
        }
    }
    best.map(|(_, release)| release)
}

/// Returns the archive name the release workflow gives `target`.
pub(super) fn archive_name(tool: &str, target: &str, ext: &str) -> String {
    format!("{tool}-{target}.{ext}")
}

/// Returns the API URL of the asset `name` in a release object.
pub(super) fn asset_url(release: &Value, name: &str) -> Option<String> {
    let asset = release["assets"]
        .as_array()?
        .iter()
        .find(|a| a["name"].as_str() == Some(name))?;
    asset["url"].as_str().map(String::from)
}

/// Sends an authenticated GET; the repository is private, so every request needs the token.
fn send(
    url: &str,
    token: &str,
    accept: &str,
) -> Result<ureq::http::Response<ureq::Body>, UpdateError> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .user_agent(format!("capsa/{}", env!("CARGO_PKG_VERSION")))
        .build()
        .into();
    agent
        .get(url)
        .header("Authorization", format!("Bearer {token}"))
        .header("Accept", accept)
        .call()
        .map_err(|source| http(url, source))
}

/// Parses `v` into numbers that order `0.10.1` above `0.9.3`; non-numeric suffixes are ignored.
pub(super) fn version(v: &str) -> Vec<u64> {
    v.trim()
        .split('.')
        .map(|p| {
            let digits: String = p.chars().take_while(char::is_ascii_digit).collect();
            digits.parse().unwrap_or(0)
        })
        .collect()
}

/// Returns the release target for this platform, as the release workflow builds it.
pub(super) fn target() -> Result<&'static str, UpdateError> {
    Ok(match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => "aarch64-apple-darwin",
        ("macos", "x86_64") => "x86_64-apple-darwin",
        ("linux", "x86_64") => "x86_64-unknown-linux-gnu",
        ("linux", "aarch64") => "aarch64-unknown-linux-gnu",
        ("windows", "x86_64") => "x86_64-pc-windows-msvc",
        (os, arch) => return Err(UpdateError::UnsupportedPlatform { os, arch }),
    })
}
