// SPDX-License-Identifier: MPL-2.0
use std::collections::HashSet;
use std::path::{Path, PathBuf};

use super::*;

/// Executables by path; directories are implied by the files under them.
#[derive(Default)]
struct FakeFs {
    files: HashSet<PathBuf>,
}

impl FakeFs {
    fn with(files: &[&str]) -> Self {
        Self {
            files: files.iter().map(PathBuf::from).collect(),
        }
    }
}

impl Fs for FakeFs {
    fn is_executable(&self, path: &Path) -> bool {
        self.files.contains(path)
    }

    fn is_dir(&self, path: &Path) -> bool {
        self.files.iter().any(|f| f.starts_with(path))
    }

    fn subdirs(&self, _: &Path) -> Vec<PathBuf> {
        Vec::new()
    }
}

#[cfg(unix)]
const PKG: &str = "@agentclientprotocol/claude-agent-acp";

fn env(path: &[&str]) -> Env {
    Env {
        platform: crate::which::Platform::Linux,
        path: path.iter().map(PathBuf::from).collect(),
        home: Some(PathBuf::from("/home/u")),
        appdata: None,
        local_appdata: None,
        pathext: Vec::new(),
    }
}

fn unanswered(_: &str, _: &[&str]) -> Option<String> {
    None
}

#[test]
fn pnpm_bin_dir_is_asked_of_pnpm_first() {
    let run = |program: &str, args: &[&str]| {
        (program == "pnpm" && args == ["bin", "-g"]).then(|| "/x/pnpm/bin\n".to_owned())
    };
    let dir = global_bin_dir_in(
        &env(&[]),
        &FakeFs::default(),
        Manager::Pnpm,
        Some(Path::new("/h/pnpm")),
        &run,
    );
    assert_eq!(dir, Some(PathBuf::from("/x/pnpm/bin")));
}

#[test]
fn pnpm_falls_back_to_pnpm_home_bin_then_pnpm_home() {
    let home = Path::new("/h/pnpm");
    let dir = |files: &[&str]| {
        global_bin_dir_in(
            &env(&[]),
            &FakeFs::with(files),
            Manager::Pnpm,
            Some(home),
            &unanswered,
        )
    };
    assert_eq!(
        dir(&["/h/pnpm/bin/pnpm"]),
        Some(PathBuf::from("/h/pnpm/bin"))
    );
    assert_eq!(dir(&["/h/pnpm/pnpm"]), Some(PathBuf::from("/h/pnpm")));
    assert_eq!(dir(&[]), None);
}

#[test]
fn bun_falls_back_to_its_home_bin_directory() {
    let fs = FakeFs::with(&["/home/u/.bun/bin/bun"]);
    assert_eq!(
        global_bin_dir_in(&env(&[]), &fs, Manager::Bun, None, &unanswered),
        Some(PathBuf::from("/home/u/.bun/bin"))
    );
}

#[test]
fn npm_bin_dir_is_the_bin_under_its_prefix_and_the_prefix_on_windows() {
    let prefix = |_: &str, _: &[&str]| Some("/opt/homebrew\n".to_owned());
    let unix = global_bin_dir_in(&env(&[]), &FakeFs::default(), Manager::Npm, None, &prefix);
    assert_eq!(unix, Some(PathBuf::from("/opt/homebrew/bin")));

    let windows = Env {
        platform: crate::which::Platform::Windows,
        ..env(&[])
    };
    let prefix = |_: &str, _: &[&str]| Some("C:/npm\n".to_owned());
    let dir = global_bin_dir_in(&windows, &FakeFs::default(), Manager::Npm, None, &prefix);
    assert_eq!(dir, Some(PathBuf::from("C:/npm")));
}

#[test]
fn an_unanswering_npm_has_no_bin_dir() {
    let dir = global_bin_dir_in(
        &env(&[]),
        &FakeFs::default(),
        Manager::Npm,
        None,
        &unanswered,
    );
    assert_eq!(dir, None);
}

#[test]
fn install_commands_match_each_package_manager() {
    assert_eq!(
        install_command(Manager::Pnpm, "p"),
        ["pnpm", "add", "-g", "p"]
    );
    assert_eq!(
        install_command(Manager::Bun, "p"),
        ["bun", "add", "-g", "p"]
    );
    assert_eq!(
        install_command(Manager::Npm, "p"),
        ["npm", "install", "-g", "p"]
    );
}

#[test]
fn detection_prefers_the_named_manager_then_pnpm_bun_npm() {
    let e = env(&["/bin"]);
    let all = FakeFs::with(&["/bin/pnpm", "/bin/bun", "/bin/npm"]);
    assert_eq!(detect_in(&e, &all, None), Some(Manager::Pnpm));
    assert_eq!(detect_in(&e, &all, Some(Manager::Npm)), Some(Manager::Npm));
    let no_pnpm = FakeFs::with(&["/bin/bun", "/bin/npm"]);
    assert_eq!(detect_in(&e, &no_pnpm, None), Some(Manager::Bun));
    // The named manager is not on the PATH: the order applies.
    assert_eq!(
        detect_in(&e, &no_pnpm, Some(Manager::Pnpm)),
        Some(Manager::Bun)
    );
    assert_eq!(
        detect_in(&e, &FakeFs::with(&["/bin/npm"]), None),
        Some(Manager::Npm)
    );
    assert_eq!(detect_in(&e, &FakeFs::default(), None), None);
}

#[test]
fn detection_searches_the_user_bin_directories_too() {
    let e = env(&["/usr/bin"]);
    let fs = FakeFs::with(&["/home/u/.bun/bin/bun"]);
    assert_eq!(detect_in(&e, &fs, None), Some(Manager::Bun));
}

#[test]
fn a_dry_run_returns_the_command_and_runs_nothing() {
    let report = install_in(&env(&[]), &FakeFs::default(), Manager::Pnpm, "p", true).unwrap();
    assert_eq!(report.command, ["pnpm", "add", "-g", "p"]);
    assert!(report.status.is_none());
    assert!(report.success());
}

#[test]
fn a_manager_missing_from_the_search_path_is_not_run() {
    let err = install_in(
        &env(&["/nowhere"]),
        &FakeFs::default(),
        Manager::Npm,
        "p",
        false,
    )
    .unwrap_err();
    assert!(matches!(err, PkgError::Spawn { program: "npm", .. }));
    assert!(err.hint().is_some());
}

#[cfg(unix)]
fn fake_manager(dir: &Path, name: &str, exit: u8) {
    use std::os::unix::fs::PermissionsExt;

    let args_file = dir.join(format!("{name}.args"));
    let script = format!(
        "#!/bin/sh\necho \"$@\" > '{}'\nexit {exit}\n",
        args_file.display()
    );
    let path = dir.join(name);
    std::fs::write(&path, script).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

#[cfg(unix)]
#[test]
fn install_runs_the_manager_found_on_the_search_path() {
    let dir = tempfile::tempdir().unwrap();
    fake_manager(dir.path(), "pnpm", 0);
    fake_manager(dir.path(), "bun", 3);
    let e = Env {
        path: vec![dir.path().to_path_buf()],
        ..env(&[])
    };
    let report = install_in(&e, &RealFs, Manager::Pnpm, "pkg-x", false).unwrap();
    assert!(report.success());
    let args = std::fs::read_to_string(dir.path().join("pnpm.args")).unwrap();
    assert_eq!(args.trim(), "add -g pkg-x");

    let report = install_in(&e, &RealFs, Manager::Bun, "pkg-x", false).unwrap();
    assert!(!report.success());
    assert_eq!(report.status.and_then(|s| s.code()), Some(3));
}

#[cfg(unix)]
#[test]
fn package_version_reads_the_manifest_next_to_the_real_binary() {
    let root = tempfile::tempdir().unwrap();
    let pkg_dir = root
        .path()
        .join("lib/node_modules/@agentclientprotocol/claude-agent-acp");
    std::fs::create_dir_all(pkg_dir.join("dist")).unwrap();
    std::fs::write(
        pkg_dir.join("package.json"),
        format!(r#"{{"name":"{PKG}","version":"1.2.3"}}"#),
    )
    .unwrap();
    std::fs::write(pkg_dir.join("dist/index.js"), "").unwrap();
    let bin_dir = root.path().join("bin");
    std::fs::create_dir_all(&bin_dir).unwrap();
    let link = bin_dir.join("claude-agent-acp");
    std::os::unix::fs::symlink(pkg_dir.join("dist/index.js"), &link).unwrap();

    assert_eq!(package_version(&link, PKG).as_deref(), Some("1.2.3"));
    assert_eq!(package_version(&link, "other"), None);
    assert_eq!(package_version(&root.path().join("missing"), PKG), None);
}

#[cfg(unix)]
#[test]
fn package_version_follows_a_cmd_shim_target() {
    let root = tempfile::tempdir().unwrap();
    let pkg_dir = root
        .path()
        .join("global/node_modules/@agentclientprotocol/claude-agent-acp");
    std::fs::create_dir_all(pkg_dir.join("dist")).unwrap();
    std::fs::write(
        pkg_dir.join("package.json"),
        format!(r#"{{"name":"{PKG}","version":"0.9.0"}}"#),
    )
    .unwrap();
    let shim = root.path().join("bin-claude-agent-acp");
    let script = format!(
        "#!/bin/sh\nexec node '{}' \"$@\"\n# cmd-shim-target={}\n",
        pkg_dir.join("dist/index.js").display(),
        pkg_dir.join("dist/index.js").display()
    );
    std::fs::write(&shim, script).unwrap();
    assert_eq!(package_version(&shim, PKG).as_deref(), Some("0.9.0"));
}

#[test]
fn manifest_in_needs_the_named_package() {
    let read = |p: &Path| {
        (p == Path::new("/d/package.json")).then(|| r#"{"name":"p","version":"2.0.0"}"#.to_owned())
    };
    assert_eq!(
        manifest_in(Path::new("/d"), "p", &read),
        Some(("2.0.0".to_owned(), PathBuf::from("/d/package.json")))
    );
    assert_eq!(manifest_in(Path::new("/d"), "q", &read), None);
    assert_eq!(manifest_in(Path::new("/e"), "p", &read), None);
}
