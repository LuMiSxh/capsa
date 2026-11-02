// SPDX-License-Identifier: MPL-2.0
//! `pons self setup`: reports the Claude ACP adapter, or installs it when it is missing.
//! The install goes through [`capsa_core::pkg`].

use std::path::PathBuf;

use anyhow::Result;
use capsa_core::hint::{Hinted, ResultExt};
use capsa_core::pkg::{self, Manager};
use capsa_core::render::{Check, Doc, Render, Status};
use capsa_core::which::{self, Env, Fs, RealFs};
use clap::ValueEnum;
use serde::Serialize;

use crate::launcher::PACKAGE;

/// The adapter's program name.
const ADAPTER: &str = "claude-agent-acp";

/// The package manager `pons self setup` installs with.
#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum ManagerArg {
    Pnpm,
    Bun,
    Npm,
}

impl From<ManagerArg> for Manager {
    fn from(arg: ManagerArg) -> Self {
        match arg {
            ManagerArg::Pnpm => Self::Pnpm,
            ManagerArg::Bun => Self::Bun,
            ManagerArg::Npm => Self::Npm,
        }
    }
}

/// What setup found or did. It serializes as the `--json` object, tagged by `status`.
///
/// A step keeps every key of the JSON object: the fields a step never fills are `null`.
#[derive(Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Step {
    /// The adapter is on the PATH or in a user bin directory; nothing ran.
    Present {
        path: PathBuf,
        version: Option<String>,
        command: Option<Vec<String>>,
    },
    /// A dry run: the adapter is missing and this is the command that would install it.
    WouldInstall {
        path: Option<PathBuf>,
        version: Option<String>,
        command: Vec<String>,
    },
    /// The install exited with 0. `path` is set when the adapter is now found;
    /// `bin_dir` is the global bin directory when it is not on the PATH.
    Installed {
        path: Option<PathBuf>,
        version: Option<String>,
        command: Vec<String>,
        bin_dir: Option<PathBuf>,
    },
}

/// Makes sure the adapter is installed, using the process environment.
pub fn run(preferred: Option<Manager>, dry_run: bool) -> Result<Step> {
    run_in(
        &Env::from_process(),
        &RealFs,
        &pkg::global_bin_dir,
        preferred,
        dry_run,
    )
}

/// Makes sure the adapter is installed, like [`run`], with the given search environment,
/// file system and global bin
/// directory lookup.
pub fn run_in(
    env: &Env,
    fs: &dyn Fs,
    global_bin: &dyn Fn(Manager) -> Option<PathBuf>,
    preferred: Option<Manager>,
    dry_run: bool,
) -> Result<Step> {
    if let Some(path) = which::find_in(env, fs, ADAPTER, None) {
        let version = pkg::package_version(&path, PACKAGE);
        return Ok(Step::Present {
            path,
            version,
            command: None,
        });
    }
    let manager = pkg::detect_in(env, fs, preferred)
        .ok_or(pkg::PkgError::NoManager)
        .hinted()?;
    let report = pkg::install_in(env, fs, manager, PACKAGE, dry_run).hinted()?;
    if dry_run {
        return Ok(Step::WouldInstall {
            path: None,
            version: None,
            command: report.command,
        });
    }
    if let Some(status) = report.status.filter(|s| !s.success()) {
        return Err(Hinted::new(
            format!("{} failed: {status}", report.command.join(" ")),
            "run `pons self setup --dry-run` to see the command, or pick another manager with --package-manager",
        )
        .into());
    }
    let (path, bin_dir) = locate_installed(env, fs, global_bin(manager));
    let version = path
        .as_deref()
        .and_then(|p| pkg::package_version(p, PACKAGE));
    Ok(Step::Installed {
        command: report.command,
        path,
        version,
        bin_dir,
    })
}

/// Finds the adapter after an install: on the PATH, else in the manager's global
/// bin directory. The second value is that directory when it is not on the PATH,
/// so the user can be told to add it.
fn locate_installed(
    env: &Env,
    fs: &dyn Fs,
    global: Option<PathBuf>,
) -> (Option<PathBuf>, Option<PathBuf>) {
    let path = which::find_in(env, fs, ADAPTER, None).or_else(|| {
        let mut in_dir = env.clone();
        in_dir.path = vec![global.clone()?];
        which::find_in(&in_dir, fs, ADAPTER, None)
    });
    let bin_dir = global.filter(|dir| !env.path.contains(dir));
    (path, bin_dir)
}

impl Step {
    /// The one check row this step shows.
    fn check(&self) -> Check {
        match self {
            Self::Present { path, version, .. } => Check {
                name: ADAPTER.into(),
                status: Status::Ok,
                detail: format!(
                    "present at {} (version {})",
                    path.display(),
                    version.as_deref().unwrap_or("unknown")
                ),
                hint: None,
            },
            Self::WouldInstall { command, .. } => Check {
                name: ADAPTER.into(),
                status: Status::Warn,
                detail: format!(
                    "not installed; dry run would run `{}`; nothing was installed",
                    command.join(" ")
                ),
                hint: None,
            },
            Self::Installed {
                path,
                version,
                command,
                bin_dir,
            } => {
                let hint = match (bin_dir, path) {
                    (Some(dir), _) => Some(format!("add {} to PATH", dir.display())),
                    (None, None) => {
                        Some("check the global bin directory of the package manager".to_owned())
                    }
                    (None, Some(_)) => None,
                };
                match path {
                    Some(path) => Check {
                        name: ADAPTER.into(),
                        status: Status::Ok,
                        detail: format!(
                            "installed at {} (version {})",
                            path.display(),
                            version.as_deref().unwrap_or("unknown")
                        ),
                        hint,
                    },
                    None => Check {
                        name: ADAPTER.into(),
                        status: Status::Warn,
                        detail: format!("ran `{}`; not on PATH yet", command.join(" ")),
                        hint,
                    },
                }
            }
        }
    }
}

impl Render for Step {
    fn doc(&self, doc: &mut Doc) {
        doc.title("pons self setup".to_owned());
        doc.check(&self.check());
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::path::Path;

    use capsa_core::format::Format;
    use capsa_core::hint::find_hint;
    use capsa_core::render::emit_to;
    use capsa_core::which::Platform;

    use super::*;

    #[derive(Default)]
    struct Files(HashSet<PathBuf>);

    impl Files {
        fn with(files: &[&str]) -> Self {
            Self(files.iter().map(PathBuf::from).collect())
        }
    }

    impl Fs for Files {
        fn is_executable(&self, path: &Path) -> bool {
            self.0.contains(path)
        }

        fn is_dir(&self, path: &Path) -> bool {
            self.0.iter().any(|f| f.starts_with(path))
        }

        fn subdirs(&self, _: &Path) -> Vec<PathBuf> {
            Vec::new()
        }
    }

    // The fake files use Unix paths, so the platform is fixed rather than the host's.
    fn env() -> Env {
        Env {
            platform: Platform::Linux,
            path: vec![PathBuf::from("/bin")],
            home: None,
            appdata: None,
            local_appdata: None,
            pathext: Vec::new(),
        }
    }

    fn nothing(_: Manager) -> Option<PathBuf> {
        None
    }

    /// The `--json` line of a step, as `pons self setup --json` prints it.
    fn json(step: &Step) -> String {
        let mut out = Vec::new();
        emit_to(step, Format::Json, false, &mut out).unwrap();
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn an_installed_adapter_in_the_global_bin_dir_is_found_and_that_dir_is_hinted() {
        let fs = Files::with(&["/g/bin/claude-agent-acp"]);
        let (path, bin_dir) = locate_installed(&env(), &fs, Some(PathBuf::from("/g/bin")));
        assert_eq!(path, Some(PathBuf::from("/g/bin/claude-agent-acp")));
        assert_eq!(bin_dir, Some(PathBuf::from("/g/bin")));
    }

    #[test]
    fn a_present_adapter_is_reported_and_nothing_runs() {
        let fs = Files::with(&["/bin/claude-agent-acp"]);
        let step = run_in(&env(), &fs, &nothing, None, false).unwrap();
        assert_eq!(
            step,
            Step::Present {
                path: PathBuf::from("/bin/claude-agent-acp"),
                version: None,
                command: None,
            }
        );
    }

    #[test]
    fn a_dry_run_names_the_first_manager_found() {
        let fs = Files::with(&["/bin/bun", "/bin/npm"]);
        let step = run_in(&env(), &fs, &nothing, None, true).unwrap();
        assert_eq!(
            step,
            Step::WouldInstall {
                path: None,
                version: None,
                command: ["bun", "add", "-g", PACKAGE].map(str::to_owned).to_vec(),
            }
        );
    }

    #[test]
    fn the_named_manager_wins_when_it_is_on_the_path() {
        let fs = Files::with(&["/bin/pnpm", "/bin/npm"]);
        let step = run_in(&env(), &fs, &nothing, Some(Manager::Npm), true).unwrap();
        assert_eq!(
            step,
            Step::WouldInstall {
                path: None,
                version: None,
                command: ["npm", "install", "-g", PACKAGE]
                    .map(str::to_owned)
                    .to_vec(),
            }
        );
    }

    #[test]
    fn no_manager_fails_with_a_hint_and_installs_nothing() {
        let err = run_in(&env(), &Files::default(), &nothing, None, true).unwrap_err();
        assert!(find_hint(&err).is_some_and(|hint| hint.contains("Node.js")));
    }

    #[test]
    fn the_views_show_the_step_in_each_format() {
        let step = Step::WouldInstall {
            path: None,
            version: None,
            command: vec!["pnpm".to_owned()],
        };
        assert!(json(&step).contains("\"status\":\"would_install\""));
        let mut agent = Vec::new();
        emit_to(&step, Format::Agent, false, &mut agent).unwrap();
        let agent = String::from_utf8(agent).unwrap();
        assert!(agent.contains("pons self setup"));
        assert!(agent.contains("nothing was installed"));
        assert!(!agent.contains('\x1b'));
    }

    #[test]
    fn json_keeps_the_keys_and_their_order_of_each_step() {
        let present = Step::Present {
            path: PathBuf::from("/bin/claude-agent-acp"),
            version: Some("0.85.1".to_owned()),
            command: None,
        };
        assert_eq!(
            json(&present),
            "{\"status\":\"present\",\"path\":\"/bin/claude-agent-acp\",\"version\":\"0.85.1\",\"command\":null}\n"
        );
        let would_install = Step::WouldInstall {
            path: None,
            version: None,
            command: ["bun", "add", "-g", "@agentclientprotocol/claude-agent-acp"]
                .map(str::to_owned)
                .to_vec(),
        };
        assert_eq!(
            json(&would_install),
            "{\"status\":\"would_install\",\"path\":null,\"version\":null,\"command\":[\"bun\",\"add\",\"-g\",\"@agentclientprotocol/claude-agent-acp\"]}\n"
        );
        let installed = Step::Installed {
            command: [
                "npm",
                "install",
                "-g",
                "@agentclientprotocol/claude-agent-acp",
            ]
            .map(str::to_owned)
            .to_vec(),
            path: Some(PathBuf::from("/g/bin/claude-agent-acp")),
            version: None,
            bin_dir: Some(PathBuf::from("/g/bin")),
        };
        assert_eq!(
            json(&installed),
            "{\"status\":\"installed\",\"path\":\"/g/bin/claude-agent-acp\",\"version\":null,\"command\":[\"npm\",\"install\",\"-g\",\"@agentclientprotocol/claude-agent-acp\"],\"bin_dir\":\"/g/bin\"}\n"
        );
    }
}
