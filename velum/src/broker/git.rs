// SPDX-License-Identifier: MPL-2.0
//! Plans `git_push`: which branch goes to which remote, and whether it needs a stricter
//! approval.

use super::approval::{Gate, Remember};
use super::exec::{Executor, Ran};
use super::repo::Recorded;
use super::request::{GitPushArgs, Refusal};
use super::text::MAX_FIELD;
use super::tools::Plan;

/// Branches treated as default whatever the repository's own pointer says. The pointer is a
/// file in `.git/refs` that the sandbox can rewrite.
const ALWAYS_DEFAULT: [&str; 2] = ["main", "master"];

/// The branch types of the repository's naming rule, `<type>/<outcome>`.
const BRANCH_TYPES: [&str; 8] = [
    "feat", "fix", "perf", "refactor", "test", "docs", "chore", "release",
];

/// Runs a git question about the recorded repository.
pub fn ask(exec: &dyn Executor, repo: &Recorded, args: &[&str]) -> Result<Ran, Refusal> {
    exec.run(&repo.root, &repo.git(args), Some(repo))
        .map_err(|err| Refusal::Spawn("git".to_string(), err.to_string()))
}

/// Returns the lines of a successful git answer.
pub fn lines(exec: &dyn Executor, repo: &Recorded, args: &[&str]) -> Result<Vec<String>, Refusal> {
    let ran = ask(exec, repo, args)?;
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
fn current_branch(exec: &dyn Executor, repo: &Recorded) -> Result<String, Refusal> {
    let ran = ask(exec, repo, &["symbolic-ref", "--quiet", "--short", "HEAD"])?;
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
fn default_branches(exec: &dyn Executor, repo: &Recorded, remote: &str) -> Vec<String> {
    let pointer = format!("refs/remotes/{remote}/HEAD");
    let named = ask(
        exec,
        repo,
        &["symbolic-ref", "--quiet", "--short", &pointer],
    )
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

/// Returns whether `branch` is named `<type>/<outcome>` with a type from the naming rule and
/// an outcome in lower-case words, digits, `-` and `.`.
pub fn is_work_branch(branch: &str) -> bool {
    branch.split_once('/').is_some_and(|(kind, outcome)| {
        BRANCH_TYPES.contains(&kind)
            && outcome.starts_with(|c: char| c.is_ascii_lowercase() || c.is_ascii_digit())
            && outcome
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '.')
    })
}

/// Returns the URL a push to `remote` uses.
pub fn push_url(exec: &dyn Executor, repo: &Recorded, remote: &str) -> Result<String, Refusal> {
    let found = lines(exec, repo, &["remote", "get-url", "--push", remote])?;
    found
        .into_iter()
        .next()
        .filter(|url| !url.is_empty())
        .ok_or_else(|| Refusal::Git("remote get-url".to_string(), "no URL".to_string()))
}

/// Plans a push of the checked-out branch to the same-named branch of a configured remote.
/// Only a branch named by the naming rule, pushed without `force`, may be remembered; the
/// approval is bound to the push URL.
pub fn push_plan(
    exec: &dyn Executor,
    repo: &Recorded,
    args: &GitPushArgs,
) -> Result<Plan, Refusal> {
    let remote = args.remote.as_deref().unwrap_or("origin");
    if remote.chars().count() > MAX_FIELD {
        return Err(Refusal::TooLong("the remote name", MAX_FIELD));
    }
    let configured = lines(exec, repo, &["remote"])?;
    if remote.starts_with('-') || !configured.iter().any(|name| name == remote) {
        return Err(Refusal::UnknownRemote(remote.to_string()));
    }
    let branch = current_branch(exec, repo)?;
    let on_default = default_branches(exec, repo, remote).contains(&branch);
    let url = push_url(exec, repo, remote)?;
    let rememberable = !args.force && !on_default && is_work_branch(&branch);
    let gate = if rememberable {
        Gate {
            class: "git_push",
            remember: Remember::Repo,
        }
    } else {
        Gate {
            class: "git_push_per_call",
            remember: Remember::Never,
        }
    };
    let refspec = format!("HEAD:refs/heads/{branch}");
    let mut push = vec!["push", "--no-verify", "-u"];
    if args.force {
        push.push("--force-with-lease");
    }
    push.extend([remote, &refspec]);
    let mut details = vec![
        ("Remote", remote.to_string()),
        ("URL", url.clone()),
        ("Branch", branch),
        ("Pushes", refspec.clone()),
    ];
    if on_default {
        details.push(("Warning", "this is the default branch".to_string()));
    }
    if args.force {
        details.push(("Warning", "force push (--force-with-lease)".to_string()));
    }
    if !rememberable && !on_default && !args.force {
        details.push((
            "Note",
            "not a <type>/<outcome> branch, asked every time".to_string(),
        ));
    }
    Ok(Plan {
        tool: "git_push",
        gate,
        repo: repo.root.clone(),
        subject: url,
        details,
        argv: repo.git(&push),
        bound: true,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_typed_kebab_case_branches_are_work_branches() {
        for ok in [
            "feat/approval-broker",
            "fix/pill-height",
            "release/1.2.3",
            "docs/a",
        ] {
            assert!(is_work_branch(ok), "{ok}");
        }
        for bad in [
            "main",
            "master",
            "feat",
            "feat/",
            "feat/Upper",
            "feat/a/b",
            "wip/thing",
            "feat/-x",
            "feat/a_b",
            "Feat/x",
            "../feat/x",
        ] {
            assert!(!is_work_branch(bad), "{bad}");
        }
    }
}
