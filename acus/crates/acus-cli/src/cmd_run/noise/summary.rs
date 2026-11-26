// SPDX-License-Identifier: MPL-2.0
//! Parses the green summary line of the common test runners into counts, so a passing run
//! collapses to its numbers instead of its summaries.

use serde::Serialize;

/// A run's test counts, from a summary line.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub(in crate::cmd_run) struct Summary {
    pub(in crate::cmd_run) passed: usize,
    pub(in crate::cmd_run) failed: usize,
    pub(in crate::cmd_run) skipped: usize,
}

/// How a summary line's counts join the run's: cargo and nextest report each binary or run once
/// (add), XCTest repeats the same total for every nesting level (take the largest).
#[derive(Clone, Copy, Debug)]
pub(super) enum Merge {
    Add,
    Max,
}

impl Summary {
    /// Folds a summary line into the run's counts.
    pub(super) fn merge(&mut self, other: Self, merge: Merge) {
        match merge {
            Merge::Add => {
                self.passed += other.passed;
                self.failed += other.failed;
                self.skipped += other.skipped;
            }
            Merge::Max => {
                self.passed = self.passed.max(other.passed);
                self.failed = self.failed.max(other.failed);
                self.skipped = self.skipped.max(other.skipped);
            }
        }
    }
}

/// The counts of a summary line that reports no failure or problem, with how to merge them.
#[expect(
    clippy::too_many_lines,
    reason = "one case per runner reads as one table"
)]
pub(super) fn green(line: &str) -> Option<(Summary, Merge)> {
    let l = line.trim();
    // cargo: `test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; …`
    if let Some(rest) = l.strip_prefix("test result: ok. ") {
        return Some((
            Summary {
                passed: count_in(rest, "passed").unwrap_or(0),
                failed: 0,
                skipped: count_in(rest, "ignored").unwrap_or(0),
            },
            Merge::Add,
        ));
    }
    // nextest: `Summary [   0.021s] 14 tests run: 14 passed, 177 skipped`
    if let Some(rest) = l.strip_prefix("Summary [") {
        let s = nextest_counts(nextest_counts_text(rest)?)?;
        return (s.failed == 0).then_some((s, Merge::Add));
    }
    // XCTest: `Executed 3 tests, with 0 failures (0 unexpected) in 0.003 (0.003) seconds`
    if let Some(rest) = l.strip_prefix("Executed ") {
        let s = xctest_counts(rest)?;
        return (s.failed == 0).then_some((s, Merge::Max));
    }
    // swift-testing: `✔ Test run with 3 tests passed after 0.002 seconds.`
    let rest = l.strip_prefix("✔ ").unwrap_or(l);
    if let Some(rest) = rest.strip_prefix("Test run with ")
        && rest.contains(" passed after ")
    {
        return Some((
            Summary {
                passed: leading_usize(rest)?,
                ..Summary::default()
            },
            Merge::Add,
        ));
    }
    // pytest: `3 passed, 1 skipped in 0.12s`, optionally around a banner of `=`.
    let banner = l.trim_matches(|c: char| c == '=' || c.is_whitespace());
    if let Some((counts, duration)) = banner.rsplit_once(" in ")
        && counts.contains(" passed")
        && !counts.contains("failed")
        && !counts.contains("error")
        && duration.ends_with('s')
    {
        return Some((
            Summary {
                passed: count_in(counts, "passed").unwrap_or(0),
                failed: 0,
                skipped: count_in(counts, "skipped").unwrap_or(0),
            },
            Merge::Add,
        ));
    }
    // bun: `3 pass` on one line and `0 fail` on another.
    if let Some(n) = l.strip_suffix(" pass").and_then(|p| p.trim().parse().ok()) {
        return Some((
            Summary {
                passed: n,
                ..Summary::default()
            },
            Merge::Add,
        ));
    }
    if l == "0 fail" {
        return Some((Summary::default(), Merge::Add));
    }
    // deno: `ok | 3 passed | 0 failed (10ms)`
    if let Some(rest) = l.strip_prefix("ok | ") {
        let (passed, tail) = rest.split_once(" passed | ")?;
        if tail.starts_with("0 failed") {
            return Some((
                Summary {
                    passed: passed.trim().parse().ok()?,
                    ..Summary::default()
                },
                Merge::Add,
            ));
        }
    }
    // rspec: `2 examples, 0 failures`; `1 example, 1 failure` stays.
    if let Some(rest) = l
        .strip_suffix(" failures")
        .or_else(|| l.strip_suffix(" failure"))
    {
        let (examples, n) = rest.rsplit_once(", ")?;
        if n.parse::<usize>().ok()? == 0 {
            return Some((
                Summary {
                    passed: leading_usize(examples)?,
                    ..Summary::default()
                },
                Merge::Add,
            ));
        }
    }
    // phpunit: `OK (3 tests, 5 assertions)`
    if let Some(rest) = l.strip_prefix("OK (") {
        return Some((
            Summary {
                passed: leading_usize(rest)?,
                ..Summary::default()
            },
            Merge::Add,
        ));
    }
    // ctest: `100% tests passed, 0 tests failed out of 3`
    if let Some(rest) = l.strip_prefix("100% tests passed, 0 tests failed out of ") {
        return Some((
            Summary {
                passed: leading_usize(rest)?,
                ..Summary::default()
            },
            Merge::Add,
        ));
    }
    // dotnet: `Passed!  - Failed:     0, Passed:     2, Skipped:     0, Total: 2, …`
    if let Some(rest) = l.strip_prefix("Passed!  - Failed:") {
        let failed = leading_usize(rest.trim_start())?;
        if failed > 0 {
            return None;
        }
        return Some((
            Summary {
                passed: field(rest, "Passed:")?,
                failed: 0,
                skipped: field(rest, "Skipped:").unwrap_or(0),
            },
            Merge::Add,
        ));
    }
    // vitest / jest: `Tests  3 passed (3)` or `Tests: 3 passed, 3 total`.
    if let Some(rest) = l.strip_prefix("Tests") {
        let rest = rest.trim_start_matches(|c: char| c == ':' || c.is_whitespace());
        if rest.contains(" passed")
            && !rest.contains(" failed")
            && let Some(n) = leading_usize(rest)
        {
            return Some((
                Summary {
                    passed: n,
                    ..Summary::default()
                },
                Merge::Add,
            ));
        }
    }
    // mocha: `3 passing (10ms)`; a `failing` line keeps the run.
    if let Some((n, _)) = l.split_once(" passing")
        && !l.contains(" failing")
    {
        return Some((
            Summary {
                passed: n.trim().parse().ok()?,
                ..Summary::default()
            },
            Merge::Add,
        ));
    }
    // node:test: `# pass 3` and `# fail 0`.
    if let Some(n) = l.strip_prefix("# pass ") {
        return Some((
            Summary {
                passed: n.trim().parse().ok()?,
                ..Summary::default()
            },
            Merge::Add,
        ));
    }
    if l == "# fail 0" {
        return Some((Summary::default(), Merge::Add));
    }
    None
}

/// The counts of a summary line that reports a failure, so the footer and JSON still carry the
/// numbers while the line itself stays. Returns `None` when the line reports no failure.
#[expect(
    clippy::too_many_lines,
    reason = "one case per runner reads as one table"
)]
pub(super) fn red(line: &str) -> Option<(Summary, Merge)> {
    let l = line.trim();
    // cargo: `test result: FAILED. 2 passed; 1 failed; 0 ignored; …`
    if let Some(rest) = l.strip_prefix("test result: FAILED. ") {
        return Some((
            Summary {
                passed: count_in(rest, "passed").unwrap_or(0),
                failed: count_in(rest, "failed").unwrap_or(0),
                skipped: count_in(rest, "ignored").unwrap_or(0),
            },
            Merge::Add,
        ));
    }
    // nextest: `Summary [ …] 2 tests run: 1 passed, 1 failed, 0 skipped`
    if let Some(rest) = l.strip_prefix("Summary [") {
        let s = nextest_counts(nextest_counts_text(rest)?)?;
        return (s.failed > 0).then_some((s, Merge::Add));
    }
    // XCTest: `Executed 3 tests, with 1 failure (0 unexpected) in …`
    if let Some(rest) = l.strip_prefix("Executed ") {
        let s = xctest_counts(rest)?;
        return (s.failed > 0).then_some((s, Merge::Max));
    }
    // swift-testing: `✘ Test run with 3 tests failed after … with 1 issue.`
    let rest = l.strip_prefix("✘ ").unwrap_or(l);
    if let Some(rest) = rest.strip_prefix("Test run with ") {
        let total = leading_usize(rest)?;
        let failed = rest
            .split_once(" with ")
            .and_then(|(_, tail)| leading_usize(tail))
            .unwrap_or(0);
        if failed > 0 {
            return Some((
                Summary {
                    passed: total.saturating_sub(failed),
                    failed,
                    skipped: 0,
                },
                Merge::Add,
            ));
        }
    }
    // pytest: `1 failed, 2 passed in 0.12s`
    let banner = l.trim_matches(|c: char| c == '=' || c.is_whitespace());
    if let Some((counts, duration)) = banner.rsplit_once(" in ")
        && (counts.contains("failed") || counts.contains("error"))
        && duration.ends_with('s')
    {
        let errors = count_in(counts, "error").unwrap_or(0);
        return Some((
            Summary {
                passed: count_in(counts, "passed").unwrap_or(0),
                failed: count_in(counts, "failed").unwrap_or(0) + errors,
                skipped: count_in(counts, "skipped").unwrap_or(0),
            },
            Merge::Add,
        ));
    }
    // deno: `FAILED | 1 passed | 1 failed (10ms)`
    if let Some(rest) = l.strip_prefix("FAILED | ") {
        let (passed, tail) = rest.split_once(" passed | ")?;
        return Some((
            Summary {
                passed: passed.trim().parse().ok()?,
                failed: leading_usize(tail)?,
                skipped: 0,
            },
            Merge::Add,
        ));
    }
    // rspec: `3 examples, 1 failure`
    if let Some(rest) = l
        .strip_suffix(" failures")
        .or_else(|| l.strip_suffix(" failure"))
    {
        let (examples, n) = rest.rsplit_once(", ")?;
        let failed: usize = n.parse().ok()?;
        if failed > 0 {
            let total = leading_usize(examples)?;
            return Some((
                Summary {
                    passed: total.saturating_sub(failed),
                    failed,
                    skipped: 0,
                },
                Merge::Add,
            ));
        }
    }
    // ctest: `50% tests passed, 1 tests failed out of 3`
    if let Some((before, total)) = l.rsplit_once(" tests failed out of ") {
        let failed = leading_usize(before.rsplit(' ').next()?)?;
        if failed > 0 {
            let total = leading_usize(total)?;
            return Some((
                Summary {
                    passed: total.saturating_sub(failed),
                    failed,
                    skipped: 0,
                },
                Merge::Add,
            ));
        }
    }
    // dotnet: `Failed!  - Failed:     1, Passed:     2, Skipped:     0, …`
    if let Some(rest) = l.strip_prefix("Failed!  - Failed:") {
        let failed = leading_usize(rest.trim_start())?;
        if failed > 0 {
            return Some((
                Summary {
                    passed: field(rest, "Passed:")?,
                    failed,
                    skipped: field(rest, "Skipped:").unwrap_or(0),
                },
                Merge::Add,
            ));
        }
    }
    // node:test: `# fail 1`
    if let Some(n) = l.strip_prefix("# fail ") {
        let failed: usize = n.trim().parse().ok()?;
        if failed > 0 {
            return Some((
                Summary {
                    failed,
                    ..Summary::default()
                },
                Merge::Add,
            ));
        }
    }
    None
}

/// The counts after `]` in a nextest `Summary` line.
fn nextest_counts_text(rest: &str) -> Option<&str> {
    Some(rest.split_once(']')?.1.trim_start().split_once(": ")?.1)
}

/// Parses `14 passed, 1 failed, 3 skipped`, summing the problem categories.
fn nextest_counts(counts: &str) -> Option<Summary> {
    let mut s = Summary::default();
    for part in counts.split(", ") {
        let (n, kind) = part.split_once(' ')?;
        let n: usize = n.parse().ok()?;
        if kind.starts_with("passed") {
            s.passed = n;
        } else if kind.starts_with("skipped") || kind.starts_with("ignored") {
            s.skipped = n;
        } else if [
            "failed", "leaked", "timed", "cancel", "abort", "flaky", "sigsegv",
        ]
        .iter()
        .any(|w| kind.starts_with(w))
        {
            s.failed += n;
        }
    }
    Some(s)
}

/// Parses `3 tests, with 1 failure (0 unexpected) in …` into passed and failed.
fn xctest_counts(rest: &str) -> Option<Summary> {
    let total = leading_usize(rest)?;
    let failed = leading_usize(rest.split_once("with ")?.1)?;
    Some(Summary {
        passed: total.saturating_sub(failed),
        failed,
        skipped: 0,
    })
}

/// Reads the count before `kind` in `3 passed; 1 failed; 0 ignored`.
fn count_in(counts: &str, kind: &str) -> Option<usize> {
    counts.split([';', ',']).find_map(|part| {
        let mut it = part.trim().split(' ');
        let n = it.next()?;
        let k = it.next()?;
        (k == kind).then(|| n.parse().ok()).flatten()
    })
}

/// The number at the start of `s`, if any.
fn leading_usize(s: &str) -> Option<usize> {
    let n: String = s.chars().take_while(char::is_ascii_digit).collect();
    n.parse().ok()
}

/// The number after `key` in `Failed: 0, Passed: 2`, with its padding.
fn field(s: &str, key: &str) -> Option<usize> {
    leading_usize(s.split_once(key)?.1.trim_start())
}
