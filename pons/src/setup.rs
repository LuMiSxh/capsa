// SPDX-License-Identifier: MPL-2.0
//! `pons self setup`: reports the Claude ACP adapter, or installs it when it is missing.
//! The install goes through [`capsa_core::pkg`].

use std::fmt::Write as _;
use std::path::PathBuf;

use anyhow::{Result, bail};
use capsa_core::format::{Format, color_stdout};
use capsa_core::hint::ResultExt;
use capsa_core::pkg::{self, Manager};
use capsa_core::which::{self, Env, Fs, RealFs};
use clap::ValueEnum;
use serde_json::json;

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

/// What setup found or did.
#[derive(Debug, PartialEq, Eq)]
pub enum Step {
    /// The adapter is on the PATH or in a user bin directory; nothing ran.
    Present {
        path: PathBuf,
        version: Option<String>,
    },
    /// A dry run: the adapter is missing and this is the command that would install it.
    WouldInstall { command: Vec<String> },
    /// The install exited with 0. `path` is set when the adapter is now found;
    /// `bin_dir` is the global bin directory when it is not on the PATH.
    Installed {
        command: Vec<String>,
        path: Option<PathBuf>,
        version: Option<String>,
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
        return Ok(Step::Present { path, version });
    }
    let manager = pkg::detect_in(env, fs, preferred)
        .ok_or(pkg::PkgError::NoManager)
        .hinted()?;
    let report = pkg::install_in(env, fs, manager, PACKAGE, dry_run).hinted()?;
    if dry_run {
        return Ok(Step::WouldInstall {
            command: report.command,
        });
    }
    if let Some(status) = report.status.filter(|s| !s.success()) {
        bail!("{} failed: {status}", report.command.join(" "));
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

/// Renders the step as one document: JSON, coloured text for a person on a terminal,
/// or plain text.
pub fn render(step: &Step, format: Format) -> String {
    match format {
        Format::Json => render_json(step),
        Format::Human => render_text(step, color_stdout(format)),
        Format::Agent => render_text(step, false),
    }
}

fn render_text(step: &Step, colour: bool) -> String {
    let bold = |word: &str| {
        if colour {
            format!("\x1b[1m{word}\x1b[0m")
        } else {
            word.to_owned()
        }
    };
    let mut out = String::new();
    match step {
        Step::Present { path, version } => {
            let version = version.as_deref().unwrap_or("unknown");
            let _ = writeln!(
                out,
                "{}: {ADAPTER} at {} (version {version})",
                bold("present"),
                path.display()
            );
        }
        Step::WouldInstall { command } => {
            let _ = writeln!(
                out,
                "{}: {ADAPTER} is not installed; would run: {}",
                bold("dry run"),
                command.join(" ")
            );
            let _ = writeln!(out, "nothing was installed");
        }
        Step::Installed {
            command,
            path,
            version,
            bin_dir,
        } => {
            let _ = writeln!(out, "{}: ran {}", bold("installed"), command.join(" "));
            match path {
                Some(path) => {
                    let version = version.as_deref().unwrap_or("unknown");
                    let _ = writeln!(out, "{ADAPTER} at {} (version {version})", path.display());
                }
                None => {
                    let _ = writeln!(
                        out,
                        "{ADAPTER} is not on PATH yet; check the global bin directory of the package manager"
                    );
                }
            }
            if let Some(dir) = bin_dir {
                let _ = writeln!(out, "    hint: add {} to PATH", dir.display());
            }
        }
    }
    out
}

fn render_json(step: &Step) -> String {
    let value = match step {
        Step::Present { path, version } => json!({
            "status": "present",
            "path": path.display().to_string(),
            "version": version,
            "command": null,
        }),
        Step::WouldInstall { command } => json!({
            "status": "would_install",
            "path": null,
            "version": null,
            "command": command,
        }),
        Step::Installed {
            command,
            path,
            version,
            bin_dir,
        } => json!({
            "status": "installed",
            "path": path.as_ref().map(|p| p.display().to_string()),
            "version": version,
            "command": command,
            "bin_dir": bin_dir.as_ref().map(|p| p.display().to_string()),
        }),
    };
    format!("{value}\n")
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::path::Path;

    use capsa_core::hint::find_hint;
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
    fn the_renderers_show_the_status_in_each_format() {
        let step = Step::WouldInstall {
            command: vec!["pnpm".to_owned()],
        };
        assert!(render(&step, Format::Json).contains("\"status\":\"would_install\""));
        assert!(render(&step, Format::Agent).starts_with("dry run: claude-agent-acp"));
        assert!(!render(&step, Format::Agent).contains('\x1b'));
    }
}
