// SPDX-License-Identifier: MPL-2.0
//! Matches the stages of a command against the classification rules.

use super::pipelines::pipelines;
use super::shape::{base, has_short_flag, program, shape_pipe, writes_file};

/// `edit-python` when the python code writes a file, `python-other` otherwise.
fn python_category(cmd: &str) -> &'static str {
    let writes = [".replace(", "re.sub", "write_text", ".write("]
        .iter()
        .any(|k| cmd.contains(k))
        || ["'w'", "\"w\""].iter().any(|k| cmd.contains(k)) && cmd.contains("open(");
    if writes {
        "edit-python"
    } else {
        "python-other"
    }
}

/// Whether a git command shows a diff: `git diff` or `git show`, after its `-C` and `-c`
/// options.
fn git_shows_diff(args: &[String]) -> bool {
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "-C" | "-c" => {
                it.next();
            }
            a if a.starts_with('-') => {}
            "diff" | "show" => return true,
            _ => break,
        }
    }
    false
}

const BUILD_TOOLS: [&str; 7] = [
    "cargo",
    "swift",
    "xcodebuild",
    "pytest",
    "make",
    "gradle",
    "gradlew",
];

fn is_build(p: &[String]) -> bool {
    let b = base(&p[0]);
    match b {
        "npm" | "pnpm" | "yarn" | "bun" => p
            .get(1)
            .is_some_and(|a| matches!(a.as_str(), "run" | "test" | "build")),
        "go" => p.get(1).is_some_and(|a| a == "test" || a == "build"),
        _ => BUILD_TOOLS.contains(&b),
    }
}

/// What a classification row requires of a stage beyond its program name.
#[derive(Clone, Copy)]
enum Pattern {
    /// Any use of the program.
    Any,
    /// The first argument is this subcommand.
    Subcommand(&'static str),
    /// The stage is first in its pipeline, or the operand of `xargs`.
    First,
    /// The stage is first, and an argument contains this text.
    FirstWith(&'static str),
    /// The stage is first, and a leading short flag contains this letter.
    FirstFlag(char),
    /// The stage writes a file.
    Writes,
    /// The stage edits in place with `-i` or `--in-place`.
    InPlace,
    /// A leading short flag contains this letter.
    ShortFlag(char),
    /// The arguments show a diff.
    ShowsDiff,
}

impl Pattern {
    fn matches(self, p: &[String], first: bool) -> bool {
        let args = &p[1..];
        match self {
            Self::Any => true,
            Self::Subcommand(sub) => args.first().is_some_and(|a| a.as_str() == sub),
            Self::First => first,
            Self::FirstWith(text) => first && args.iter().any(|a| a.contains(text)),
            Self::FirstFlag(c) => first && has_short_flag(args, c),
            Self::Writes => writes_file(p),
            Self::InPlace => {
                has_short_flag(args, 'i') || args.iter().any(|a| a.starts_with("--in-place"))
            }
            Self::ShortFlag(c) => has_short_flag(args, c),
            Self::ShowsDiff => git_shows_diff(args),
        }
    }
}

/// Program, pattern and category of each classification row. The first matching row wins, so
/// stricter rows come before the looser ones for the same program.
const RULES: &[(&str, Pattern, &str)] = &[
    ("acus", Pattern::Subcommand("find"), "acus:find"),
    ("acus", Pattern::Subcommand("outline"), "acus:outline"),
    ("acus", Pattern::Subcommand("show"), "acus:show"),
    ("acus", Pattern::Subcommand("patch"), "acus:patch"),
    ("acus", Pattern::Subcommand("ctx"), "acus:ctx"),
    ("acus", Pattern::Subcommand("diff"), "acus:diff"),
    ("acus", Pattern::Subcommand("run"), "acus:run"),
    ("acus", Pattern::Any, "acus:other"),
    ("grep", Pattern::First, "search"),
    ("egrep", Pattern::First, "search"),
    ("fgrep", Pattern::First, "search"),
    ("rg", Pattern::First, "search"),
    ("ag", Pattern::First, "search"),
    ("ack", Pattern::First, "search"),
    ("find", Pattern::First, "search"),
    ("fd", Pattern::First, "search"),
    ("cat", Pattern::Writes, "edit-shell"),
    ("printf", Pattern::Writes, "edit-shell"),
    ("echo", Pattern::Writes, "edit-shell"),
    ("tee", Pattern::Writes, "edit-shell"),
    ("cat", Pattern::First, "read"),
    ("head", Pattern::First, "read"),
    ("tail", Pattern::First, "read"),
    ("less", Pattern::First, "read"),
    ("bat", Pattern::First, "read"),
    ("nl", Pattern::First, "read"),
    ("awk", Pattern::FirstWith("print"), "read"),
    ("sed", Pattern::InPlace, "edit-sed"),
    ("sed", Pattern::FirstFlag('n'), "read"),
    ("perl", Pattern::ShortFlag('i'), "edit-sed"),
    ("apply_patch", Pattern::Any, "edit-builtin"),
    ("git", Pattern::ShowsDiff, "diff"),
];

/// Every category a shell command hits, once each, with the shape of the pipeline that triggered
/// it.
pub(crate) fn classify_shaped(cmd: &str) -> Vec<(&'static str, Option<String>)> {
    let mut hits: Vec<(&'static str, Option<String>)> = vec![];
    for pipe in pipelines(cmd) {
        let shape = shape_pipe(&pipe);
        for (n, stage) in pipe.iter().enumerate() {
            let mut p = program(stage);
            // `find … | xargs grep` searches like a first stage.
            let mut first = n == 0;
            if p.first().is_some_and(|w| base(w) == "xargs") {
                let skip = p[1..].iter().take_while(|a| a.starts_with('-')).count();
                p = program(&p[1 + skip..]);
                first = true;
            }
            if let Some(category) = stage_category(cmd, &pipe[n + 1..], p, first) {
                add_hit(&mut hits, category, &shape);
            }
        }
    }
    hits
}

/// The category of one stage: the first matching row of `RULES`, else Python, else a build tool,
/// which depend on the whole command.
fn stage_category(
    cmd: &str,
    rest: &[Vec<String>],
    p: &[String],
    first: bool,
) -> Option<&'static str> {
    let name = base(p.first()?);
    if let Some(&(_, _, category)) = RULES
        .iter()
        .find(|(program, pattern, _)| *program == name && pattern.matches(p, first))
    {
        return Some(category);
    }
    if name == "python" || name.starts_with("python3") || name == "python2" {
        return Some(python_category(cmd));
    }
    is_build(p).then(|| build_category(rest))
}

/// Whether the commands after a build tool filter its output with `grep`, `rg`, `tail` or `head`.
fn build_category(rest: &[Vec<String>]) -> &'static str {
    let filtered = rest.iter().any(|s| {
        matches!(
            program(s).first().map(|w| base(w)),
            Some("grep" | "egrep" | "rg" | "tail" | "head")
        )
    });
    if filtered {
        "build-filtered"
    } else {
        "build-unfiltered"
    }
}

fn add_hit(hits: &mut Vec<(&'static str, Option<String>)>, category: &'static str, shape: &str) {
    if !hits.iter().any(|(c, _)| *c == category) {
        let command = if shape.is_empty() {
            None
        } else {
            Some(shape.to_owned())
        };
        hits.push((category, command));
    }
}
