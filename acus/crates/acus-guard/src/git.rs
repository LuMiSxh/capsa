// SPDX-License-Identifier: MPL-2.0
//! Owns the `git` status, diff, log and show commands and their acus equivalents.

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

/// Returns the `acus diff` or `acus log` call that answers a `git diff` or `git log` whose output
/// the agent reads, when acus takes every argument. Anything else stays as it is.
pub(super) fn rewrite_acus(seg: &str) -> Option<(Rule, String)> {
    let (head, words) = head(seg)?;
    let args: Vec<&str> = words.collect();
    let (sub, rest) = args.split_first()?;
    if head != "git" {
        return None;
    }
    match *sub {
        "diff" => diff_acus(rest).map(|new| (Rule::GitDiff, new)),
        "log" => log_acus(rest).map(|new| (Rule::GitLog, new)),
        _ => None,
    }
}

/// Returns the `acus diff` call that answers `git diff ARGS`, when acus diff takes every argument:
/// one revision (`REV` or `A..B`), `--stat`, `-p`, `--staged` or `--cached`, and paths after `--`.
/// `A...B`, other flags and `-C` keep the hint.
fn diff_acus(rest: &[&str]) -> Option<String> {
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

/// Returns the `acus log` call for `git log REST`, when every argument keeps the commits and paths
/// the agent asks for: a count, `--oneline`, the built-in layouts (`--pretty` and `--format` with
/// `oneline`, `short`, `medium` or `full`, `--date=…`) and `--stat`, one revision and paths after
/// `--`. Patching (`-p`), selecting (`--grep`, `-S`, `--author`, `--all`) and walking (`--graph`,
/// `--follow`) change what is listed, and a format string (`--pretty=format:…`,
/// `--format=%H`) asks for git's exact output; both stay as they are.
fn log_acus(rest: &[&str]) -> Option<String> {
    let (mut count, mut oneline, mut after_dashes) = (None, false, false);
    let (mut revs, mut paths) = (Vec::new(), Vec::new());
    let mut it = rest.iter().copied().peekable();
    while let Some(a) = it.next() {
        if after_dashes {
            paths.push(a);
        } else if a == "--" {
            after_dashes = true;
        } else if let Some(n) = count_arg(a, &mut it) {
            if count.replace(n).is_some() {
                return None;
            }
        } else if a == "--oneline" {
            oneline = true;
        } else if a == "--stat" || a == "--pretty" || a.starts_with("--date=") || names_a_layout(a)
        {
            // Display only: acus log prints its own line and table.
        } else if a.starts_with('-') || unquote(a).starts_with('-') {
            return None;
        } else {
            revs.push(a);
        }
    }
    if revs.len() > 1 || paths.iter().any(|p| unquote(p).starts_with('-')) {
        return None;
    }
    let mut out = String::from("acus log");
    if let Some(n) = count {
        out.push_str(&format!(" -n {n}"));
    }
    if oneline {
        out.push_str(" --oneline");
    }
    // Without a revision, acus log reads a path that does not exist as a revision. HEAD is the
    // revision git log defaults to, and naming it keeps the paths paths.
    if revs.is_empty() && !paths.is_empty() {
        revs.push("HEAD");
    }
    for a in revs.iter().chain(&paths) {
        out.push(' ');
        out.push_str(a);
    }
    Some(out)
}

/// Reports whether a `--pretty=` or `--format=` value names one of git's built-in layouts. Any
/// other value is a format string, whose exact output acus log does not print.
fn names_a_layout(a: &str) -> bool {
    ["--pretty=", "--format="].iter().any(|p| {
        a.strip_prefix(p)
            .is_some_and(|v| matches!(v, "oneline" | "short" | "medium" | "full"))
    })
}

/// Returns the count of a commit-count flag, taking its separate value when it has one. Other
/// words, and a flag whose value is missing or not a number, give `None`.
fn count_arg<'a>(a: &'a str, it: &mut impl Iterator<Item = &'a str>) -> Option<usize> {
    let value = if a == "-n" || a == "--max-count" {
        it.next()?
    } else if let Some(n) = a.strip_prefix("-n") {
        n
    } else if let Some(n) = a.strip_prefix("--max-count=") {
        n
    } else {
        a.strip_prefix('-')
            .filter(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()))?
    };
    value.parse().ok()
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
