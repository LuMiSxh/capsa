// SPDX-License-Identifier: MPL-2.0
//! Checks the Claude ACP adapter for `pons self doctor`.

use std::cmp::Ordering;
use std::io::Read;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use capsa_core::doctor::check;
use capsa_core::render::{Check, Status};

use super::*;

/// How long the npm registry may take before the adapter's version is reported as unchecked.
const REGISTRY_TIMEOUT: Duration = Duration::from_secs(5);

/// The package whose `claudeCodeVersion` names the Claude Code that the adapter bundles.
const SDK: &str = "@anthropic-ai/claude-agent-sdk";

/// How long `claude --version` may run before the terminal version is skipped.
const CLAUDE_TIMEOUT: Duration = Duration::from_secs(3);

/// How often a bounded command is polled for its exit.
const POLL: Duration = Duration::from_millis(20);

/// Reads the adapter version from the package.json under `dir`.
pub fn adapter_version(dir: &Path, read: &dyn Fn(&Path) -> Option<String>) -> Option<String> {
    let manifest = dir
        .join("node_modules")
        .join("@agentclientprotocol")
        .join("claude-agent-acp")
        .join("package.json");
    let json: Value = serde_json::from_str(&read(&manifest)?).ok()?;
    json.get("version")?.as_str().map(str::to_owned)
}

/// Finds the Claude adapter that Zed installed itself.
pub fn adapter_dir(env: &Env) -> Option<PathBuf> {
    let tail = ["external_agents", "registry", "npx", "claude-acp"];
    let base = match env.platform {
        Platform::Macos => env
            .home
            .as_ref()?
            .join("Library")
            .join("Application Support")
            .join("Zed"),
        Platform::Linux => env.home.as_ref()?.join(".local").join("share").join("zed"),
        Platform::Windows => env.local_appdata.as_ref()?.join("Zed"),
    };
    Some(tail.iter().fold(base, |p, part| p.join(part)))
}

/// Reads the version of the adapter that the launcher would run, from the
/// `package.json` above the real path of its entry point (a symlink in a
/// `.bin` directory is followed). `None` for launchers that fetch the adapter
/// at run time.
pub fn configured_adapter(
    program: &Path,
    args: &[String],
    sys: &dyn Sys,
    read: &dyn Fn(&Path) -> Option<String>,
) -> Option<(String, PathBuf)> {
    std::iter::once(program.to_path_buf())
        .chain(
            args.iter()
                .filter(|a| a.contains(['/', '\\']))
                .map(PathBuf::from),
        )
        .find_map(|c| pkg::manifest_for(&c, PACKAGE, &|p| sys.canonicalize(p), read))
}

/// Whether the Claude ACP adapter can be started, with the fix when it cannot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdapterCheck {
    /// `ok` or `fail`.
    pub status: &'static str,
    pub detail: String,
    pub hint: Option<String>,
}

/// Builds the install command for the package manager found on `PATH`, preferring
/// pnpm, then bun, then npm. When that manager's global bin directory is not on
/// `PATH`, the hint also names it.
pub(super) fn install_hint(env: &Env, fs: &dyn Fs, sys: &dyn Sys) -> String {
    install_hint_for(env, fs, sys, PACKAGE)
}

/// Builds the global install command for `spec`, as [`install_hint`] does for the adapter.
pub(super) fn install_hint_for(env: &Env, fs: &dyn Fs, sys: &dyn Sys, spec: &str) -> String {
    match pkg::detect_in(env, fs, None) {
        Some(manager) => {
            let command = pkg::install_command(manager, spec).join(" ");
            match sys
                .global_bin_dir(manager)
                .filter(|dir| !env.path.contains(dir))
            {
                Some(dir) => format!("{command}, then put {} on PATH", dir.display()),
                None => command,
            }
        }
        None => format!("install Node.js, then run: npm install -g {spec}"),
    }
}

/// Judges the launcher's discovery: `detected` is the launcher name and the
/// path it was found at, `version` the adapter version read next to the
/// binary, `install` the hint used when nothing can run the adapter.
pub fn adapter_check(
    explicit: bool,
    detected: Option<(&str, &Path)>,
    version: Option<&str>,
    install: impl FnOnce() -> String,
) -> AdapterCheck {
    if explicit {
        return AdapterCheck {
            status: "ok",
            detail: "an explicit agent command is given; discovery does not check the adapter"
                .to_owned(),
            hint: None,
        };
    }
    match detected {
        Some(("claude-agent-acp", path)) => AdapterCheck {
            status: "ok",
            detail: format!(
                "claude-agent-acp at {} (version {})",
                path.display(),
                version.unwrap_or("unknown")
            ),
            hint: None,
        },
        Some((launcher, path)) => AdapterCheck {
            status: "ok",
            detail: format!(
                "not installed; would fetch it at run time through {launcher} at {} (version unknown)",
                path.display()
            ),
            hint: None,
        },
        None => AdapterCheck {
            status: "fail",
            detail: "not found: no claude-agent-acp, npx, pnpm, pnpx or bunx on PATH or in the user bin directories".to_owned(),
            hint: Some(install()),
        },
    }
}

/// The facts behind the freshness checks. A `None` is a fact that could not be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Freshness {
    /// Version of the adapter the launcher would run; `None` when it fetches the adapter itself.
    pub installed: Option<String>,
    /// Newest version that the npm registry lists; `None` when the registry did not answer.
    pub latest: Option<String>,
    /// Claude Code version that the adapter's Claude Agent SDK bundles.
    pub bundled: Option<String>,
    /// Version that `claude --version` prints on `PATH`.
    pub terminal: Option<String>,
    /// The command that installs the adapter's npm `latest` with the detected manager.
    pub hint: String,
}

/// Judges the installed adapter against npm's `latest`, and the bundled Claude Code against the
/// terminal's `claude`. A fact that cannot be read is an ok check that says what was skipped.
pub fn freshness_checks(f: &Freshness) -> Vec<Check> {
    vec![latest_check(f), bundled_check(f)]
}

fn latest_check(f: &Freshness) -> Check {
    let (status, detail) = match (&f.installed, &f.latest) {
        (None, _) => (
            Status::Ok,
            "skipped: the launcher fetches the adapter at run time".to_owned(),
        ),
        (Some(_), None) => (
            Status::Ok,
            "could not check: the npm registry did not answer".to_owned(),
        ),
        (Some(installed), Some(latest)) if is_older(installed, latest) => (
            Status::Warn,
            format!("{installed} is older than the npm latest {latest}"),
        ),
        (Some(installed), Some(latest)) => {
            (Status::Ok, format!("{installed}, npm latest {latest}"))
        }
    };
    check("adapter version", status, detail, warn_hint(status, f))
}

fn bundled_check(f: &Freshness) -> Check {
    let (status, detail) = match (&f.installed, &f.bundled, &f.terminal) {
        (None, _, _) => (
            Status::Ok,
            "skipped: no installed adapter to read".to_owned(),
        ),
        (Some(_), None, _) => (
            Status::Ok,
            "skipped: the Claude Code version bundled in the adapter could not be read".to_owned(),
        ),
        (Some(_), Some(_), None) => (
            Status::Ok,
            "skipped: claude is not on PATH to compare with".to_owned(),
        ),
        (Some(_), Some(bundled), Some(terminal)) if is_older(bundled, terminal) => (
            Status::Warn,
            format!(
                "the adapter runs Claude Code {bundled}, terminal has {terminal}; models added since {bundled} are missing from the picker"
            ),
        ),
        (Some(_), Some(bundled), Some(terminal)) => (
            Status::Ok,
            format!("the adapter runs Claude Code {bundled}, terminal has {terminal}"),
        ),
    };
    check("bundled Claude Code", status, detail, warn_hint(status, f))
}

/// Returns the reinstall command when the check warns, so the fix comes with the warning.
fn warn_hint(status: Status, f: &Freshness) -> Option<&str> {
    (status == Status::Warn).then_some(f.hint.as_str())
}

/// Returns the version that the npm registry lists as `latest` for `package`, or `None` when the
/// registry does not answer within [`REGISTRY_TIMEOUT`].
pub fn registry_latest(package: &str) -> Option<String> {
    let url = format!(
        "https://registry.npmjs.org/{}/latest",
        package.replace('/', "%2F")
    );
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(REGISTRY_TIMEOUT))
        .user_agent(format!("pons/{}", env!("CARGO_PKG_VERSION")))
        .build()
        .into();
    let text = agent
        .get(&url)
        .call()
        .ok()?
        .body_mut()
        .read_to_string()
        .ok()?;
    let json: Value = serde_json::from_str(&text).ok()?;
    json.get("version")?.as_str().map(str::to_owned)
}

/// Returns the Claude Code version that the adapter at `manifest` bundles, from the
/// `claudeCodeVersion` of its Claude Agent SDK. The SDK is looked up in the adapter's own
/// `node_modules` (npm nests it) and beside the adapter (pnpm links it there).
pub fn bundled_claude_code(
    manifest: &Path,
    read: &dyn Fn(&Path) -> Option<String>,
) -> Option<String> {
    let root = manifest.parent()?;
    let modules = [
        Some(root.join("node_modules")),
        root.parent().and_then(Path::parent).map(Path::to_path_buf),
    ];
    modules.into_iter().flatten().find_map(|dir| {
        let text = read(&dir.join(SDK).join("package.json"))?;
        let json: Value = serde_json::from_str(&text).ok()?;
        json.get("claudeCodeVersion")?.as_str().map(str::to_owned)
    })
}

/// Returns the version that `claude --version` prints, read from its first word such as
/// `2.1.311` in `2.1.311 (Claude Code)`, or `None` when that word is not a version.
pub fn leading_version(text: &str) -> Option<String> {
    let token = text.split_whitespace().next()?;
    semver(token).map(|_| token.to_owned())
}

/// Runs `claude --version` with the timeout of [`CLAUDE_TIMEOUT`] and returns its stdout.
pub fn claude_version_text(path: &Path) -> Option<String> {
    run_bounded(path, &["--version"], CLAUDE_TIMEOUT)
}

/// Runs `program` with `args` and returns its stdout when it exits 0 within `limit`. A process
/// that runs longer is killed and reported as `None`, so the doctor never waits on it.
pub fn run_bounded(program: &Path, args: &[&str], limit: Duration) -> Option<String> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let deadline = Instant::now() + limit;
    loop {
        if let Some(status) = child.try_wait().ok()? {
            if !status.success() {
                return None;
            }
            let mut out = String::new();
            child.stdout.take()?.read_to_string(&mut out).ok()?;
            return Some(out);
        }
        if Instant::now() >= deadline {
            // Killed so that a hung `claude` cannot outlive the doctor.
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }
        std::thread::sleep(POLL);
    }
}

/// The parts of a version that decide whether it is older: the `major.minor.patch` core, with
/// missing parts as 0, and whether a prerelease tag follows it.
#[derive(Debug, PartialEq, Eq)]
struct Semver {
    core: [u64; 3],
    prerelease: bool,
}

/// Parses `version` as semver core, ignoring build metadata after `+`. Two or three numeric
/// parts are accepted, so `2.1` and `2.1.0` are the same core; anything else is `None`.
fn semver(version: &str) -> Option<Semver> {
    let without_build = version.split('+').next()?;
    let (core_text, prerelease) = match without_build.split_once('-') {
        Some((core, _)) => (core, true),
        None => (without_build, false),
    };
    let parts = core_text
        .split('.')
        .map(|part| part.parse::<u64>().ok())
        .collect::<Option<Vec<u64>>>()?;
    let [major, minor, patch] = match parts.as_slice() {
        [a, b] => [*a, *b, 0],
        [a, b, c] => [*a, *b, *c],
        _ => return None,
    };
    Some(Semver {
        core: [major, minor, patch],
        prerelease,
    })
}

/// Whether the version `a` is older than `b`. Cores compare numerically, so `0.106.0` is newer
/// than `0.85.1`; with equal cores a prerelease is older than the release, and two prereleases
/// are not ordered. A version that does not parse is never older, so it never warns.
fn is_older(a: &str, b: &str) -> bool {
    let (Some(a), Some(b)) = (semver(a), semver(b)) else {
        return false;
    };
    match a.core.cmp(&b.core) {
        Ordering::Less => true,
        Ordering::Greater => false,
        Ordering::Equal => a.prerelease && !b.prerelease,
    }
}
