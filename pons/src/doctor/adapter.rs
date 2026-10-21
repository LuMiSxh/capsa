// SPDX-License-Identifier: MPL-2.0
//! Checks the Claude ACP adapter for `pons self doctor`.

use super::*;

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
    match pkg::detect_in(env, fs, None) {
        Some(manager) => {
            let command = pkg::install_command(manager, PACKAGE).join(" ");
            match sys
                .global_bin_dir(manager)
                .filter(|dir| !env.path.contains(dir))
            {
                Some(dir) => format!("{command}, then put {} on PATH", dir.display()),
                None => command,
            }
        }
        None => format!("install Node.js, then run: npm install -g {PACKAGE}"),
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
