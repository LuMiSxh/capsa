// SPDX-License-Identifier: MPL-2.0
//! `census hook`: the UserPromptSubmit and PostToolUse hook. Exits 0 and prints nothing unless the
//! main agent has a new context or limit level to hear about; any error also prints nothing.

use crate::now_secs;
use capsa_core::exit::Outcome;
use census::hook;
use census::limits::{self, LimitStatus};
use std::io::{IsTerminal, Read, Write};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// The most hook input read; a larger one is not a hook input.
const MAX_INPUT: u64 = 1 << 20;
/// Time after which the limits are skipped for this call.
const LIMITS_DEADLINE: Duration = Duration::from_millis(150);
/// Time after which the ledger update stops reading and saves its progress, early enough for the
/// helper to finish before `LIMITS_DEADLINE`. A cold ledger so warms over a few calls.
const LEDGER_DEADLINE: Duration = Duration::from_millis(100);

pub fn run() -> Outcome {
    let start = Instant::now();
    let now = now_secs();
    if let Some(input) = read_input() {
        match hook::run(&input, now, || limits_within(start)) {
            Ok(Some(out)) => {
                let _ = writeln!(std::io::stdout(), "{out}");
            }
            Ok(None) => {}
            Err(e) => hook::log_line(now, &e.to_string()),
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

/// Computes the limits on a helper thread and gives up when `LIMITS_DEADLINE` has passed since
/// `start`. A busy ledger lock already makes `limits::status` skip its update.
fn limits_within(start: Instant) -> Option<LimitStatus> {
    let (tx, rx) = mpsc::channel();
    let now = now_secs();
    std::thread::Builder::new()
        .spawn(move || {
            let _ = tx.send(limits::status_by(now, Some(start + LEDGER_DEADLINE)).ok());
        })
        .ok()?;
    rx.recv_timeout(LIMITS_DEADLINE.saturating_sub(start.elapsed()))
        .ok()
        .flatten()
}
