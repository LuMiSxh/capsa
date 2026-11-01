// SPDX-License-Identifier: MPL-2.0
//! Owns the `git` status, diff and show commands and their acus equivalents.

use super::*;

/// Hints at plain `git status|diff|show`, which print more than an agent needs: `acus diff`
/// and `acus log` group and cap them. Output already compacted by a flag, and `git show REV:path`
/// file reads, stay unhinted.
pub(super) fn git_hint(args: &[&str]) -> Option<Verdict> {
    let mut it = args.iter().copied();
    let sub = loop {
        match it.next()? {
            "-C" | "-c" | "--git-dir" | "--work-tree" => {
                it.next();
            }
            a if a.starts_with('-') => {}
            a => break a,
        }
    };
    let rest: Vec<&str> = it.collect();
    let has = |flags: &[&str]| {
        rest.iter().any(|a| {
            flags
                .iter()
                .any(|f| a == f || a.starts_with(&format!("{f}=")))
        })
    };
    let (rule, hint) = match sub {
        "status" if !has(&["-s", "-sb", "--short", "--porcelain"]) => (
            Rule::GitStatus,
            "acus hint: `acus diff` lists uncommitted changes per file and function with line counts, untracked files included; `-p` adds the changed lines and `--staged` limits it to the index.",
        ),
        "diff"
            if !has(&[
                "--shortstat",
                "--numstat",
                "--name-only",
                "--name-status",
                "--quiet",
                "--check",
                "--exit-code",
                "--summary",
                "--no-index",
            ]) =>
        {
            (
                Rule::GitDiff,
                "acus hint: `acus diff [REV|A..B] [PATH…]` groups changes by enclosing function; `-p` adds the changed lines, `--staged` limits it to the index.",
            )
        }
        "show"
            if !has(&[
                "--stat",
                "--name-only",
                "--name-status",
                "-s",
                "--no-patch",
                "--format",
                "--pretty",
            ]) && !rest.iter().any(|a| a.contains(':')) =>
        {
            (
                Rule::GitShow,
                "acus hint: `acus diff REV^..REV` shows one commit's changes per function (`-p` adds the lines); `acus log` lists commits.",
            )
        }
        _ => return None,
    };
    Some(Verdict::Hint(rule, hint.into()))
}

/// Returns the `acus diff` call that answers `git diff ARGS`, when acus diff takes every argument:
/// one revision (`REV` or `A..B`), `--stat`, `-p`, `--staged` or `--cached`, and paths after `--`.
/// `A...B`, other flags and `-C` keep the hint.
pub(super) fn diff_acus(seg: &str) -> Option<String> {
    let (head, words) = head(seg)?;
    let args: Vec<&str> = words.collect();
    let (sub, rest) = args.split_first()?;
    if head != "git" || *sub != "diff" {
        return None;
    }
    let (mut stat, mut patch, mut staged, mut after_dashes) = (false, false, false, false);
    let (mut revs, mut paths) = (Vec::new(), Vec::new());
    for a in rest {
        if after_dashes {
            paths.push(*a);
        } else if *a == "--" {
            after_dashes = true;
        } else if a.starts_with('-') {
            match *a {
                "--stat" => stat = true,
                "-p" | "--patch" => patch = true,
                "--staged" | "--cached" => staged = true,
                _ => return None,
            }
        } else {
            revs.push(*a);
        }
    }
    if revs.len() > 1 || (stat && patch) || paths.iter().any(|p| p.starts_with('-')) {
        return None;
    }
    if let Some(r) = revs.first()
        && (r.contains("...") || (staged && r.contains("..")))
    {
        return None;
    }
    let mut out = String::from("acus diff");
    if staged {
        out.push_str(" --staged");
    }
    if !stat {
        out.push_str(" -p");
    }
    // Without a revision, `git diff -- PATH` may name a deleted file, which acus would read as a
    // revision.
    if revs.is_empty() && !paths.is_empty() {
        out.push_str(" HEAD");
    }
    for a in revs.iter().chain(&paths) {
        out.push(' ');
        out.push_str(a);
    }
    Some(out)
}

/// Reports whether a command is a recursive directory listing: `tree`, `ls -R`, `find` without
/// filters, `Get-ChildItem -Recurse`. A name filter or wildcard makes it a file search, which
/// stays unhinted.
pub(super) fn is_listing(head: &str, args: &[&str], ps: bool) -> bool {
    pub(super) const FIND_FILTERS: &[&str] = &[
        "-name", "-iname", "-path", "-ipath", "-regex", "-exec", "-execdir", "-delete", "-newer",
        "-mtime", "-mmin", "-size", "-empty", "-perm", "-user",
    ];
    let short = |a: &&&str| a.starts_with('-') && !a.starts_with("--");
    match head {
        "tree" => true,
        "ls" if !ps => {
            args.iter().filter(short).any(|a| a.contains('R')) || args.contains(&"--recursive")
        }
        // `-r` is `-Recurse` in PowerShell, any prefix of it being enough.
        "get-childitem" | "gci" | "dir" | "ls" if ps || head != "ls" => {
            args.iter().any(|a| {
                let name = a.split(':').next().unwrap_or(a);
                *a == "/s" || (name.len() > 1 && "-recurse".starts_with(name))
            }) && !args
                .iter()
                .any(|a| a.contains('*') || matches!(*a, "-filter" | "-include"))
        }
        "find" => !args.iter().any(|a| FIND_FILTERS.contains(a)),
        _ => false,
    }
}
