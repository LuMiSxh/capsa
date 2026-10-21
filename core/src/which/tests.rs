// SPDX-License-Identifier: MPL-2.0
use std::collections::HashSet;
use std::ffi::{OsStr, OsString};
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

    fn subdirs(&self, path: &Path) -> Vec<PathBuf> {
        let mut out: Vec<PathBuf> = self
            .files
            .iter()
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

fn unix(path: &[&str]) -> Env {
    Env {
        platform: Platform::Macos,
        path: path.iter().map(PathBuf::from).collect(),
        home: Some(PathBuf::from("/home/u")),
        appdata: None,
        local_appdata: None,
        pathext: parse_pathext(DEFAULT_PATHEXT),
    }
}

fn windows(path: &[&str]) -> Env {
    Env {
        platform: Platform::Windows,
        path: path.iter().map(PathBuf::from).collect(),
        home: Some(PathBuf::from("C:/Users/u")),
        appdata: Some(PathBuf::from("C:/Users/u/AppData/Roaming")),
        local_appdata: Some(PathBuf::from("C:/Users/u/AppData/Local")),
        pathext: parse_pathext(DEFAULT_PATHEXT),
    }
}

/// The file `name` resolves to, with `\` folded to `/` so expectations hold on every host.
fn resolved(env: &Env, fs: &FakeFs, name: &str) -> Option<String> {
    find_in(env, fs, name, None).map(|p| p.display().to_string().replace('\\', "/"))
}

#[test]
fn path_entries_come_first_in_their_order() {
    let fs = FakeFs::with(&["/b/tool", "/a/tool"]);
    assert_eq!(
        resolved(&unix(&["/a", "/b"]), &fs, "tool").as_deref(),
        Some("/a/tool")
    );
    assert_eq!(
        resolved(&unix(&["/b", "/a"]), &fs, "tool").as_deref(),
        Some("/b/tool")
    );
}

#[test]
fn user_dirs_are_searched_after_path() {
    let env = unix(&["/usr/bin"]);
    let fs = FakeFs::with(&["/home/u/.cargo/bin/tool"]);
    assert_eq!(
        resolved(&env, &fs, "tool").as_deref(),
        Some("/home/u/.cargo/bin/tool")
    );
    let fs = FakeFs::with(&["/home/u/.cargo/bin/tool", "/usr/bin/tool"]);
    assert_eq!(
        resolved(&env, &fs, "tool").as_deref(),
        Some("/usr/bin/tool")
    );
}

#[test]
fn every_user_bin_directory_is_searched() {
    let env = unix(&["/usr/bin"]);
    for dir in [
        "/home/u/.local/bin",
        "/home/u/.cargo/bin",
        "/home/u/Library/pnpm",
        "/home/u/Library/pnpm/bin",
        "/home/u/.local/share/pnpm",
        "/home/u/.local/share/pnpm/bin",
        "/home/u/.bun/bin",
        "/opt/homebrew/bin",
        "/usr/local/bin",
        "/home/u/.volta/bin",
        "/home/u/.local/share/fnm/aliases/default/bin",
        "/home/u/.fnm/aliases/default/bin",
    ] {
        let fs = FakeFs::with(&[&format!("{dir}/tool")]);
        assert_eq!(resolved(&env, &fs, "tool"), Some(format!("{dir}/tool")));
    }
}

#[test]
fn newest_nvm_node_wins() {
    let fs = FakeFs::with(&[
        "/home/u/.nvm/versions/node/v9.0.0/bin/npx",
        "/home/u/.nvm/versions/node/v20.11.1/bin/npx",
        "/home/u/.nvm/versions/node/v18.0.0/bin/npx",
    ]);
    assert_eq!(
        resolved(&unix(&["/first"]), &fs, "npx").as_deref(),
        Some("/home/u/.nvm/versions/node/v20.11.1/bin/npx")
    );
}

#[test]
fn exclude_skips_that_directory_wherever_it_is() {
    let env = unix(&["/shim", "/real", "/home/u/.local/bin"]);
    let fs = FakeFs::with(&["/shim/claude", "/real/claude"]);
    assert_eq!(
        find_in(&env, &fs, "claude", Some(Path::new("/shim"))),
        Some(PathBuf::from("/real/claude"))
    );
    let fs = FakeFs::with(&["/home/u/.local/bin/claude"]);
    assert_eq!(
        find_in(&env, &fs, "claude", Some(Path::new("/home/u/.local/bin"))),
        None
    );
    assert_eq!(
        find_in(&env, &fs, "claude", None),
        Some(PathBuf::from("/home/u/.local/bin/claude"))
    );
}

#[test]
fn windows_pathext_picks_runnable_shims() {
    let env = windows(&["C:/node"]);
    let fs = FakeFs::with(&["C:/node/npx", "C:/node/npx.cmd"]);
    assert_eq!(
        resolved(&env, &fs, "npx").as_deref(),
        Some("C:/node/npx.cmd")
    );
    // An extensionless script cannot be run on Windows.
    assert_eq!(resolved(&env, &FakeFs::with(&["C:/node/npx"]), "npx"), None);
    // PATHEXT order: exe before cmd.
    let fs = FakeFs::with(&["C:/node/bunx.cmd", "C:/node/bunx.exe"]);
    assert_eq!(
        resolved(&env, &fs, "bunx").as_deref(),
        Some("C:/node/bunx.exe")
    );
}

#[test]
fn windows_name_with_an_extension_is_used_as_given() {
    let fs = FakeFs::with(&["C:/tools/agent.exe"]);
    assert_eq!(
        resolved(&windows(&["C:/tools"]), &fs, "agent.exe").as_deref(),
        Some("C:/tools/agent.exe")
    );
}

#[test]
fn windows_user_dirs_come_from_appdata_and_local_appdata() {
    let env = windows(&["C:/other"]);
    for dir in [
        "C:/Users/u/AppData/Roaming/npm",
        "C:/Users/u/AppData/Local/pnpm",
        "C:/Users/u/.bun/bin",
    ] {
        let fs = FakeFs::with(&[&format!("{dir}/bunx.exe")]);
        assert_eq!(resolved(&env, &fs, "bunx"), Some(format!("{dir}/bunx.exe")));
    }
    // Unix-only directories are not searched on Windows.
    let fs = FakeFs::with(&["/usr/local/bin/bunx.exe"]);
    assert_eq!(resolved(&env, &fs, "bunx"), None);
}

#[test]
fn pathext_is_lowercased_and_junk_is_dropped() {
    assert_eq!(
        parse_pathext(".COM;.EXE; .Cmd;;x"),
        vec![".com", ".exe", ".cmd"]
    );
}

#[test]
fn search_dirs_lists_path_then_user_dirs_without_duplicates_or_empties() {
    let env = unix(&["/usr/local/bin", "", "/x"]);
    let fs = FakeFs::with(&["/home/u/.cargo/bin/tool"]);
    let dirs = search_dirs(&env, &fs);
    assert_eq!(dirs[0], PathBuf::from("/usr/local/bin"));
    assert_eq!(dirs[1], PathBuf::from("/x"));
    assert!(dirs.contains(&PathBuf::from("/home/u/.cargo/bin")));
    assert_eq!(
        dirs.iter()
            .filter(|d| d.as_path() == Path::new("/usr/local/bin"))
            .count(),
        1
    );
    assert!(dirs.iter().all(|d| !d.as_os_str().is_empty()));
}

#[test]
fn child_path_puts_the_program_dir_first_then_existing_user_dirs_once() {
    let env = unix(&["/usr/bin"]);
    let fs = FakeFs::with(&["/opt/homebrew/bin/npx", "/home/u/.cargo/bin/x"]);
    let dirs = child_path(&env, &fs, Path::new("/opt/homebrew/bin/npx"));
    assert_eq!(
        dirs,
        [
            PathBuf::from("/opt/homebrew/bin"),
            PathBuf::from("/usr/bin"),
            PathBuf::from("/home/u/.cargo/bin"),
        ]
    );
}

#[cfg(unix)]
#[test]
fn find_on_path_reports_the_index_and_needs_an_executable_file() {
    use std::os::unix::fs::PermissionsExt;

    let root = tempfile::tempdir().unwrap();
    let (a, b) = (root.path().join("a"), root.path().join("b"));
    std::fs::create_dir_all(&a).unwrap();
    std::fs::create_dir_all(&b).unwrap();
    let write = |file: PathBuf, mode: u32| {
        std::fs::write(&file, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(mode)).unwrap();
    };
    write(a.join("tool"), 0o644);
    write(b.join("tool"), 0o755);
    let path = std::env::join_paths([&a, &b]).unwrap();
    assert_eq!(find_on_path("tool", &path, None), Some((1, b.join("tool"))));

    write(a.join("tool"), 0o755);
    assert_eq!(find_on_path("tool", &path, None), Some((0, a.join("tool"))));
    assert_eq!(
        find_on_path("tool", &path, Some(&a)),
        Some((1, b.join("tool")))
    );
    assert_eq!(find_on_path("missing", &path, None), None);
    assert_eq!(path_index(&path, &b), Some(1));
}

#[cfg(unix)]
#[test]
fn find_on_path_skips_empty_and_relative_entries() {
    use std::os::unix::fs::PermissionsExt;

    let root = tempfile::tempdir().unwrap();
    let abs = root.path().join("abs");
    std::fs::create_dir_all(&abs).unwrap();
    let tool = abs.join("tool");
    std::fs::write(&tool, "#!/bin/sh\n").unwrap();
    std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755)).unwrap();
    // The same directory spelled relative to the process cwd would resolve to
    // an executable, so only the absoluteness check can reject it.
    let up = std::env::current_dir().unwrap().components().count() - 1;
    let relative = format!(
        "{}{}",
        "../".repeat(up),
        abs.strip_prefix("/").unwrap().display()
    );
    assert!(Path::new(&relative).join("tool").exists());

    let path = OsString::from(format!(":{relative}:{}", abs.display()));
    assert_eq!(find_on_path("tool", &path, None), Some((2, tool)));
    assert!(find_on_path("tool", OsStr::new(&format!(":{relative}")), None).is_none());
}
