// SPDX-License-Identifier: MPL-2.0
//! Owns the UTC calendar of the limits: weekdays, hours and day starts of unix seconds.

/// Seconds in a day.
pub const DAY_S: u64 = 86_400;
/// Seconds in an hour.
pub const HOUR_S: u64 = 3_600;
/// English weekday names, Monday first.
pub const WEEKDAYS: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];

/// Returns the weekday of `t` in UTC, Monday as 0. The epoch day, 1970-01-01, was a Thursday.
pub(super) fn weekday(t: u64) -> usize {
    ((t / DAY_S + 3) % 7) as usize
}

/// Returns the first second of the hour of `t`.
pub(super) fn hour_start(t: u64) -> u64 {
    t / HOUR_S * HOUR_S
}

/// Returns the first second of the UTC day of `t`.
pub(super) fn day_start(t: u64) -> u64 {
    t / DAY_S * DAY_S
}

/// Returns the weekday and UTC clock time of `t`, such as `Sat 21:00 UTC`.
pub fn when(t: u64) -> String {
    let s = t % DAY_S;
    format!(
        "{} {:02}:{:02} UTC",
        WEEKDAYS[weekday(t)],
        s / HOUR_S,
        s % HOUR_S / 60
    )
}
