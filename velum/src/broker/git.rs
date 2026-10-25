// SPDX-License-Identifier: MPL-2.0
//! Plans `git_push`: which branch goes to which remote, and whether it needs a stricter
//! approval.

use super::approval::{Gate, Remember};
use super::exec::{Executor, Ran};
use super::request::{GitPushArgs, Refusal};
use super::text::MAX_FIELD;
use super::tools::Plan;
use std::path::{Path, PathBuf};

/// Branches treated as default whatever the repository's own pointer says. The pointer is a
/// file in `.git/refs` that the sandbox can rewrite.
const ALWAYS_DEFAULT: [&str; 2] = ["main", "master"];

fn git(exec: &dyn Executor, cwd: &Path, args: &[&str]) -> Result<Ran, Refusal> {
    let argv: Vec<String> = std::iter::once("git")
        .chain(args.iter().copied())
        .map(str::to_string)
        .collect();
    exec.run(cwd, &argv)
        .map_err(|err| Refusal::Spawn("git".to_string(), err.to_string()))
}

/// Returns the canonical top-level directory of the repository around `cwd`, or `cwd`
/// itself (canonical) when git does not name one.
pub fn repo_root(exec: &dyn Executor, cwd: &Path) -> PathBuf {
    let root = git(exec, cwd, &["rev-parse", "--show-toplevel"])
        .ok()
        .filter(|ran| ran.code == 0)
        .map(|ran| PathBuf::from(ran.output.trim()))
        .filter(|path| path.is_absolute());
    let path = root.unwrap_or_else(|| cwd.to_path_buf());
    std::fs::canonicalize(&path).unwrap_or(path)
}

/// Returns the lines of a successful git answer.
fn lines(exec: &dyn Executor, cwd: &Path, args: &[&str]) -> Result<Vec<String>, Refusal> {
    let ran = git(exec, cwd, args)?;
    if ran.code != 0 {
        return Err(Refusal::Git(args.join(" "), ran.output.trim().to_string()));
    }
    Ok(ran
        .output
        .lines()
        .map(|line| line.trim().to_string())
        .collect())
}

/// Returns the checked-out branch, refusing a detached HEAD.
fn current_branch(exec: &dyn Executor, cwd: &Path) -> Result<String, Refusal> {
    let ran = git(exec, cwd, &["symbolic-ref", "--quiet", "--short", "HEAD"])?;
    match ran.code {
        0 if !ran.output.trim().is_empty() => Ok(ran.output.trim().to_string()),
        1 => Err(Refusal::DetachedHead),
        _ => Err(Refusal::Git(
            "symbolic-ref HEAD".to_string(),
            ran.output.trim().to_string(),
        )),
    }
}

/// Returns the branches of `remote` that count as default: `main`, `master` and the one the
/// remote-tracking pointer names.
fn default_branches(exec: &dyn Executor, cwd: &Path, remote: &str) -> Vec<String> {
    let pointer = format!("refs/remotes/{remote}/HEAD");
    let named = git(exec, cwd, &["symbolic-ref", "--quiet", "--short", &pointer])
        .ok()
        .filter(|ran| ran.code == 0)
        .and_then(|ran| {
            ran.output
                .trim()
                .strip_prefix(&format!("{remote}/"))
                .map(str::to_string)
        });
    ALWAYS_DEFAULT
        .iter()
        .map(|name| (*name).to_string())
        .chain(named)
        .collect()
}

/// Plans a push of the checked-out branch to the same-named branch of a configured remote.
/// A default branch or `force` allows only a single approval.
pub fn push_plan(exec: &dyn Executor, cwd: &Path, args: &GitPushArgs) -> Result<Plan, Refusal> {
    let remote = args.remote.as_deref().unwrap_or("origin");
    if remote.chars().count() > MAX_FIELD {
        return Err(Refusal::TooLong("the remote name", MAX_FIELD));
    }
    let configured = lines(exec, cwd, &["remote"])?;
    if remote.starts_with('-') || !configured.iter().any(|name| name == remote) {
        return Err(Refusal::UnknownRemote(remote.to_string()));
    }
    let branch = current_branch(exec, cwd)?;
    let on_default = default_branches(exec, cwd, remote).contains(&branch);
    let strict = args.force || on_default;
    let gate = if strict {
        Gate {
            class: "git_push_strict",
            remember: Remember::Never,
        }
    } else {
        Gate {
            class: "git_push",
            remember: Remember::Repo,
        }
    };
    let refspec = format!("HEAD:refs/heads/{branch}");
    let mut argv: Vec<String> = ["git", "push", "-u"].map(str::to_string).into();
    if args.force {
        argv.push("--force-with-lease".to_string());
    }
    argv.extend([remote.to_string(), refspec.clone()]);
    let url = lines(exec, cwd, &["remote", "get-url", "--push", remote])
        .ok()
        .and_then(|found| found.into_iter().next())
        .unwrap_or_else(|| "unknown".to_string());
    let mut details = vec![
        ("Remote", remote.to_string()),
        ("URL", url),
        ("Branch", branch),
        ("Pushes", refspec),
    ];
    if on_default {
        details.push(("Warning", "this is the default branch".to_string()));
    }
    if args.force {
        details.push(("Warning", "force push (--force-with-lease)".to_string()));
    }
    Ok(Plan {
        tool: "git_push",
        gate,
        repo: repo_root(exec, cwd),
        details,
        argv,
    })
}
