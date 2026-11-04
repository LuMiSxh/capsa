// SPDX-License-Identifier: MPL-2.0
//! Reads `git status --porcelain=v2` for the `acus diff` header and file lists.

use crate::git::git;
use anyhow::Result;
use std::path::{Path, PathBuf};

/// The `git status` flags behind the header, the untracked list and the unmerged paths.
const FLAGS: &[&str] = &[
    "-c",
    "core.quotepath=off",
    "status",
    "--porcelain=v2",
    "--branch",
    "--untracked-files=all",
];

/// A git operation that stopped for the user to resolve it.
#[derive(Clone, Copy)]
enum Operation {
    Merge,
    Rebase,
    CherryPick,
}

impl Operation {
    fn name(self) -> &'static str {
        match self {
            Operation::Merge => "merge",
            Operation::Rebase => "rebase",
            Operation::CherryPick => "cherry-pick",
        }
    }
}

/// The working tree's status: branch, upstream position, change counts and conflict state.
#[derive(Default)]
pub(super) struct Status {
    head: String,
    oid: String,
    upstream: Option<String>,
    ahead: usize,
    behind: usize,
    staged: usize,
    unstaged: usize,
    /// Untracked paths, relative to the working directory.
    pub(super) untracked: Vec<String>,
    /// Unmerged paths, relative to the working directory.
    pub(super) unmerged: Vec<String>,
    operation: Option<Operation>,
}

impl Status {
    /// Runs `git status` once, limited to `paths` when they are given.
    pub(super) fn load(paths: &[String]) -> Result<Self> {
        let mut cmd = FLAGS.to_vec();
        if !paths.is_empty() {
            cmd.push("--");
            cmd.extend(paths.iter().map(String::as_str));
        }
        let mut s = Self::parse(&git(&cmd)?);
        s.operation = operation();
        Ok(s)
    }

    /// The one-line header, with zero and absent parts left out.
    pub(super) fn header(&self) -> String {
        let mut s = format!("on {}", self.branch());
        if let Some(u) = &self.upstream {
            s += &format!(" → {u}");
            if self.ahead > 0 {
                s += &format!(" ahead {}", self.ahead);
            }
            if self.behind > 0 {
                s += &format!(" behind {}", self.behind);
            }
        }
        let parts = [
            (self.staged, "staged"),
            (self.unstaged, "unstaged"),
            (self.untracked.len(), "untracked"),
            (self.unmerged.len(), "conflicts"),
        ];
        for (n, label) in parts {
            if n > 0 {
                s += &format!(" · {label} {n}");
            }
        }
        if let Some(op) = self.operation {
            s += &format!(" · {} in progress", op.name());
        }
        s
    }

    /// The header as JSON fields; counts are always present, absent parts are left out.
    pub(super) fn json(&self) -> serde_json::Value {
        let mut v = serde_json::json!({
            "branch": self.branch(),
            "staged": self.staged,
            "unstaged": self.unstaged,
            "untracked": self.untracked.len(),
            "conflicts": self.unmerged.len(),
        });
        if self.head == "(detached)" {
            v["detached"] = true.into();
        }
        if let Some(u) = &self.upstream {
            v["upstream"] = u.as_str().into();
            if self.ahead > 0 {
                v["ahead"] = self.ahead.into();
            }
            if self.behind > 0 {
                v["behind"] = self.behind.into();
            }
        }
        if let Some(op) = self.operation {
            v["operation"] = op.name().into();
        }
        v
    }

    /// The branch name, or the short hash on a detached HEAD.
    fn branch(&self) -> &str {
        if self.head == "(detached)" {
            self.oid.get(..7).unwrap_or(&self.oid)
        } else {
            &self.head
        }
    }

    fn parse(text: &str) -> Self {
        let mut s = Self::default();
        for line in text.lines() {
            if let Some(v) = line.strip_prefix("# branch.oid ") {
                s.oid = v.to_owned();
            } else if let Some(v) = line.strip_prefix("# branch.head ") {
                s.head = v.to_owned();
            } else if let Some(v) = line.strip_prefix("# branch.upstream ") {
                s.upstream = Some(v.to_owned());
            } else if let Some(v) = line.strip_prefix("# branch.ab ") {
                let (ahead, behind) = v.split_once(' ').unwrap_or((v, ""));
                s.ahead = ahead.trim_start_matches('+').parse().unwrap_or(0);
                s.behind = behind.trim_start_matches('-').parse().unwrap_or(0);
            } else if let Some(p) = line.strip_prefix("? ") {
                s.untracked.push(p.to_owned());
            } else if line.starts_with("u ") {
                if let Some(p) = line.splitn(11, ' ').nth(10) {
                    s.unmerged.push(p.to_owned());
                }
            } else if line.starts_with("1 ") || line.starts_with("2 ") {
                s.count_change(line);
            }
        }
        s
    }

    /// Counts file-level staged and unstaged changes from an ordinary or renamed entry.
    fn count_change(&mut self, line: &str) {
        let Some(xy) = line.split(' ').nth(1) else {
            return;
        };
        let b = xy.as_bytes();
        if b.first().is_some_and(|c| *c != b'.') {
            self.staged += 1;
        }
        if b.get(1).is_some_and(|c| *c != b'.') {
            self.unstaged += 1;
        }
    }
}

/// The operation git stopped in, read from the git directory's marker files.
fn operation() -> Option<Operation> {
    let dir = git_dir()?;
    if dir.join("rebase-merge").is_dir() || dir.join("rebase-apply").is_dir() {
        Some(Operation::Rebase)
    } else if dir.join("MERGE_HEAD").exists() {
        Some(Operation::Merge)
    } else if dir.join("CHERRY_PICK_HEAD").exists() {
        Some(Operation::CherryPick)
    } else {
        None
    }
}

/// Finds the git directory without another `git` call, following a worktree's `.git` file.
fn git_dir() -> Option<PathBuf> {
    let mut dir = std::env::current_dir().ok()?;
    loop {
        let dot = dir.join(".git");
        if dot.is_dir() {
            return Some(dot);
        }
        if dot.is_file() {
            let text = std::fs::read_to_string(&dot).ok()?;
            let path = Path::new(text.lines().next()?.strip_prefix("gitdir: ")?);
            return Some(if path.is_absolute() {
                path.to_owned()
            } else {
                dir.join(path)
            });
        }
        dir = dir.parent()?.to_owned();
    }
}
