// SPDX-License-Identifier: MPL-2.0
//! Owns the srt settings, the filtered environment and the run of the sandboxed command.

use crate::policy::{Entry, Policy, PolicyError, env_matches, expand_entry};
use crate::rundir::RunDir;
use crate::{proc, safedir};
use anyhow::{Context, Result};
use capsa_core::hint::Hinted;
use capsa_core::which;
use serde_json::{Value, json};
use std::ffi::{OsStr, OsString};
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The agent's `TMPDIR`. Agent-writable, so nothing velum trusts lives here.
pub const TMP_DIR: &str = "/private/tmp/velum";

/// The `TMPDIR` srt sets for its child, whatever velum passes. srt does not create it, and
/// swift fails without it.
pub const SRT_TMP_DIR: &str = "/private/tmp/claude";

/// Both temporary directories, created before every run.
pub const TMP_DIRS: [&str; 2] = [TMP_DIR, SRT_TMP_DIR];

/// Creates the temporary directories (mode 0700); an existing one must be a real directory
/// owned by the user, not a symlink.
pub fn prepare_tmp() -> Result<()> {
    TMP_DIRS
        .iter()
        .try_for_each(|dir| safedir::ensure_private_dir(Path::new(dir)))
}

/// Variables never passed to the child, whatever the policy says.
pub const ENV_DROP: &[&str] = &["CAPSA_HUMAN"];

/// Name parts that mark a secret. A variable containing one is never passed.
const SECRET_PARTS: &[&str] = &["TOKEN", "SECRET", "PASSWORD", "PASSWD", "CREDENTIAL"];

/// Builds the text of a wrapper script for the real binary at a path.
type WrapperScript = fn(&Path) -> String;

/// Wrapper scripts written to `<run dir>/bin`, each with the generator for its script text.
const WRAPPERS: &[(&str, WrapperScript)] =
    &[("swift", swift_script), ("xcodebuild", xcodebuild_script)];

/// Builds the srt settings document. `deniedDomains` is required by srt, even when empty.
/// Every file-system entry is expanded and checked (see `expand_entry`).
///
/// When `home` is a symlink, the deny lists hold each `~` entry for the path as given and
/// for the canonical path, so the sandbox sees the one it resolves.
pub fn settings(policy: &Policy, home: &str) -> Result<Value, PolicyError> {
    let canonical = std::fs::canonicalize(home)
        .ok()
        .and_then(|path| path.to_str().map(str::to_string))
        .filter(|real| real != home);
    let expand = |paths: &[String], kind, both: bool| -> Result<Vec<String>, PolicyError> {
        let mut out: Vec<String> = Vec::new();
        for path in paths {
            let homes = std::iter::once(home).chain(canonical.as_deref().filter(|_| both));
            for home in homes {
                let entry = expand_entry(path, home, kind)?;
                if !out.contains(&entry) {
                    out.push(entry);
                }
            }
        }
        Ok(out)
    };
    Ok(json!({
        "network": {
            "allowedDomains": policy.net_allow,
            "deniedDomains": [],
        },
        "filesystem": {
            "denyRead": expand(&policy.deny_read, Entry::Deny, true)?,
            "allowWrite": expand(&policy.write, Entry::Allow, false)?,
            "denyWrite": expand(&policy.deny_write, Entry::Deny, true)?,
            // srt always denies `.git/hooks` and, unless told otherwise, `.git/config`
            // below the working directory. `git init` writes the config; the repositories
            // that exist before the run keep a concrete deny (`gitguard`).
            "allowGitConfig": true,
        },
    }))
}

/// Copies only the allowed variables from the current environment, and sets TMPDIR.
pub fn filtered_env(policy: &Policy) -> Vec<(String, String)> {
    let mut env = select_env(policy, std::env::vars());
    env.push(("TMPDIR".to_string(), TMP_DIR.to_string()));
    env
}

/// Returns whether a name looks like a secret: it contains one of `SECRET_PARTS` or ends in
/// `_KEY`.
fn looks_secret(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    SECRET_PARTS.iter().any(|part| upper.contains(part)) || upper.ends_with("_KEY")
}

/// Returns whether the child gets the variable `name`. `ENV_DROP` never passes. A secret-looking
/// name passes only when `env.pass_secret` lists it exactly, even when `env.pass` matches
/// it; any other name passes when `env.pass` matches it.
fn passes(policy: &Policy, name: &str) -> bool {
    if ENV_DROP.contains(&name) {
        return false;
    }
    if looks_secret(name) {
        return policy.env_pass_secret.iter().any(|secret| secret == name);
    }
    policy.env_pass.iter().any(|p| env_matches(p, name))
}

/// Returns the secret-looking names in `names` that `policy` does not pass (for
/// `velum self doctor`).
pub fn withheld_secrets(policy: &Policy, names: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut found: Vec<String> = names
        .into_iter()
        .filter(|name| looks_secret(name) && !passes(policy, name))
        .collect();
    found.sort();
    found
}

fn select_env(
    policy: &Policy,
    vars: impl IntoIterator<Item = (String, String)>,
) -> Vec<(String, String)> {
    vars.into_iter()
        .filter(|(name, _)| passes(policy, name))
        .collect()
}

/// Finds `srt` in the absolute entries of `path`, then in the user bin directories of
/// `capsa_core::which` below `home` (pnpm's global bin `~/Library/pnpm/bin` among them),
/// so an install that is not on PATH is still found. A relative or empty PATH entry (the
/// current directory, which the agent can fill) is never searched.
pub fn find_srt(home: &str, path: &OsStr) -> Option<PathBuf> {
    let mut env = which::Env::from_path(path);
    env.path.retain(|dir| dir.is_absolute());
    env.home = Some(PathBuf::from(home));
    which::find_in(&env, &which::RealFs, "srt", None)
}

/// Returns the absolute entries of a PATH value; empty and relative entries mean the current
/// directory.
fn absolute_entries(path: &OsStr) -> Vec<PathBuf> {
    std::env::split_paths(path)
        .filter(|entry| entry.is_absolute())
        .collect()
}

/// Writes each wrapper into `<run dir>/bin` and puts that directory first on PATH in `env`.
/// A wrapper is written only when its tool is on the original PATH.
pub fn install_wrappers(env: &mut Vec<(String, String)>, run_dir: &RunDir) -> Result<()> {
    let bin_dir = run_dir.make_dir("bin")?;
    let original = absolute_entries(&std::env::var_os("PATH").unwrap_or_default());
    let original = std::env::join_paths(&original).context("joining PATH")?;
    for (name, script) in WRAPPERS {
        let Some(real) = resolve(name, &original, &bin_dir) else {
            continue;
        };
        if !real.is_absolute() {
            return Err(Hinted::new(
                format!("{name} resolved to the relative path {}", real.display()),
                "remove relative entries from PATH",
            )
            .into());
        }
        run_dir.write_file(&format!("bin/{name}"), script(&real).as_bytes(), 0o700)?;
    }
    let mut path = OsString::from(bin_dir);
    if !original.is_empty() {
        path.push(":");
        path.push(&original);
    }
    env.retain(|(name, _)| name != "PATH");
    env.push(("PATH".to_string(), path.to_string_lossy().into_owned()));
    Ok(())
}

/// Sets the git and cargo variables that let new repositories be made under srt, which
/// always denies writes to `.git/hooks`. `GIT_TEMPLATE_DIR` is an empty directory in the
/// run directory (git then copies no sample hooks, and the agent cannot fill it).
/// libgit2, which cargo uses, ignores that variable and writes `hooks/README.sample`, so
/// `cargo new` and `cargo init` get `vcs = "none"`; `git init` is the way to make a repository.
pub fn install_git_env(env: &mut Vec<(String, String)>, run_dir: &RunDir) -> Result<()> {
    let template = run_dir.make_dir("git-template")?;
    env.retain(|(name, _)| name != "GIT_TEMPLATE_DIR" && name != "CARGO_CARGO_NEW_VCS");
    env.push((
        "GIT_TEMPLATE_DIR".to_string(),
        template.to_string_lossy().into_owned(),
    ));
    env.push(("CARGO_CARGO_NEW_VCS".to_string(), "none".to_string()));
    Ok(())
}

/// Finds `name` on `path`, skipping the `skip` directory.
fn resolve(name: &str, path: &OsStr, skip: &Path) -> Option<PathBuf> {
    which::find_on_path(name, path, Some(skip)).map(|(_, real)| real)
}

/// Quotes a path as one POSIX sh word.
pub fn sh_quote(path: &Path) -> String {
    format!("'{}'", path.to_string_lossy().replace('\'', r"'\''"))
}

/// Seatbelt cannot nest, so SwiftPM build, test and run must skip their own sandbox.
fn swift_script(real: &Path) -> String {
    format!(
        "#!/bin/sh\n\
         # Generated by velum. Seatbelt cannot nest, so SwiftPM runs without its own sandbox.\n\
         real={real}\n\
         case \"$1\" in\n\
         build|test|run) sub=\"$1\"; shift; exec \"$real\" \"$sub\" --disable-sandbox \"$@\" ;;\n\
         esac\n\
         exec \"$real\" \"$@\"\n",
        real = sh_quote(real)
    )
}

/// Xcode skips its manifest sandbox, which cannot nest inside srt.
fn xcodebuild_script(real: &Path) -> String {
    format!(
        "#!/bin/sh\n\
         # Generated by velum. Seatbelt cannot nest, so Xcode skips its manifest sandbox.\n\
         exec {} -IDEPackageSupportDisableManifestSandbox=1 \"$@\"\n",
        sh_quote(real)
    )
}

/// Records how the sandboxed command ended.
#[derive(Debug)]
pub struct Ran {
    /// The exit code; 128 plus the signal number when it was killed by a signal.
    pub code: u8,
    /// Processes of the sandbox were still running after it exited and were killed.
    pub killed_survivors: bool,
}

/// Writes the settings file into the run directory and runs `srt -s <file> -- <cmd>` in its
/// own process group (see `proc`). The run directory is removed by its owner's drop.
pub fn run(
    bin: &Path,
    run_dir: &RunDir,
    settings: &Value,
    env: Vec<(String, String)>,
    cmd: &[String],
) -> Result<Ran> {
    prepare_tmp()?;
    let settings_path = run_dir.write_file(
        "settings.json",
        &serde_json::to_vec_pretty(settings)?,
        0o600,
    )?;
    let mut command = Command::new(bin);
    command
        .arg("-s")
        .arg(&settings_path)
        .arg("--")
        .args(cmd)
        .env_clear()
        .envs(env);
    let done =
        proc::run_in_group(&mut command).with_context(|| format!("running {}", bin.display()))?;
    let code = done
        .status
        .code()
        .or_else(|| done.status.signal().map(|signal| 128 + signal))
        .unwrap_or(1);
    Ok(Ran {
        code: u8::try_from(code).unwrap_or(1),
        killed_survivors: done.killed_survivors,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn run_does_not_start_srt_when_the_settings_file_cannot_be_written() {
        use crate::scratch::{Scratch, write_program};
        use std::os::unix::fs::PermissionsExt;
        let scratch = Scratch::new("srt-no-settings");
        let marker = scratch.path().join("started");
        let bin = scratch.path().join("bin");
        write_program(&bin, "srt", &format!("/usr/bin/touch {}", marker.display()));
        let home = scratch.path().to_str().unwrap();
        let run_dir = RunDir::create(home).unwrap();
        let dir = run_dir.path().to_path_buf();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o500)).unwrap();
        let cmd = ["true".to_string()];
        let result = run(&bin.join("srt"), &run_dir, &json!({}), Vec::new(), &cmd);
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert!(result.is_err());
        assert!(!marker.exists());
    }

    #[test]
    fn settings_always_contain_denied_domains() {
        let policy = Policy::builtin(Path::new("/work"));
        let value = settings(&policy, "/Users/a").unwrap();
        assert_eq!(value["network"]["deniedDomains"], json!([]));
        assert_eq!(value["filesystem"]["denyRead"][0], "/Users/a/.ssh");
    }

    #[test]
    fn settings_refuse_relative_and_parent_dir_entries() {
        let mut policy = Policy::builtin(Path::new("/work"));
        policy.write.push("../escape".to_string());
        assert!(settings(&policy, "/Users/a").is_err());
        let mut policy = Policy::builtin(Path::new("/work"));
        policy.deny_write.push("~root/x".to_string());
        assert!(settings(&policy, "/Users/a").is_err());
        let policy = Policy::builtin(Path::new("/work"));
        assert!(settings(&policy, "relative/home").is_err());
    }

    #[test]
    fn a_symlinked_home_gets_both_paths_in_the_deny_lists() {
        let scratch = crate::scratch::Scratch::new("srt-home-link");
        let real = scratch.path().join("real");
        let link = scratch.path().join("link");
        std::fs::create_dir_all(&real).unwrap();
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let policy = Policy::builtin(Path::new("/work"));
        let value = settings(&policy, link.to_str().unwrap()).unwrap();
        let canonical = std::fs::canonicalize(&real).unwrap();
        let deny_read = value["filesystem"]["denyRead"].as_array().unwrap();
        let want = [
            format!("{}/.ssh", link.display()),
            format!("{}/.ssh", canonical.display()),
        ];
        for entry in want {
            assert!(deny_read.iter().any(|v| *v == entry), "{entry}");
        }
        let deny_write = value["filesystem"]["denyWrite"].as_array().unwrap();
        assert!(
            deny_write
                .iter()
                .any(|v| *v == format!("{}/.local/bin", canonical.display()))
        );
        let plain = settings(&policy, "/Users/a").unwrap();
        let ssh = plain["filesystem"]["denyRead"].as_array().unwrap();
        assert_eq!(ssh.iter().filter(|v| **v == "/Users/a/.ssh").count(), 1);
    }

    #[test]
    fn srt_is_found_in_user_dirs_but_never_through_relative_path_entries() {
        use crate::scratch::{Scratch, write_program};
        let scratch = Scratch::new("srt-find");
        let home = scratch.path().join("home");
        write_program(&home.join("Library/pnpm/bin"), "srt", "echo 0.0.80");
        let home = home.to_str().unwrap();
        let found = find_srt(home, OsStr::new("")).unwrap();
        assert_eq!(found, Path::new(home).join("Library/pnpm/bin/srt"));
        let first = scratch.path().join("first");
        write_program(&first, "srt", "echo first");
        assert_eq!(
            find_srt(home, first.as_os_str()).unwrap(),
            first.join("srt")
        );
        // Relative and empty entries mean the current directory and are skipped.
        let relative = OsStr::new("first:.:");
        assert_eq!(
            find_srt(home, relative).unwrap(),
            Path::new(home).join("Library/pnpm/bin/srt")
        );
    }

    #[test]
    fn secret_looking_names_pass_only_when_listed_exactly() {
        let mut policy = Policy::builtin(Path::new("/work"));
        policy.env_pass.push("*".to_string());
        policy.env_pass_secret = vec!["ANTHROPIC_API_KEY".to_string(), "CAPSA_*".to_string()];
        let passed = names(
            &policy,
            &[
                ("ANTHROPIC_API_KEY", "k"),
                ("CLAUDE_CODE_OAUTH_TOKEN", "t"),
                ("CAPSA_HUMAN", "1"),
                ("HOME", "/h"),
            ],
        );
        assert_eq!(passed, ["ANTHROPIC_API_KEY", "HOME"]);
        let withheld = withheld_secrets(
            &policy,
            ["ANTHROPIC_API_KEY", "CLAUDE_CODE_OAUTH_TOKEN", "HOME"].map(str::to_string),
        );
        assert_eq!(withheld, ["CLAUDE_CODE_OAUTH_TOKEN"]);
    }

    #[test]
    fn the_secret_filter_beats_env_pass_without_pass_secret() {
        let mut policy = Policy::builtin(Path::new("/work"));
        policy.env_pass.push("ANTHROPIC_API_KEY".to_string());
        policy.env_pass.push("CLAUDE_CODE_OAUTH_TOKEN".to_string());
        assert!(
            names(
                &policy,
                &[("ANTHROPIC_API_KEY", "k"), ("CLAUDE_CODE_OAUTH_TOKEN", "t")]
            )
            .is_empty()
        );
    }

    fn names(policy: &Policy, vars: &[(&str, &str)]) -> Vec<String> {
        let vars = vars
            .iter()
            .map(|(name, value)| (name.to_string(), value.to_string()));
        select_env(policy, vars)
            .into_iter()
            .map(|(n, _)| n)
            .collect()
    }

    #[test]
    fn cargo_secrets_are_not_passed_by_default() {
        let policy = Policy::builtin(Path::new("/work"));
        let passed = names(
            &policy,
            &[
                ("CARGO_HOME", "/h"),
                ("CARGO_TARGET_DIR", "/t"),
                ("CARGO_REGISTRY_TOKEN", "s"),
                ("CARGO_REGISTRIES_X_TOKEN", "s"),
                ("CARGO_NET_GIT_FETCH_WITH_CLI", "true"),
                ("RUSTUP_TOOLCHAIN", "stable"),
            ],
        );
        assert_eq!(
            passed,
            ["CARGO_HOME", "CARGO_TARGET_DIR", "RUSTUP_TOOLCHAIN"]
        );
    }

    #[test]
    fn hard_drop_list_beats_policy_wildcards() {
        let mut policy = Policy::builtin(Path::new("/work"));
        policy.env_pass.push("*".to_string());
        policy.env_pass.push("CARGO_*".to_string());
        let passed = names(
            &policy,
            &[
                ("CARGO_REGISTRY_TOKEN", "s"),
                ("CARGO_REGISTRIES_X_TOKEN", "s"),
                ("GITHUB_SECRET", "s"),
                ("DB_PASSWORD", "s"),
                ("DB_PASSWD", "s"),
                ("GOOGLE_CREDENTIALS", "s"),
                ("OPENAI_API_KEY", "s"),
                ("STRIPE_KEY", "s"),
                ("my_token", "s"),
                ("CAPSA_HUMAN", "1"),
                ("CARGO_HOME", "/h"),
                ("KEYBOARD", "us"),
            ],
        );
        assert_eq!(passed, ["CARGO_HOME", "KEYBOARD"]);
    }

    #[test]
    fn wrappers_land_in_the_run_dir_with_private_modes() {
        use std::os::unix::fs::PermissionsExt;
        let scratch = crate::scratch::Scratch::new("srt-wrappers");
        let run_dir = RunDir::create(scratch.path().to_str().unwrap()).unwrap();
        let mut env = Vec::new();
        install_wrappers(&mut env, &run_dir).unwrap();
        let path = &env.iter().find(|(n, _)| n == "PATH").unwrap().1;
        assert!(path.starts_with(&format!("{}/bin", run_dir.path().display())));
        if let Ok(meta) = std::fs::metadata(run_dir.path().join("bin/swift")) {
            assert_eq!(meta.permissions().mode() & 0o777, 0o700);
        }
    }

    #[test]
    fn git_env_points_at_an_empty_template_dir_in_the_run_dir() {
        let scratch = crate::scratch::Scratch::new("srt-git-env");
        let run_dir = RunDir::create(scratch.path().to_str().unwrap()).unwrap();
        let mut env = vec![("GIT_TEMPLATE_DIR".to_string(), "/evil".to_string())];
        install_git_env(&mut env, &run_dir).unwrap();
        let get = |name: &str| env.iter().find(|(n, _)| n == name).map(|(_, v)| v.clone());
        let template = get("GIT_TEMPLATE_DIR").unwrap();
        assert!(template.starts_with(run_dir.path().to_str().unwrap()));
        assert_eq!(std::fs::read_dir(&template).unwrap().count(), 0);
        assert_eq!(get("CARGO_CARGO_NEW_VCS").as_deref(), Some("none"));
        assert_eq!(
            env.iter().filter(|(n, _)| n == "GIT_TEMPLATE_DIR").count(),
            1
        );
    }

    #[test]
    fn path_for_the_child_drops_empty_and_relative_entries() {
        let kept = absolute_entries(OsStr::new(":rel:./x:/usr/bin::/bin"));
        assert_eq!(kept, [PathBuf::from("/usr/bin"), PathBuf::from("/bin")]);
    }

    #[test]
    fn capsa_human_is_dropped_even_when_policy_matches() {
        let mut policy = Policy::builtin(Path::new("/work"));
        policy.env_pass.push("CAPSA_*".to_string());
        let vars = [("CAPSA_HUMAN", "1"), ("CAPSA_OTHER", "2"), ("PATH", "/bin")]
            .map(|(name, value)| (name.to_string(), value.to_string()));
        let names: Vec<String> = select_env(&policy, vars)
            .into_iter()
            .map(|(name, _)| name)
            .collect();
        assert_eq!(names, ["CAPSA_OTHER", "PATH"]);
    }

    #[test]
    fn sh_quote_escapes_single_quotes() {
        assert_eq!(sh_quote(Path::new("/a b/it's")), r"'/a b/it'\''s'");
    }

    #[test]
    fn swift_wrapper_disables_sandbox_for_build_test_and_run() {
        let script = swift_script(Path::new("/usr/bin/swift"));
        assert!(script.starts_with("#!/bin/sh\n"));
        assert!(script.contains("\nreal='/usr/bin/swift'\n"));
        assert!(script.contains(
            "build|test|run) sub=\"$1\"; shift; exec \"$real\" \"$sub\" --disable-sandbox \"$@\" ;;\n"
        ));
        assert!(script.ends_with("exec \"$real\" \"$@\"\n"));
    }

    #[test]
    fn xcodebuild_wrapper_adds_manifest_sandbox_flag() {
        let script = xcodebuild_script(Path::new(
            "/Applications/Xcode.app/Contents/Developer/usr/bin/xcodebuild",
        ));
        assert_eq!(
            script,
            "#!/bin/sh\n\
             # Generated by velum. Seatbelt cannot nest, so Xcode skips its manifest sandbox.\n\
             exec '/Applications/Xcode.app/Contents/Developer/usr/bin/xcodebuild' -IDEPackageSupportDisableManifestSandbox=1 \"$@\"\n"
        );
    }
}
