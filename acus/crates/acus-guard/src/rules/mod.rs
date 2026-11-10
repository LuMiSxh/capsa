// SPDX-License-Identifier: MPL-2.0

//! Owns the rules that refuse a command and name the acus command that replaces it.

use super::*;

mod show;

use show::*;

/// Names every rule. `as_str` is what `guard.jsonl` and `census usage --guard` record, so the
/// spellings must not change.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rule {
    Grep,
    GrepTool,
    ReadShell,
    ReadTool,
    BuildPipe,
    CtxPipe,
    SedI,
    Tree,
    GlobTool,
    PsReplace,
    EditScript,
    GitStatus,
    GitDiff,
    GitLog,
    GitShow,
    AcusPipe,
    TaskLog,
    PatchQuiet,
    FileWrite,
}

impl Rule {
    #[cfg(test)]
    pub(super) const ALL: [Rule; 19] = [
        Rule::Grep,
        Rule::GrepTool,
        Rule::ReadShell,
        Rule::ReadTool,
        Rule::BuildPipe,
        Rule::CtxPipe,
        Rule::SedI,
        Rule::Tree,
        Rule::GlobTool,
        Rule::PsReplace,
        Rule::EditScript,
        Rule::GitStatus,
        Rule::GitDiff,
        Rule::GitLog,
        Rule::GitShow,
        Rule::AcusPipe,
        Rule::TaskLog,
        Rule::PatchQuiet,
        Rule::FileWrite,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Rule::Grep => "grep",
            Rule::GrepTool => "grep-tool",
            Rule::ReadShell => "read-shell",
            Rule::ReadTool => "read-tool",
            Rule::BuildPipe => "build-pipe",
            Rule::CtxPipe => "ctx-pipe",
            Rule::SedI => "sed-i",
            Rule::Tree => "tree",
            Rule::GlobTool => "glob-tool",
            Rule::PsReplace => "ps-replace",
            Rule::EditScript => "edit-script",
            Rule::GitStatus => "git-status",
            Rule::GitDiff => "git-diff",
            Rule::GitLog => "git-log",
            Rule::GitShow => "git-show",
            Rule::AcusPipe => "acus-pipe",
            Rule::TaskLog => "task-log",
            Rule::PatchQuiet => "patch-quiet",
            Rule::FileWrite => "file-write",
        }
    }

    /// Returns the rule a verdict's name stands for. Only the tests look rules up by name.
    #[cfg(test)]
    pub(super) fn parse(name: &str) -> Option<Rule> {
        Rule::ALL.into_iter().find(|r| r.as_str() == name)
    }
}

/// A search (`grep`, `rg`, `Select-String`, …) of source files whose output the model reads.
fn searches_source(head: &str, args: &[&str], in_pipe: bool, via_xargs: bool) -> bool {
    if !GREPS.contains(&head) || in_pipe || redirects_stdout(args) || is_quiet(args) {
        return false;
    }
    let s = parse_search(head, args);
    !is_count(&s) && aims_at_source(&s, via_xargs)
}

/// `sed -i` and `perl -i`, which edit files in place.
fn edits_in_place(head: &str, args: &[&str]) -> bool {
    matches!(head, "sed" | "perl")
        && args.iter().any(|a| {
            *a == "--in-place" || (a.starts_with('-') && !a.starts_with("--") && a.contains('i'))
        })
}

/// `head -c N` and `tail -c N` count bytes, which no line range of acus says.
fn counts_bytes(head: &str, args: &[&str]) -> bool {
    matches!(head, "head" | "tail")
        && args
            .iter()
            .any(|a| a.starts_with("-c") || a.starts_with("--bytes"))
}

/// A `cat`, `head`, `sed -n` or `Get-Content` read of a source file whose output the model
/// reads.
fn reads_source(head: &str, args: &[&str], in_pipe: bool, scope: Option<&Scope>) -> bool {
    // JSON, TOML and YAML are data, read whole on purpose; they do not count as source here.
    // Neither do files outside the repository.
    let names_source = || {
        args.iter().any(|a| {
            unquote(a)
                .split(',')
                .any(|p| is_source_path(p) && inside(scope, p))
        })
    };
    // Following a file is a wait, not a read.
    let follows = || {
        args.iter()
            .any(|a| matches!(*a, "-f" | "-F" | "-wait" | "--follow"))
    };
    matches!(
        head,
        "sed"
            | "cat"
            | "head"
            | "tail"
            | "nl"
            | "bat"
            | "less"
            | "more"
            | "get-content"
            | "gc"
            | "type"
    ) && !in_pipe
        && names_source()
        && !redirects_stdout(args)
        && (head != "sed" || args.contains(&"-n"))
        && !follows()
        && !counts_bytes(head, args)
}

/// A grep or rg whose pattern or path the shell expands runs as written: `acus find` cannot take
/// the expanded words, so the agent expands them first.
const EXPAND_HINT: &str = "acus hint: `acus find 'PAT' PATH` takes the pattern directly; expand the variable yourself first.";

/// `piped` is whether the command reads a pipe and `feeds` whether its output goes into one:
/// then the output does not reach the model as it is, and nothing is refused or converted.
pub(super) fn rule(
    seg: &str,
    mut piped: bool,
    ps: bool,
    scope: Option<&Scope>,
    feeds: bool,
) -> Option<Verdict> {
    let (mut head, mut words) = head(seg)?;
    if is_interpreter(head) {
        // Inline `-c` / `-e` scripts sit in the segment itself.
        return script_verdict(head, seg, scope);
    }
    let via_xargs = head == "xargs";
    if via_xargs {
        // `… | xargs grep` searches the files it is given.
        head = command_name(words.by_ref().find(|w| !w.starts_with('-'))?);
        piped = false;
    }
    let args: Vec<&str> = words.collect();
    // `find … -exec grep …` searches the files it finds.
    if head == "find"
        && let Some(i) = args.iter().position(|a| matches!(*a, "-exec" | "-execdir"))
        && let Some(&exec) = args.get(i + 1).filter(|c| GREPS.contains(&command_name(c)))
    {
        return Some(Verdict::Deny(Rule::Grep, grep_reason(command_name(exec))));
    }
    if is_listing(head, &args, ps) {
        return Some(Verdict::Hint(Rule::Tree, TREE_HINT.into()));
    }
    if head == "git" {
        // A short log that feeds a filter is data for that filter.
        return git_hint(&args).filter(|_| !(feeds && args.contains(&"--oneline")));
    }
    // PowerShell cmdlets and aliases (lowercased by `check_ps`) share the rules.
    let in_pipe = piped || feeds;
    if matches!(head, "fd" | "fdfind") && !in_pipe {
        return Some(Verdict::Hint(Rule::GlobTool, FD_HINT.into()));
    }
    let (rule, reason) = if searches_source(head, &args, in_pipe, via_xargs) {
        (Rule::Grep, grep_reason(head))
    } else if edits_in_place(head, &args) {
        (
            Rule::SedI,
            format!(
                "Use acus instead of {head} -i: one `acus patch` with `*** Replace All: PATHS` or SEARCH/REPLACE blocks edits every file at once and prints the written lines. {ESCAPE}"
            ),
        )
    } else if reads_source(head, &args, in_pipe, scope) {
        // A sed script of regex addresses has no line range: a search reads the same lines.
        let regex_script = head == "sed"
            && args
                .iter()
                .find(|a| !a.starts_with('-'))
                .is_some_and(|s| sed_lines(unquote(s)).is_none() && sed_from(s).is_none());
        let reason = if regex_script {
            format!(
                "Use acus instead of sed with a regex address: `acus find 'RE' PATH` lists the matching lines with their enclosing symbol, and `acus show PATH#Symbol` reads one symbol. {ESCAPE}"
            )
        } else {
            format!(
                "Use acus instead of {head}: `acus show path path:A-B path#Symbol` reads several files, ranges or symbols in one call; `acus outline PATH` gives an overview. {ESCAPE}"
            )
        };
        (Rule::ReadShell, reason)
    } else {
        return None;
    };
    Some(Verdict::Deny(rule, reason))
}

/// Turns a grep or rg that the shell expands into a hint where it was a refusal: `acus find`
/// cannot take the expanded words, so the command runs as written. Applied to the verdict only,
/// never inside `rule`, because folds read `rule` and a rewrite must not be lost to a hint.
pub(super) fn expansion_hint(v: Verdict, seg: &str) -> Verdict {
    match v {
        Verdict::Deny(Rule::Grep, _) if expands_shell(seg) => {
            Verdict::Hint(Rule::Grep, EXPAND_HINT.into())
        }
        v => v,
    }
}

/// Returns the verdict for a segment inside a loop or group whose output reaches the agent. It
/// runs once per item, so a refusal becomes a hint and the command runs as it is: a read or search
/// gets the acus command that reads them in one call, and other refusals their reason. A hint
/// stays a hint.
/// `rule` only yields refusals and hints; rewrites and escapes are added later and never reach
/// here.
pub(super) fn loop_hint(v: Verdict, seg: &str, scope: Option<&Scope>) -> Option<Verdict> {
    match v {
        Verdict::Hint(r, text) => Some(Verdict::Hint(r, text)),
        Verdict::Deny(r, reason) => {
            let read = matches!(r, Rule::Grep | Rule::ReadShell).then_some(r);
            // The command the top level would have run, with the line range it names.
            let suggestion = read.and_then(|rule| {
                rewrite_read(rule, seg, scope)
                    .map(|s| format!("`{s}`"))
                    .or_else(|| suggest(rule, seg))
            });
            let text = match suggestion {
                Some(s) => format!(
                    "acus hint: this runs once per loop item; {s} reads them in one call and numbers the lines."
                ),
                None => format!("acus hint: {}", reason.replace(ESCAPE, "").trim()),
            };
            Some(Verdict::Hint(r, text))
        }
        Verdict::Rewrite(_) | Verdict::Escape(_) => None,
    }
}

/// Returns the refusal of a compound command. The whole command stops, so it names the refused
/// segment and what to run instead, then the segments refused without a rewrite and the command
/// that does the rewritable parts (`send_instead_parsed`). `refusals` are the refusals by segment
/// index.
pub(super) fn compound_refusal(
    cmd: &str,
    segs: &[Segment],
    refusals: &[(usize, Rule, String)],
    scope: Option<&Scope>,
    ps: bool,
    (rule, reason): (Rule, String),
    (from, to): (usize, usize),
) -> Verdict {
    // What to send instead, and the segments refused without a rewrite (Bash only).
    let sent = if ps {
        None
    } else {
        send_instead_parsed(cmd, segs, scope)
    };
    let (send, blocked) = sent.unwrap_or_default();
    // The refusal names the first refused segment that has no rewrite.
    let named = blocked
        .iter()
        .find_map(|b| refusals.iter().find(|(f, ..)| f == b));
    let (rule, reason, from, to) = match named {
        Some((f, r, why)) if !blocked.contains(&from) => (*r, why.clone(), *f, *f),
        _ => (rule, reason, from, to),
    };
    let at = |i: usize| segs[i].orig.as_str();
    let part = (from..=to).map(at).collect::<Vec<_>>().join(" | ");
    let part = one_line(&part);
    let instead = Some(rule)
        .and_then(|r| {
            let seg = at(from);
            // `acus files` exits 1 on no match, unlike `rg --files`.
            if segs[from].gates && lists_files(seg) {
                return None;
            }
            // `grep … | head -N` names the limit that makes the same read.
            if r == Rule::Grep
                && let Some(rewrite::Limited { find, limit, .. }) =
                    rewrite::limited_search(segs, from, scope)
            {
                let cmd = match limit {
                    Some(l) => format!("{find} {l}"),
                    None => find,
                };
                return Some(format!("`{cmd}`"));
            }
            // The range of `cat FILE | head -N` names that read, never the whole file.
            if r == Rule::ReadShell && from != to {
                return cat_head(segs[from].low_unescaped().1, &segs[to].low)
                    .and_then(|read| suggest(r, &read));
            }
            suggest(
                r,
                &if ps {
                    seg.to_owned()
                } else {
                    quote_escaped_spaces(seg)
                },
            )
        })
        .filter(|s| s.len() <= 240)
        .map_or("Replace that segment".to_owned(), |s| {
            format!("Replace it with {s}")
        });
    let mut lines = vec![format!(
        "Nothing ran: the whole command was refused because of `{part}`; the other segments are fine. {instead} and send the command again. {reason}"
    )];
    if let Some(send) = send {
        lines.push(format!("Send instead: {send}"));
    }
    for b in &blocked {
        let why = refusals
            .iter()
            .find(|(f, ..)| f == b)
            .map_or("refused", |(_, _, why)| {
                why.split(':').next().unwrap_or(why)
            });
        lines.push(format!("Blocked: `{}` — {why}", one_line(at(*b))));
    }
    Verdict::Deny(rule, lines.join("\n"))
}

/// Returns what to run instead of the segment a rule refused, when it can be worked out.
pub(super) fn suggest(rule: Rule, seg: &str) -> Option<String> {
    let (head, words) = head(seg)?;
    let head = head.to_lowercase();
    let args: Vec<&str> = words.collect();
    match rule {
        Rule::Grep if head == "rg" && args.contains(&"--files") => {
            rg_files(&args).map(|s| format!("`{s}`"))
        }
        Rule::Grep if GREPS.contains(&head.as_str()) => {
            let s = parse_search(&head, &args);
            (!s.inexact).then(|| format!("`{}`", find_cmd(&s)))
        }
        Rule::ReadShell => show_cmd(&head, &args),
        Rule::SedI => {
            let files: Vec<&str> = args
                .iter()
                .map(|a| unquote(a))
                .filter(|a| is_code_path(a))
                .collect();
            let files = if files.is_empty() {
                "PATHS".to_owned()
            } else {
                files.join(" ")
            };
            Some(format!(
                "`acus patch` with `*** Replace All: {files}` and a SEARCH/REPLACE block"
            ))
        }
        _ => None,
    }
}

#[cfg(test)]
mod rule_names {
    use super::Rule;

    /// The names are a contract with `guard.jsonl` and `census usage --guard`.
    #[test]
    fn keep_their_logged_spelling() {
        let names: Vec<&str> = Rule::ALL.iter().map(|r| r.as_str()).collect();
        assert_eq!(
            names,
            [
                "grep",
                "grep-tool",
                "read-shell",
                "read-tool",
                "build-pipe",
                "ctx-pipe",
                "sed-i",
                "tree",
                "glob-tool",
                "ps-replace",
                "edit-script",
                "git-status",
                "git-diff",
                "git-log",
                "git-show",
                "acus-pipe",
                "task-log",
                "patch-quiet",
                "file-write",
            ]
        );
        for r in Rule::ALL {
            assert_eq!(Rule::parse(r.as_str()), Some(r));
        }
    }

    /// Names the two folds here: a rename would split `guard.jsonl` counts.
    #[test]
    fn new_rule_names_parse_back() {
        for r in Rule::ALL {
            assert_eq!(Rule::parse(r.as_str()), Some(r));
        }
        assert_eq!(Rule::AcusPipe.as_str(), "acus-pipe");
        assert_eq!(Rule::TaskLog.as_str(), "task-log");
    }
}
