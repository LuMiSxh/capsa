// SPDX-License-Identifier: MPL-2.0
//! Drops the output lines that only cost tokens: progress, status and passing-test lines,
//! colour codes, `\r` redraws, repeats and nested summaries. Failures stay.

mod budget;
mod fold;
mod patterns;
mod profile;
mod summary;
#[cfg(test)]
mod tests;

pub(super) use self::budget::keep;
pub(super) use self::patterns::Stats;
use self::patterns::{Category, PY_LIB_FRAME};
pub(super) use self::profile::Profile;
pub(super) use self::summary::Summary;
use regex::{Regex, RegexSet};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

/// The kept lines and what was dropped, parsed and detected.
pub(super) struct Quiet {
    pub(super) lines: Vec<String>,
    pub(super) summary: Summary,
    pub(super) stats: Stats,
    pub(super) profile: Option<&'static str>,
}

/// Writes the full output to a temp file named after the directory and command, so a rerun
/// replaces its previous log.
pub(super) fn save_log(cwd: &Path, command: &str, text: &str) -> Option<PathBuf> {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    (cwd, command).hash(&mut h);
    let dir = std::env::temp_dir().join("acus-run");
    std::fs::create_dir_all(&dir).ok()?;
    let path = dir.join(format!("{:016x}.log", h.finish()));
    std::fs::write(&path, text).ok()?;
    Some(path)
}

/// The output as a terminal would show it: no colour codes, and only the final state of lines
/// a progress bar redrew with `\r`.
pub(super) fn clean(text: &str) -> String {
    #[expect(clippy::unwrap_used, reason = "a constant pattern")]
    static ANSI: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"\x1b(\[[0-9;?]*[ -/]*[@-~]|\][^\x07\x1b]*(\x07|\x1b\\)|[()][0-9A-B])").unwrap()
    });
    // `gh run view --log` lines start with the job, step and timestamp fields.
    #[expect(clippy::unwrap_used, reason = "a constant pattern")]
    static GH_PREFIX: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r"(?m)^[^\t\n]*\t[^\t\n]*\t\x{FEFF}?\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d+)?Z ?",
        )
        .unwrap()
    });
    let text = ANSI.replace_all(text, "");
    GH_PREFIX
        .replace_all(&text, "")
        .lines()
        .map(|l| l.rsplit('\r').find(|s| !s.is_empty()).unwrap_or(""))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Drops and folds the noise lines. `extra` is `[run] drop` plus `--drop`, and `explain` keeps
/// a count of each kind for `--explain`.
pub(super) fn quiet(lines: &[&str], extra: &RegexSet, explain: bool) -> Quiet {
    let mut summary = Summary::default();
    let mut stats = Stats::new(explain);
    let profile = Profile::detect(lines);
    let mut kept: Vec<&str> = Vec::new();
    let mut frame: Option<usize> = None;
    for &line in lines {
        // A Python library frame and the indented lines under it.
        let indent = line.len() - line.trim_start().len();
        if let Some(f) = frame {
            if indent > f && !line.trim().is_empty() {
                stats.add(Category::Framework);
                continue;
            }
            frame = None;
        }
        if let Some(c) = PY_LIB_FRAME.captures(line) {
            frame = Some(c[1].len());
            stats.add(Category::Framework);
            continue;
        }
        if let Some((s, merge)) = summary::green(line) {
            summary.merge(s, merge);
            stats.add(Category::Summary);
            continue;
        }
        if let Some((s, merge)) = summary::red(line) {
            summary.merge(s, merge);
        }
        if let Some(p) = profile {
            if p.skip(line) {
                stats.add(Category::Skip);
                continue;
            }
            if p.pass(line) {
                stats.add(Category::Pass);
                continue;
            }
        }
        if patterns::matches(line) {
            if explain && let Some(c) = patterns::category(line) {
                stats.add(c);
            }
            continue;
        }
        if extra.is_match(line) {
            stats.add(Category::Extra);
            continue;
        }
        if line.trim().is_empty() && kept.last().is_none_or(|p| p.trim().is_empty()) {
            stats.add(Category::Blank);
            continue;
        }
        kept.push(line);
    }
    Quiet {
        lines: fold::collapse(&kept, &mut stats),
        summary,
        stats,
        profile: profile.map(Profile::name),
    }
}
