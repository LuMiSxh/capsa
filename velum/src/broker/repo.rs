// SPDX-License-Identifier: MPL-2.0
//! Owns the repository the broker may act on: the one around the working directory at the
//! start of the run, whose git configuration and hooks the sandbox cannot write.

use super::request::Refusal;
use crate::writable;
use serde_json::Value;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

/// The most bytes read from a `.git` file or `commondir`.
const MAX_POINTER: u64 = 4096;

/// A repository as it was when the run started. Git runs against these paths, never against
/// what the sandbox may have changed since, and approvals are keyed by `root`.
#[derive(Debug, Clone)]
pub struct Recorded {
    pub root: PathBuf,
    git_dir: PathBuf,
    common_dir: PathBuf,
    ids: [(u64, u64); 2],
    watched: Vec<Watched>,
}

/// The most bytes read from a watched file.
const MAX_WATCHED: u64 = 1 << 20;

/// How deep `include.path` chains are followed.
const MAX_INCLUDE_DEPTH: usize = 4;

/// A file git reads for configuration or for where the common directory is, as it was at the
/// start: the sandbox can create or change these in a plain `.git` directory.
#[derive(Debug, Clone)]
struct Watched {
    path: PathBuf,
    state: Option<(u64, u64, Vec<u8>)>,
}

/// Returns the device, inode and content of a regular file, `None` when it does not exist.
fn snapshot(path: &Path) -> Result<Option<(u64, u64, Vec<u8>)>, String> {
    use std::io::Read;
    let meta = match path.symlink_metadata() {
        Ok(meta) => meta,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(format!("{}: {err}", path.display())),
    };
    if !meta.is_file() || meta.len() > MAX_WATCHED {
        return Err(format!(
            "{} is not a regular file of a sane size",
            path.display()
        ));
    }
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .and_then(|file| file.take(MAX_WATCHED).read_to_end(&mut bytes))
        .map_err(|err| format!("{}: {err}", path.display()))?;
    Ok(Some((meta.dev(), meta.ino(), bytes)))
}

/// Resolves an `include.path` value the way git does: `~/` is the home directory, a relative
/// path starts at the directory of the including file.
fn include_target(value: &str, including: &Path) -> Result<PathBuf, String> {
    if let Some(rest) = value.strip_prefix("~/") {
        let home = std::env::var_os("HOME").ok_or("HOME is not set")?;
        return Ok(Path::new(&home).join(rest));
    }
    Ok(including.parent().unwrap_or(Path::new("/")).join(value))
}

/// Refuses a config file whose `include.path` or `includeIf.*.path` leads, directly or through
/// further includes, to a location the sandbox can write.
fn check_includes(config: &Path, settings: &Value, depth: usize) -> Result<(), String> {
    if depth > MAX_INCLUDE_DEPTH {
        return Err("config includes nest too deeply".to_string());
    }
    let out = std::process::Command::new("git")
        .args(["config", "-z", "--file"])
        .arg(config)
        .args(["--get-regexp", r"^(include|includeif\..*)\.path$"])
        .output()
        .map_err(|err| format!("running git: {err}"))?;
    match out.status.code() {
        Some(1) => return Ok(()),
        Some(0) => {}
        _ => return Err(format!("git cannot read {}", config.display())),
    }
    for record in String::from_utf8_lossy(&out.stdout).split('\0') {
        let Some((_, value)) = record.split_once('\n') else {
            continue;
        };
        let target = include_target(value, config)?;
        if let Some(entry) = writable::writable_by(settings, &target) {
            return Err(format!(
                "{} includes {}, which is writable inside the sandbox (allowed by {entry})",
                config.display(),
                target.display()
            ));
        }
        if target.is_file() {
            check_includes(&target, settings, depth + 1)?;
        }
    }
    Ok(())
}

fn identity(path: &Path) -> Option<(u64, u64)> {
    let meta = std::fs::metadata(path).ok()?;
    Some((meta.dev(), meta.ino()))
}

fn read_pointer(path: &Path) -> Result<String, String> {
    use std::io::Read;
    let file = std::fs::File::open(path).map_err(|err| format!("{}: {err}", path.display()))?;
    let mut text = String::new();
    file.take(MAX_POINTER)
        .read_to_string(&mut text)
        .map_err(|err| format!("{}: {err}", path.display()))?;
    Ok(text.trim().to_string())
}

/// Finds the git directory behind `dot_git`, a directory or a `gitdir:` file. A link is refused.
fn git_dir_of(root: &Path, dot_git: &Path) -> Result<PathBuf, String> {
    let meta = dot_git
        .symlink_metadata()
        .map_err(|err| format!("{}: {err}", dot_git.display()))?;
    if meta.is_dir() {
        return std::fs::canonicalize(dot_git).map_err(|err| err.to_string());
    }
    if !meta.is_file() {
        return Err(format!(
            "{} is not a plain file or directory",
            dot_git.display()
        ));
    }
    let text = read_pointer(dot_git)?;
    let target = text
        .strip_prefix("gitdir:")
        .map(str::trim)
        .ok_or_else(|| format!("{} is not a gitdir file", dot_git.display()))?;
    std::fs::canonicalize(root.join(target)).map_err(|err| format!("{target}: {err}"))
}

impl Recorded {
    /// Records the repository around `cwd`. Fails, with the reason, when there is none or when
    /// the sandbox can write its `.git` entry, `config` or `hooks` according to `settings`.
    pub fn resolve(cwd: &Path, settings: &Value) -> Result<Self, String> {
        let cwd = std::fs::canonicalize(cwd).map_err(|err| format!("working directory: {err}"))?;
        let (root, dot_git) = cwd
            .ancestors()
            .map(|dir| (dir, dir.join(".git")))
            .find(|(_, dot_git)| dot_git.symlink_metadata().is_ok())
            .ok_or("the working directory is not inside a git repository")?;
        let git_dir = git_dir_of(root, &dot_git)?;
        let common_dir = match read_pointer(&git_dir.join("commondir")) {
            Ok(text) => std::fs::canonicalize(git_dir.join(text)).map_err(|err| err.to_string())?,
            Err(_) => git_dir.clone(),
        };
        // Git reads `config` and `hooks` from the common directory. A `.git` file, which
        // names the git directory, and the directory of a linked worktree (HEAD,
        // `config.worktree`) are guarded too. A `.git` directory is not: velum denies only
        // its `config` and `hooks`, and `verify` catches a replacement.
        let mut guarded = vec![common_dir.join("config"), common_dir.join("hooks")];
        if dot_git.symlink_metadata().is_ok_and(|meta| meta.is_file()) {
            guarded.push(dot_git);
            guarded.push(git_dir.clone());
        }
        for path in &guarded {
            if let Some(entry) = writable::writable_by(settings, path) {
                return Err(format!(
                    "{} is writable inside the sandbox (allowed by {entry}), so git would run \
                     whatever the agent put there",
                    path.display()
                ));
            }
        }
        check_includes(&common_dir.join("config"), settings, 0)?;
        let mut watched = Vec::new();
        for path in [
            git_dir.join("commondir"),
            git_dir.join("config.worktree"),
            common_dir.join("config.worktree"),
            common_dir.join("config"),
        ] {
            if !watched.iter().any(|seen: &Watched| seen.path == path) {
                watched.push(Watched {
                    state: snapshot(&path)?,
                    path,
                });
            }
        }
        let ids = [identity(&git_dir), identity(&common_dir)];
        let (Some(first), Some(second)) = (ids[0], ids[1]) else {
            return Err("the git directory cannot be inspected".to_string());
        };
        Ok(Self {
            root: root.to_path_buf(),
            git_dir,
            common_dir,
            ids: [first, second],
            watched,
        })
    }

    /// Returns the git directory.
    pub fn git_dir(&self) -> &Path {
        &self.git_dir
    }

    /// Returns the common directory.
    pub fn common_dir(&self) -> &Path {
        &self.common_dir
    }

    /// Checks that the git directories are still the ones recorded and that no file that
    /// redirects git or adds configuration (`commondir`, `config.worktree`, `config`) has
    /// appeared or changed.
    pub fn verify(&self) -> Result<(), Refusal> {
        let same = |path: &Path, id: (u64, u64)| {
            std::fs::canonicalize(path).is_ok_and(|real| real == path) && identity(path) == Some(id)
        };
        let unchanged = self
            .watched
            .iter()
            .all(|file| snapshot(&file.path).is_ok_and(|now| now == file.state));
        if unchanged && same(&self.git_dir, self.ids[0]) && same(&self.common_dir, self.ids[1]) {
            Ok(())
        } else {
            Err(Refusal::RepoChanged)
        }
    }

    /// Builds a git command line that works on this repository only: it names the git
    /// directory and work tree, turns off hooks and the file-system monitor, and ignores
    /// `core.worktree`.
    pub fn git(&self, args: &[&str]) -> Vec<String> {
        let mut argv = vec![
            "git".to_string(),
            format!("--git-dir={}", self.git_dir.display()),
            format!("--work-tree={}", self.root.display()),
            "-c".to_string(),
            "core.hooksPath=/dev/null".to_string(),
            "-c".to_string(),
            "core.fsmonitor=false".to_string(),
        ];
        argv.extend(args.iter().map(|arg| (*arg).to_string()));
        argv
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scratch::Scratch;
    use serde_json::json;

    fn settings(allow: &Path, deny: &[&Path]) -> Value {
        json!({"filesystem": {"allowWrite": [allow], "denyWrite": deny}})
    }

    fn make_repo(root: &Path) {
        std::fs::create_dir_all(root.join(".git/hooks")).unwrap();
        std::fs::write(root.join(".git/config"), "").unwrap();
    }

    #[test]
    fn records_a_repository_whose_config_and_hooks_are_denied() {
        let scratch = Scratch::new("repo-ok");
        let work = scratch.path().join("work");
        make_repo(&work);
        let deny = [
            work.join(".git"),
            work.join(".git/config"),
            work.join(".git/hooks"),
        ];
        let deny: Vec<&Path> = deny.iter().map(PathBuf::as_path).collect();
        let recorded = Recorded::resolve(&work, &settings(&work, &deny)).unwrap();
        assert_eq!(recorded.root, std::fs::canonicalize(&work).unwrap());
        assert!(recorded.verify().is_ok());
        let argv = recorded.git(&["push"]);
        assert!(argv.iter().any(|a| a == "core.hooksPath=/dev/null"));
        assert!(argv.iter().any(|a| a.starts_with("--work-tree=")));
    }

    #[test]
    fn refuses_a_repository_whose_config_or_hooks_the_sandbox_can_write() {
        let scratch = Scratch::new("repo-writable");
        let work = scratch.path().join("work");
        make_repo(&work);
        let err = Recorded::resolve(&work, &settings(&work, &[])).unwrap_err();
        assert!(err.contains("writable inside the sandbox"), "{err}");
    }

    #[test]
    fn refuses_a_directory_without_a_repository() {
        let scratch = Scratch::new("repo-none");
        let work = scratch.path().join("work");
        std::fs::create_dir_all(&work).unwrap();
        assert!(Recorded::resolve(&work, &settings(&work, &[])).is_err());
    }

    #[test]
    fn follows_a_worktree_file_to_its_git_and_common_directories() {
        let scratch = Scratch::new("repo-worktree");
        let main = scratch.path().join("main");
        make_repo(&main);
        let admin = main.join(".git/worktrees/w");
        std::fs::create_dir_all(&admin).unwrap();
        std::fs::write(admin.join("commondir"), "../..\n").unwrap();
        let work = scratch.path().join("w");
        std::fs::create_dir_all(&work).unwrap();
        std::fs::write(work.join(".git"), format!("gitdir: {}\n", admin.display())).unwrap();
        let deny = [
            work.join(".git"),
            main.join(".git/config"),
            main.join(".git/hooks"),
        ];
        let deny: Vec<&Path> = deny.iter().map(PathBuf::as_path).collect();
        let recorded = Recorded::resolve(&work, &settings(&work, &deny)).unwrap();
        assert_eq!(recorded.root, std::fs::canonicalize(&work).unwrap());
        assert_eq!(
            recorded.common_dir,
            std::fs::canonicalize(main.join(".git")).unwrap()
        );
        let writable_main = settings(scratch.path(), &deny);
        assert!(Recorded::resolve(&work, &writable_main).is_err());
        let open = settings(&main, &[]);
        assert!(Recorded::resolve(&work, &open).is_err());
    }

    #[test]
    fn verify_notices_a_replaced_git_directory() {
        let scratch = Scratch::new("repo-replaced");
        let work = scratch.path().join("work");
        make_repo(&work);
        let deny = [
            work.join(".git"),
            work.join(".git/config"),
            work.join(".git/hooks"),
        ];
        let deny: Vec<&Path> = deny.iter().map(PathBuf::as_path).collect();
        let recorded = Recorded::resolve(&work, &settings(&work, &deny)).unwrap();
        std::fs::rename(work.join(".git"), work.join("old")).unwrap();
        make_repo(&work);
        assert_eq!(recorded.verify(), Err(Refusal::RepoChanged));
    }
}
