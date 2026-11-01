// SPDX-License-Identifier: MPL-2.0
//! Finds the limit window that is running: from the reset of the newest snapshot, or from the
//! requests when that reset has passed.

use super::clock::{DAY_S, HOUR_S};
use super::cost::Costed;
use capsa_core::census_snapshot::{Snapshot, Window};

/// A limit window: its first second, and its reset, which is the first second after it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Span {
    pub start: u64,
    pub end: u64,
}

/// Returns the length of a window in seconds: five hours or seven days.
pub(super) fn length(window: Window) -> u64 {
    match window {
        Window::FiveHour => 5 * HOUR_S,
        Window::SevenDay => 7 * DAY_S,
    }
}

/// Returns the window that runs at `now`, given the newest snapshot of its type.
///
/// A reset after `now` is the snapshot's own window. A weekly window whose reset has passed repeats
/// every seven days. A five-hour window whose reset has passed starts at the first request after
/// that reset; `None` when no request has come since.
pub(super) fn current(window: Window, latest: &Snapshot, costs: &Costed, now: u64) -> Option<Span> {
    let len = length(window);
    if latest.resets_at > now {
        return Some(Span {
            start: latest.resets_at.saturating_sub(len),
            end: latest.resets_at,
        });
    }
    match window {
        Window::SevenDay => {
            let mut end = latest.resets_at;
            while end <= now {
                end += len;
            }
            Some(Span {
                start: end - len,
                end,
            })
        }
        Window::FiveHour => {
            let mut start = costs.first_request_from(latest.resets_at)?;
            loop {
                let end = start + len;
                if end > now {
                    return Some(Span { start, end });
                }
                start = costs.first_request_from(end)?;
            }
        }
    }
}

/// Returns the start of every five-hour window in the raw requests, oldest first. Each window
/// starts at the first request after the previous one ended.
pub(super) fn five_hour_starts(costs: &Costed) -> Vec<u64> {
    let len = length(Window::FiveHour);
    let mut starts = Vec::new();
    let mut next = costs.first_request_from(0);
    while let Some(start) = next {
        starts.push(start);
        next = costs.first_request_from(start + len);
    }
    starts
}
