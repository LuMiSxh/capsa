// SPDX-License-Identifier: MPL-2.0
//! PreToolUse hook for Claude Code and Codex. It rewrites a shell command into the acus
//! command that does the same work, adds a hint that names the acus command to use, or refuses
//! the call and names its replacement.
//!
//! Each module checks one kind of command: `ast` splits a command line into segments, `shell`
//! splits a segment into words, `scope` decides what is inside the repository, and `rules` holds
//! the refusals. `search`, `files`, `build`, `read`, `scripts` and `git` hold the checks,
//! `rewrite` applies them to whole command lines, and `convert` turns grep patterns into Rust
//! regexes.

mod acus_pipe;
mod ast;
mod build;
mod convert;
mod file_write;
mod files;
mod git;
pub mod hints;
pub mod log;
mod read;
mod replay;
mod rewrite;
mod rules;
mod scope;
mod scripts;
mod search;
mod sed_patch;
mod shell;
mod task_log;
#[cfg(test)]
mod tests;

use ast::*;
use build::*;
use file_write::*;
use files::*;
use git::*;
use read::*;
use rewrite::*;
use rules::*;
use scope::*;
use scripts::*;
use search::*;
use shell::*;

pub use read::read_check;
pub use replay::{ReplayError, replay_file};
pub use rewrite::Rewrite;
pub use rules::Rule;
pub use scope::Scope;
pub use search::grep_tool;

use acus_syntax::{Lang, outline};
use regex::Regex;
use serde_json::Value;

use std::path::{Path, PathBuf};
use std::sync::LazyLock;

/// Names where the hook keeps its files: the decision log and the per-session hint counts.
#[derive(Debug, Clone)]
pub struct GuardConfig {
    /// A repeated hint shows on its first occurrence and then every this many occurrences.
    pub hint_every: usize,
    /// Directory of the per-session hint counts.
    pub state_dir: PathBuf,
    /// The JSON-lines decision log.
    pub log_path: PathBuf,
}

/// Names what the hook does with a Bash call.
#[derive(Debug)]
pub enum BashDecision {
    Rewrite(Rewrite),
    Verdict(Verdict),
    Pass,
}

/// Decides one Bash call. A rewrite applies only when `can_rewrite` is set (Codex hooks cannot
/// rewrite, only refuse); otherwise the rules give a verdict, or the call passes.
pub fn decide_bash(cmd: &str, scope: Option<&Scope>, can_rewrite: bool) -> BashDecision {
    // A command that does not parse runs unchanged. The rewrite and the checks share one parse.
    let Some(p) = parse(cmd, Dialect::Bash) else {
        return BashDecision::Pass;
    };
    let rewritten = can_rewrite
        .then(|| rewrite_parsed(cmd, &p, scope))
        .flatten();
    if let Some(rw) = rewritten {
        return BashDecision::Rewrite(rw);
    }
    match check_parsed(cmd, &p, false, scope) {
        Some(v) => BashDecision::Verdict(v),
        None => BashDecision::Pass,
    }
}

/// Returns the verdict for the Glob tool, which only ever hints.
pub fn glob_tool() -> Verdict {
    Verdict::Hint(Rule::GlobTool, GLOB_HINT.to_owned())
}

/// Sets the longest call the log keeps, so a logged call can be replayed whole (patches and
/// heredocs past it are cut).
pub const LOG_CALL_MAX: usize = 8000;

/// Holds the verdict on a call. Each one names the rule that produced it, for the log.
#[derive(Debug, PartialEq)]
pub enum Verdict {
    /// Refuse the call; the reason names the acus command to use.
    Deny(Rule, String),
    /// Run the call, but tell the agent about the acus alternative.
    Hint(Rule, String),
    /// A call the rule would refuse, run anyway through the escape; only logged.
    Escape(Rule),
    /// A call run in its rewritten form; only logged (`run` answers it).
    Rewrite(Rule),
}

impl Verdict {
    fn rule(&self) -> Rule {
        match self {
            Verdict::Deny(r, _)
            | Verdict::Hint(r, _)
            | Verdict::Escape(r)
            | Verdict::Rewrite(r) => *r,
        }
    }

    fn kind(&self) -> &'static str {
        match self {
            Verdict::Deny(..) => "deny",
            Verdict::Hint(..) => "hint",
            Verdict::Escape(_) => "escape",
            Verdict::Rewrite(_) => "rewrite",
        }
    }

    /// Returns the rule of a refusal.
    fn denied(&self) -> Option<Rule> {
        match self {
            Verdict::Deny(r, _) => Some(*r),
            _ => None,
        }
    }

    fn escaped(self) -> Verdict {
        Verdict::Escape(self.rule())
    }
}

const ESCAPE: &str = "Only if acus cannot do this, rerun with a `command ` prefix.";
const ESCAPE_PS: &str = "Only if acus cannot do this, rerun with `# acus-skip` at the end.";
const GLOB_HINT: &str = "acus hint: `acus files [DIR] -g 'GLOB'` lists the files matching a glob (sorted, .gitignore respected, capped with a note), `acus map [DIR]` shows the layout with file and line counts, `acus outline DIR --depth 1` lists a directory's source files with their top-level symbols, and `acus find -l 'PAT'` lists the files containing a text.";
const TREE_HINT: &str = "acus hint: `acus files [DIR] -g 'GLOB'` lists the files matching a glob (sorted, .gitignore respected, capped with a note), `acus map [DIR]` shows the tree with file and line counts within a token budget, rolls up deep paths and lists the top-level symbols of small directories.";
/// Checks PowerShell with the same rules (cmdlets ignore case, so the rules see a lowercased
/// command), plus `-replace … | Set-Content` edits and .NET file reads.
pub fn check_ps(cmd: &str, scope: Option<&Scope>) -> Option<Verdict> {
    let c = cmd.to_lowercase();
    let words = |is_path: fn(&str) -> bool| {
        c.split(|ch: char| ch.is_whitespace() || "()'\",;".contains(ch))
            .filter(move |w| is_path(w))
    };
    let writes = [
        "set-content",
        "out-file",
        "writealltext",
        "writealllines",
        "| sc ",
        " > ",
    ]
    .iter()
    .any(|w| c.contains(w));
    let replaces = ["-replace", "-creplace", "-ireplace", ".replace("]
        .iter()
        .any(|w| c.contains(w));
    let v = if replaces && writes && words(is_code_path).next().is_some() {
        let files: Vec<&str> = words(is_code_path).collect();
        Verdict::Deny(
            Rule::PsReplace,
            format!(
                "Nothing ran: the command was refused as a whole because it rewrites {} with -replace. Use one `acus patch` with `*** Replace All: {}` and a SEARCH/REPLACE block (or SEARCH/REPLACE blocks under `*** Update File:`): it edits every file at once and prints the written lines. {ESCAPE_PS}",
                files.join(", "),
                files.join(" ")
            ),
        )
    } else if ["readalltext", "readalllines", "readlines("]
        .iter()
        .any(|w| c.contains(w))
        && words(is_source_path).any(|w| inside(scope, w))
        && !c.contains("acus ")
    {
        Verdict::Deny(
            Rule::ReadShell,
            format!(
                "Nothing ran: the command was refused as a whole because it reads source files through .NET. Use acus instead: `acus show path path:A-B path#Symbol` reads several files, ranges or symbols in one call; `acus outline PATH` gives an overview. {ESCAPE_PS}"
            ),
        )
    } else {
        match check_in(cmd, true, scope)? {
            Verdict::Deny(r, reason) => Verdict::Deny(r, reason.replace(ESCAPE, ESCAPE_PS)),
            v => v,
        }
    };
    Some(if c.contains("acus-skip") {
        v.escaped()
    } else {
        v
    })
}

/// Returns what to do with `cmd`: refusing wins over hinting, `None` lets it run.
#[cfg(test)]
fn check(cmd: &str) -> Option<Verdict> {
    check_in(cmd, false, None)
}

/// Checks Bash, or PowerShell where cmdlets ignore case: the rules see a lowercased command and a
/// refusal quotes the original.
fn check_in(cmd: &str, ps: bool, scope: Option<&Scope>) -> Option<Verdict> {
    // A command that does not parse runs unchanged.
    let p = parse(
        cmd,
        if ps {
            Dialect::PowerShell
        } else {
            Dialect::Bash
        },
    )?;
    check_parsed(cmd, &p, ps, scope)
}

/// Runs `check_in` for a command that is already parsed, so `decide_bash` parses it once.
fn check_parsed(cmd: &str, p: &Parsed, ps: bool, scope: Option<&Scope>) -> Option<Verdict> {
    let Parsed { segs, docs } = p;
    let mut found = segment_verdicts(cmd, segs, ps, scope);
    found.extend(filtered_verdicts(segs, ps));
    found.extend(script_hints(docs, scope).into_iter().map(|v| (v, 0, 0)));
    found.extend(write_verdicts(segs, ps, scope));
    // The refusals by the segment each names: a compound refusal names one that has no rewrite.
    let refusals: Vec<(usize, Rule, String)> = found
        .iter()
        .filter_map(|(v, at, _)| match v {
            Verdict::Deny(rule, why) => Some((*at, *rule, why.clone())),
            _ => None,
        })
        .collect();
    let (v, from, to) = found.into_iter().min_by_key(|(v, ..)| match v {
        Verdict::Deny(..) => 0,
        Verdict::Hint(..) => 1,
        Verdict::Escape(_) | Verdict::Rewrite(_) => 2,
    })?;
    let compound = segs.iter().filter(|s| !s.low.trim().is_empty()).count() > 1;
    let v = match v {
        // A refusal stops the whole command, so a compound one says which part and what to run.
        Verdict::Deny(rule, reason) if compound => {
            compound_refusal(cmd, segs, &refusals, scope, ps, (rule, reason), (from, to))
        }
        v => v,
    };
    Some(escaped_if(cmd.contains("acus-skip"), v))
}

/// A verdict with the first and last segment it is about.
type Found = (Verdict, usize, usize);

/// Returns the verdicts about single segments: a read, search or build that reaches the agent.
fn segment_verdicts(cmd: &str, segs: &[Segment], ps: bool, scope: Option<&Scope>) -> Vec<Found> {
    segs.iter()
        .enumerate()
        .filter_map(|(i, s)| {
            // Inside a loop or group the output still reaches the agent, so a read or search there
            // gets a hint (`loop_hint`). Captured and piped output in a group runs unchanged.
            if s.captured() || (s.nest != Nest::Top && s.out != Out::Agent) {
                return None;
            }
            let (esc, seg) = s.low_unescaped();
            // Output that goes into a pipe is not read by the model: it runs as it is, except
            // `cat FILE | head -N`, which `acus show FILE:1-N` replaces.
            // A display pipe (`| head`, `| grep`, `| sort`) hands the agent what its first stage
            // shows, so it is not a consumer that takes the output away.
            let feeds = segs.get(i + 1).is_some_and(|n| {
                s.feeds(n)
                    && (ps
                        || n.opaque()
                        || continues(segs, i + 1)
                        || (cat_head(seg, &n.low).is_none() && !shown_by_filters(segs, i)))
            });
            let v = rule(seg, s.piped(), ps, scope, feeds)?;
            let v = if s.nest == Nest::Top {
                v
            } else {
                loop_hint(v, seg, scope)?
            };
            // Only a refusal becomes a hint, and an escaped one stays an escape.
            let v = if esc { v } else { expansion_hint(v, seg) };
            // `cat FILE | head -N` is one refusal naming both stages. `|&` is not `|`.
            let read = v.rule() == Rule::ReadShell
                && segs.get(i + 1).is_some_and(|n| {
                    s.feeds(n)
                        && !n.opaque()
                        && cmd
                            .get(s.range.end..n.range.start)
                            .is_some_and(|g| g.trim() == "|")
                        && filter_name(&n.low)
                            .is_some_and(|f| matches!(f.as_str(), "head" | "tail"))
                });
            Some((escaped_if(esc, v), i, if read { i + 1 } else { i }))
        })
        .collect()
}

/// Returns the verdicts about a build piped into a filter, found for each pair of segments.
fn filtered_verdicts(segs: &[Segment], ps: bool) -> Vec<Found> {
    segs.windows(2)
        .enumerate()
        .filter_map(|(i, w)| {
            if w[0].opaque() || w[1].opaque() {
                return None;
            }
            // A build piped on into anything but a filter does not reach the model as it is.
            let build = if feeds_other(segs, i + 1) {
                None
            } else {
                let last = filter_last(segs, i);
                filtered_build(
                    &w[0],
                    &w[1],
                    &segs[i + 2..=last.max(i + 1)],
                    continues(segs, last),
                )
                .map(|v| (escaped_if(w[0].low_unescaped().0, v), i, i + 1))
            };
            build.or_else(|| {
                let v = listed_search(&w[0], &w[1], ps)?;
                Some((escaped_if(w[0].orig_unescaped().0, v), i, i + 1))
            })
        })
        .collect()
}

/// Returns the verdicts about shell writes into source files. They run before the capture check
/// of `segment_verdicts`, because a redirected segment is captured. Inside a loop or group a
/// refusal becomes a hint.
fn write_verdicts(segs: &[Segment], ps: bool, scope: Option<&Scope>) -> Vec<Found> {
    if ps {
        return Vec::new();
    }
    segs.iter()
        .enumerate()
        .filter_map(|(i, s)| {
            let (esc, seg) = s.low_unescaped();
            let v = file_write(seg, scope)?;
            let v = if s.nest == Nest::Top {
                v
            } else {
                loop_hint(v, seg, scope)?
            };
            Some((escaped_if(esc, v), i, i))
        })
        .collect()
}

/// Returns the verdicts for the edit scripts that an interpreter runs from a heredoc or inline
/// body.
fn script_hints(docs: &[Doc], scope: Option<&Scope>) -> Vec<Verdict> {
    docs.iter()
        .filter_map(|Doc { openers, body }| {
            let interp = openers.iter().find_map(|opener| {
                head(opener)
                    .map(|(h, _)| h.to_owned())
                    .filter(|h| is_interpreter(h))
                    .or_else(|| script_writer(opener).map(str::to_owned))
            })?;
            script_verdict(&interp, body, scope)
        })
        .collect()
}

/// Returns v marked as escaped when esc is set.
fn escaped_if(esc: bool, v: Verdict) -> Verdict {
    if esc { v.escaped() } else { v }
}
