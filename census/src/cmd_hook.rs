// SPDX-License-Identifier: MPL-2.0
//! `census hook`: the SessionStart, UserPromptSubmit and PostToolUse hook. Exits 0 and prints
//! nothing unless the main agent has a new context or limit level to hear about, or a SessionStart
//! can point it at the report commands; any error also prints nothing.

use crate::now_secs;
use capsa_core::exit::Outcome;
use census::config::{Config, Mode};
use census::hook;
use census::limits::{self, LimitStatus, MonthStatus};
use std::io::{IsTerminal, Read, Write};
use std::path::Path;
use std::sync::{OnceLock, mpsc};
use std::time::{Duration, Instant};

/// The most hook input read; a larger one is not a hook input.
const MAX_INPUT: u64 = 1 << 20;
/// Default time after which the limits are skipped for this call, in milliseconds.
const LIMITS_DEADLINE_MS: u64 = 150;
/// Time before the limits deadline at which the ledger update stops reading and saves its progress,
/// early enough for the helper to finish. A cold ledger so warms over a few calls.
const LEDGER_MARGIN: Duration = Duration::from_millis(50);

/// The limits deadline. `CENSUS_HOOK_DEADLINE_MS` replaces the default, read once, so that the
/// tests can allow the hook time under a loaded machine.
fn limits_deadline() -> Duration {
    static DEADLINE: OnceLock<Duration> = OnceLock::new();
    *DEADLINE.get_or_init(|| {
        let ms = std::env::var("CENSUS_HOOK_DEADLINE_MS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(LIMITS_DEADLINE_MS);
        Duration::from_millis(ms)
    })
}

/// The ledger deadline, `LEDGER_MARGIN` before `limits_deadline`.
fn ledger_deadline() -> Duration {
    limits_deadline().saturating_sub(LEDGER_MARGIN)
}

pub fn run() -> Outcome {
    let start = Instant::now();
    let now = now_secs();
    if let Some(input) = read_input() {
        let config = Config::load();
        let budget = config.mode == Mode::Budget;
        let result = hook::run(
            &input,
            now,
            &config,
            |current| {
                (!budget)
                    .then(|| limits_within(start, now, current))
                    .flatten()
            },
            |current| {
                budget
                    .then(|| month_within(start, now, current, config.monthly_budget_usd))
                    .flatten()
            },
        );
        match result {
            Ok(Some(out)) => {
                let _ = writeln!(std::io::stdout(), "{out}");
            }
            Ok(None) => {}
            Err(e) => hook::log_error(&e.to_string()),
        }
    }
    Outcome::Found
}

/// Reads stdin up to the cap; `None` for a terminal, a larger input or text that is not UTF-8.
fn read_input() -> Option<String> {
    let stdin = std::io::stdin();
    if stdin.is_terminal() {
        return None;
    }
    let mut buf = Vec::new();
    stdin
        .lock()
        .take(MAX_INPUT + 1)
        .read_to_end(&mut buf)
        .ok()?;
    if buf.len() as u64 > MAX_INPUT {
        return None;
    }
    String::from_utf8(buf).ok()
}

/// Computes the limits on a helper thread and gives up when `limits_deadline` has passed since
/// `start`. A busy ledger lock already makes `limits::status` skip its update.
fn limits_within(start: Instant, now: u64, current: Option<&Path>) -> Option<LimitStatus> {
    // The helper thread outlives this call when it misses the deadline, so it owns the path.
    let current = current.map(Path::to_path_buf);
    within(start, move || {
        limits::status_by(now, Some(start + ledger_deadline()), current.as_deref()).ok()
    })
}

/// Like `limits_within`, for the spend of the month in budget mode.
fn month_within(
    start: Instant,
    now: u64,
    current: Option<&Path>,
    budget: Option<f64>,
) -> Option<MonthStatus> {
    let current = current.map(Path::to_path_buf);
    within(start, move || {
        limits::month(
            now,
            budget,
            Some(start + ledger_deadline()),
            current.as_deref(),
        )
        .ok()
    })
}

fn within<T: Send + 'static>(
    start: Instant,
    work: impl FnOnce() -> Option<T> + Send + 'static,
) -> Option<T> {
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new()
        .spawn(move || {
            let _ = tx.send(work());
        })
        .ok()?;
    rx.recv_timeout(limits_deadline().saturating_sub(start.elapsed()))
        .ok()
        .flatten()
}
