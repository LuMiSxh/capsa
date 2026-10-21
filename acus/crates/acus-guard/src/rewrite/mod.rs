// SPDX-License-Identifier: MPL-2.0

//! Rewrites whole command lines, replacing each rewritable segment in place.

use super::*;

mod fold;
mod span;

use fold::*;
pub(super) use span::*;

/// Holds a Bash command the guard would refuse, rewritten into the acus call that does the same.
#[derive(Debug)]
pub struct Rewrite {
    pub command: String,
    /// Pairs what was run instead of what, for the agent.
    pub note: String,
    pub rule: Rule,
    /// Marks that a build or test was rewritten, so the hook gives no permission decision.
    pub deferred: bool,
}

/// Replaces the segments `from..=last` by `new`.
struct Fold {
    last: usize,
    rule: Rule,
    new: String,
    /// Reports whether the replacement starts at the command word, so `FOO=1 cat a` keeps `FOO=1`.
    /// A build takes its assignments along into the quoted command instead.
    from_command: bool,
}

enum Step {
    /// Nothing to replace at this segment.
    Keep,
    Fold(Fold),
}

/// Rewrites every segment the guard would refuse, when each has an exact acus replacement;
/// returns `None` (refuse as usual) if any segment has none. A rewritten build is `deferred`: the
/// permission flow decides on it, as it does for a build the agent runs itself.
#[cfg(test)]
pub(super) fn rewrite(cmd: &str, scope: Option<&Scope>) -> Option<Rewrite> {
    rewrite_parsed(cmd, &parse(cmd, Dialect::Bash)?, scope)
}

/// Rewrites a command that is already parsed. Heredoc bodies and substitutions are not segments
/// of their own line, so they are never spliced.
pub(super) fn rewrite_parsed(cmd: &str, p: &Parsed, scope: Option<&Scope>) -> Option<Rewrite> {
    if cmd.contains("acus-skip") {
        return None;
    }
    let segs = &p.segs;
    // The splitter of a build without a Bash grammar knows no byte ranges to splice.
    if segs.iter().any(|s| s.range.is_empty()) {
        return None;
    }
    // A whole-command `sed -i` becomes an `acus patch` call. The note says how the two differ,
    // because the agent sees the result and a failed patch writes nothing where sed might have.
    // The replacement stands for the whole line, so nothing may sit outside the segment (a
    // comment, `&`). It must parse and pass the guard like `fold_all` results, or fall through.
    if let Some(command) = sed_patch::fold(segs, 0, scope)
        .filter(|_| cmd.trim() == segs[0].orig.trim())
        .filter(|c| {
            parse(c, Dialect::Bash).is_some()
                && !matches!(check_in(c, false, scope), Some(Verdict::Deny(..)))
        })
    {
        return Some(Rewrite {
            command,
            note: "acus guard: ran `acus patch` instead of `sed -i`. The command was rewritten, not refused; use acus directly next time. Unlike sed, acus patch fails and writes nothing when the text is not found, and refuses an edit that adds syntax errors.".to_string(),
            rule: Rule::SedI,
            deferred: true,
        });
    }
    let folded = fold_all(cmd, segs, scope, false)?;
    // The result must parse and pass the guard itself.
    if parse(&folded.out, Dialect::Bash).is_none()
        || matches!(check_in(&folded.out, false, scope), Some(Verdict::Deny(..)))
    {
        return None;
    }
    Some(Rewrite {
        command: folded.out,
        note: format!(
            "acus guard: {}. The command was rewritten, not refused; use acus directly next time.",
            folded.notes.join("; ")
        ),
        rule: folded.first?,
        deferred: folded.deferred,
    })
}

/// Returns, for a refused command, the command with each segment that has an exact acus
/// replacement replaced and the others as they are (`None` if no segment can be replaced), and the
/// indices of the segments the guard refuses without a replacement. `segs` is the parse of `cmd`.
pub(super) fn send_instead_parsed(
    cmd: &str,
    segs: &[Segment],
    scope: Option<&Scope>,
) -> Option<(Option<String>, Vec<usize>)> {
    if segs.iter().any(|s| s.range.is_empty()) {
        return None;
    }
    let folded = fold_all(cmd, segs, scope, true)?;
    let send = (!folded.notes.is_empty() && parse(&folded.out, Dialect::Bash).is_some())
        .then_some(folded.out);
    Some((send, folded.blocked))
}

/// Holds a command with its folds applied, and what each fold ran instead of.
struct Folded {
    out: String,
    notes: Vec<String>,
    first: Option<Rule>,
    deferred: bool,
    /// Lists the segments with no fold that are refused: kept as they are, only when `partial`.
    blocked: Vec<usize>,
}

/// Folds each segment of `segs` that has an exact acus replacement. A segment with none is refused
/// (`None`), unless `partial`: then it is kept as it is and listed in `blocked`.
fn fold_all(cmd: &str, segs: &[Segment], scope: Option<&Scope>, partial: bool) -> Option<Folded> {
    let (mut out, mut at, mut i) = (String::new(), 0, 0);
    let (mut notes, mut first, mut deferred, mut blocked) = (Vec::new(), None, false, Vec::new());
    // The pipeline folded just before, as (last segment, rule): an echo after it reads status.
    let mut piped: Option<(usize, Rule)> = None;
    while i < segs.len() {
        let echo = piped
            .filter(|&(end, _)| end + 1 == i)
            .and_then(|(_, rule)| Some((rule, exit_status_echo(cmd, segs, i)?)));
        let is_echo = echo.is_some();
        let step = match echo {
            Some((rule, new)) => Some(Step::Fold(Fold {
                last: i,
                rule,
                new,
                from_command: false,
            })),
            None => fold_at(segs, i, scope),
        };
        let fold = match step {
            Some(Step::Keep) => {
                i += 1;
                piped = None;
                continue;
            }
            Some(Step::Fold(fold)) if folds_run(cmd, segs, i, &fold, at) => fold,
            _ => {
                if !partial {
                    return None;
                }
                blocked.push(i);
                i += 1;
                piped = None;
                continue;
            }
        };
        let Fold {
            last,
            rule,
            new,
            from_command,
        } = fold;
        piped =
            (!is_echo && matches!(rule, Rule::BuildPipe | Rule::CtxPipe)).then_some((last, rule));
        let from = segs[i].range.start + if from_command { segs[i].lead } else { 0 };
        let to = segs[last].range.end;
        let old = one_line(&cmd[from..to]);
        notes.push(format!("ran `{new}` instead of `{old}`"));
        out.push_str(&cmd[at..from]);
        out.push_str(&new);
        deferred |= matches!(rule, Rule::BuildPipe | Rule::CtxPipe | Rule::SedI);
        (at, i, first) = (to, last + 1, first.or(Some(rule)));
    }
    out.push_str(&cmd[at..]);
    Some(Folded {
        out,
        notes,
        first,
        deferred,
        blocked,
    })
}

/// Reports whether segments `i..=fold.last` form one pipeline with no heredoc body between them,
/// and the fold starts at or after `at`, where the previous replacement ended.
fn folds_run(cmd: &str, segs: &[Segment], i: usize, fold: &Fold, at: usize) -> bool {
    let run = &segs[i..=fold.last];
    let joined = run.windows(2).all(|w| {
        cmd.get(w[0].range.end..w[1].range.start)
            .is_some_and(|gap| gap.trim() == "|")
    });
    let from = segs[i].range.start + if fold.from_command { segs[i].lead } else { 0 };
    let to = segs[fold.last].range.end;
    joined && !run.iter().any(|s| s.heredoc) && from >= at && cmd.get(from..to).is_some()
}

/// Returns the converted `acus find` of a search piped into a `head` or `tail` line count that
/// ends the pipeline (`grep x F | head -N`), and the limit flag that says the same. The limit is
/// `--max-hits N` for head and `--last N` for tail of one file. `-l` lists files, which
/// `--max-hits` does not cap, and head with context flags counts context lines too, so neither is
/// `exact`. `-o` counts matches, not lines, so it has no limit at all.
pub(super) struct Limited {
    pub(super) find: String,
    pub(super) limit: Option<String>,
    pub(super) exact: bool,
    pub(super) last: usize,
    pub(super) tail: &'static str,
}
