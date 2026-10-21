// SPDX-License-Identifier: MPL-2.0

use std::collections::HashSet;

use capsa_core::which::parse_pathext;

use super::*;

#[derive(Default)]
struct FakeFs {
    files: HashSet<PathBuf>,
    dirs: HashSet<PathBuf>,
}

impl FakeFs {
    fn with(files: &[&str]) -> Self {
        Self {
            files: files.iter().map(PathBuf::from).collect(),
            dirs: HashSet::new(),
        }
    }
}

impl Fs for FakeFs {
    fn is_executable(&self, path: &Path) -> bool {
        self.files.contains(path)
    }

    fn is_dir(&self, path: &Path) -> bool {
        self.dirs.contains(path) || self.files.iter().any(|f| f.starts_with(path))
    }

    fn subdirs(&self, path: &Path) -> Vec<PathBuf> {
        let mut out: Vec<PathBuf> = self
            .files
            .iter()
            .chain(self.dirs.iter())
            .filter_map(|f| {
                let rest = f.strip_prefix(path).ok()?;
                Some(path.join(rest.components().next()?))
            })
            .collect();
        out.sort();
        out.dedup();
        out
    }
}

fn unix(platform: Platform, path: &[&str]) -> Env {
    Env {
        platform,
        path: path.iter().map(PathBuf::from).collect(),
        home: Some(PathBuf::from("/home/u")),
        appdata: None,
        local_appdata: None,
        pathext: parse_pathext(".COM;.EXE;.BAT;.CMD"),
    }
}

fn windows(path: &[&str]) -> Env {
    Env {
        platform: Platform::Windows,
        path: path.iter().map(PathBuf::from).collect(),
        home: Some(PathBuf::from("C:/Users/u")),
        appdata: Some(PathBuf::from("C:/Users/u/AppData/Roaming")),
        local_appdata: Some(PathBuf::from("C:/Users/u/AppData/Local")),
        pathext: parse_pathext(".COM;.EXE;.BAT;.CMD"),
    }
}

/// Returns the program path with `\` folded to `/`, so expectations written with
/// `/` hold on every host.
fn picked(env: &Env, fs: &FakeFs) -> Option<(String, Vec<String>)> {
    plan(env, fs, &[]).ok().map(|s| {
        let program = Path::new(&s.program).display().to_string();
        (program.replace('\\', "/"), s.args)
    })
}

#[test]
fn prefers_the_adapter_binary_over_every_runner() {
    let fs = FakeFs::with(&["/bin/claude-agent-acp", "/bin/npx", "/bin/bunx"]);
    let (prog, args) = picked(&unix(Platform::Macos, &["/bin"]), &fs).unwrap();
    assert_eq!(prog, "/bin/claude-agent-acp");
    assert!(args.is_empty());
}

#[test]
fn falls_back_npx_then_pnpm_then_pnpx_then_bunx() {
    let env = unix(Platform::Linux, &["/bin"]);
    let all = ["/bin/npx", "/bin/pnpm", "/bin/pnpx", "/bin/bunx"];
    let expect = [
        ("/bin/npx", vec!["-y", PACKAGE]),
        ("/bin/pnpm", vec!["dlx", PACKAGE]),
        ("/bin/pnpx", vec![PACKAGE]),
        ("/bin/bunx", vec![PACKAGE]),
    ];
    for (skip, (prog, args)) in expect.iter().enumerate() {
        let fs = FakeFs::with(&all[skip..]);
        let (p, a) = picked(&env, &fs).unwrap();
        assert_eq!(&p, prog);
        assert_eq!(&a, args);
    }
    assert!(plan(&env, &FakeFs::default(), &[]).is_err());
}

#[test]
fn finds_tools_in_user_dirs_missing_from_path() {
    let fs = FakeFs::with(&["/opt/homebrew/bin/npx"]);
    let env = unix(Platform::Macos, &["/usr/bin"]);
    assert_eq!(picked(&env, &fs).unwrap().0, "/opt/homebrew/bin/npx");
    let fs = FakeFs::with(&["/home/u/.local/bin/claude-agent-acp"]);
    assert_eq!(
        picked(&env, &fs).unwrap().0,
        "/home/u/.local/bin/claude-agent-acp"
    );
    for dir in [
        "/home/u/.cargo/bin",
        "/home/u/Library/pnpm",
        "/home/u/Library/pnpm/bin",
        "/home/u/.local/share/pnpm",
        "/home/u/.local/share/pnpm/bin",
        "/home/u/.bun/bin",
        "/usr/local/bin",
        "/home/u/.volta/bin",
        "/home/u/.local/share/fnm/aliases/default/bin",
    ] {
        let fs = FakeFs::with(&[&format!("{dir}/bunx")]);
        assert_eq!(picked(&env, &fs).unwrap().0, format!("{dir}/bunx"));
    }
}

#[test]
fn path_wins_over_user_dirs_and_newest_nvm_node_wins() {
    let env = unix(Platform::Linux, &["/first"]);
    let fs = FakeFs::with(&["/first/npx", "/usr/local/bin/npx"]);
    assert_eq!(picked(&env, &fs).unwrap().0, "/first/npx");
    let fs = FakeFs::with(&[
        "/home/u/.nvm/versions/node/v9.0.0/bin/npx",
        "/home/u/.nvm/versions/node/v20.11.1/bin/npx",
        "/home/u/.nvm/versions/node/v18.0.0/bin/npx",
    ]);
    assert_eq!(
        picked(&env, &fs).unwrap().0,
        "/home/u/.nvm/versions/node/v20.11.1/bin/npx"
    );
}

#[test]
fn windows_resolves_pathext_shims_and_ignores_extensionless_scripts() {
    let env = windows(&["C:/node"]);
    let fs = FakeFs::with(&["C:/node/npx", "C:/node/npx.cmd"]);
    let (prog, args) = picked(&env, &fs).unwrap();
    assert_eq!(prog, "C:/node/npx.cmd");
    assert_eq!(args, vec!["-y", PACKAGE]);
    // Only the extensionless script: not runnable.
    assert!(picked(&env, &FakeFs::with(&["C:/node/npx"])).is_none());
    // exe beats cmd by PATHEXT order.
    let fs = FakeFs::with(&["C:/node/bunx.cmd", "C:/node/bunx.exe"]);
    assert_eq!(picked(&env, &fs).unwrap().0, "C:/node/bunx.exe");
}

#[test]
fn windows_user_dirs() {
    let env = windows(&["C:/other"]);
    for dir in [
        "C:/Users/u/AppData/Roaming/npm",
        "C:/Users/u/AppData/Local/pnpm",
        "C:/Users/u/.bun/bin",
    ] {
        let fs = FakeFs::with(&[&format!("{dir}/bunx.exe")]);
        assert_eq!(picked(&env, &fs).unwrap().0, format!("{dir}/bunx.exe"));
    }
    // Unix-only directories are not searched there.
    let fs = FakeFs::with(&["/usr/local/bin/bunx.exe"]);
    assert!(picked(&env, &fs).is_none());
}

#[test]
fn an_explicit_command_always_wins() {
    let env = unix(Platform::Macos, &["/bin"]);
    let fs = FakeFs::with(&["/bin/claude-agent-acp", "/bin/mine"]);
    let cmd = vec!["mine".to_owned(), "--x".to_owned()];
    let s = plan(&env, &fs, &cmd).unwrap();
    assert_eq!(Path::new(&s.program), Path::new("/bin/mine"));
    assert_eq!(s.args, vec!["--x"]);
    // Unknown bare names and absolute paths are left to the OS.
    let s = plan(&env, &fs, &["nope".to_owned()]).unwrap();
    assert_eq!(s.program, OsString::from("nope"));
    assert!(s.path_dirs.is_none());
    let s = plan(&env, &fs, &["/abs/agent".to_owned()]).unwrap();
    assert_eq!(s.program, OsString::from("/abs/agent"));
}

#[test]
fn the_child_path_gets_the_program_dir_and_existing_user_dirs() {
    let env = unix(Platform::Macos, &["/usr/bin"]);
    let fs = FakeFs::with(&["/opt/homebrew/bin/npx", "/home/u/.cargo/bin/x"]);
    let s = plan(&env, &fs, &[]).unwrap();
    let dirs = s.path_dirs.unwrap();
    assert_eq!(dirs[0], PathBuf::from("/opt/homebrew/bin"));
    assert_eq!(dirs[1], PathBuf::from("/usr/bin"));
    assert!(dirs.contains(&PathBuf::from("/home/u/.cargo/bin")));
    assert!(!dirs.contains(&PathBuf::from("/home/u/.bun/bin")));
    assert_eq!(
        dirs.iter()
            .filter(|d| d.as_path() == Path::new("/opt/homebrew/bin"))
            .count(),
        1
    );
}

#[test]
fn batch_shims_refuse_shell_metacharacters_in_arguments() {
    let env = windows(&["C:/node"]);
    let fs = FakeFs::with(&["C:/node/npx.cmd"]);
    let ok = vec!["npx.cmd".to_owned(), "-y".to_owned(), "pkg@1.2".to_owned()];
    assert!(plan(&env, &fs, &ok).is_ok());
    for bad in ["a&calc", "a|b", "%PATH%", "\"x", "a^b", "a>b", "a\nb", "!x"] {
        let cmd = vec!["npx.cmd".to_owned(), bad.to_owned()];
        assert!(plan(&env, &fs, &cmd).is_err(), "{bad:?} must be refused");
    }
    // Real executables take any argument.
    let fs = FakeFs::with(&["C:/node/agent.exe"]);
    let cmd = vec!["agent.exe".to_owned(), "a&b".to_owned()];
    assert!(plan(&env, &fs, &cmd).is_ok());
    assert!(is_batch(Path::new("X.CMD")) && is_batch(Path::new("a.bat")));
    assert!(!is_batch(Path::new("a.exe")));
}
