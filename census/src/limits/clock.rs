// SPDX-License-Identifier: MPL-2.0
//! Owns the calendar of the limits: weekdays, hours and day starts of unix seconds in the local
//! zone, or in UTC on Windows.

/// Seconds in a day.
pub const DAY_S: u64 = 86_400;
/// Seconds in an hour.
pub const HOUR_S: u64 = 3_600;
/// English weekday names, Monday first.
pub const WEEKDAYS: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
/// Names the zone after a displayed time: empty where the times are local.
const ZONE_SUFFIX: &str = if cfg!(unix) { "" } else { " UTC" };
const DAY_I: i64 = DAY_S as i64;
const HOUR_I: i64 = HOUR_S as i64;

/// Returns the local weekday of `t`, Monday as 0.
pub(super) fn weekday(t: u64) -> usize {
    weekday_of(local_secs(t))
}

/// Returns the local hour of the day of `t`, from 0 to 23.
pub(super) fn hour_of_day(t: u64) -> usize {
    hour_of(local_secs(t))
}

/// Returns the first second of the UTC hour of `t`, the grid of the ledger's hourly aggregates.
pub(super) fn hour_start(t: u64) -> u64 {
    t / HOUR_S * HOUR_S
}

/// Returns the first second of the local day of `t`, its local midnight.
pub(super) fn day_start(t: u64) -> u64 {
    let t = signed(t);
    let midnight = (t + offset_at(t)).div_euclid(DAY_I) * DAY_I;
    // The offset at the midnight differs from the one at `t` when a change lies between them.
    let start = midnight - offset_at(midnight - offset_at(t));
    u64::try_from(start).unwrap_or(0)
}

/// Returns the first second of the local day after the local day that starts at `day`.
pub(super) fn next_day_start(day: u64) -> u64 {
    // A day is 23 to 25 hours long, so 25 hours after its midnight is in the next day.
    day_start(day + DAY_S + HOUR_S)
}

/// Returns the local weekday and clock time of `t`, such as `Sat 21:00`. Windows, where the times
/// are UTC, adds ` UTC`.
pub fn when(t: u64) -> String {
    let local = local_secs(t);
    let s = local.rem_euclid(DAY_I);
    format!(
        "{} {:02}:{:02}{ZONE_SUFFIX}",
        WEEKDAYS[weekday_of(local)],
        s / HOUR_I,
        s % HOUR_I / 60
    )
}

/// Returns `t` as the seconds of the local clock, which are counted as if they were UTC.
fn local_secs(t: u64) -> i64 {
    let t = signed(t);
    t + offset_at(t)
}

fn signed(t: u64) -> i64 {
    i64::try_from(t).unwrap_or(i64::MAX)
}

/// Returns the Monday-first weekday of a local clock in seconds.
fn weekday_of(local: i64) -> usize {
    (local.div_euclid(DAY_I) + 3).rem_euclid(7) as usize
}

/// Returns the hour of the day of a local clock in seconds.
fn hour_of(local: i64) -> usize {
    (local.rem_euclid(DAY_I) / HOUR_I) as usize
}

/// Returns the offset of the local zone from UTC at `t`, in seconds east, as the system reads TZ.
#[cfg(unix)]
fn offset_at(t: i64) -> i64 {
    let Some(secs) = fits::<i64, libc::time_t>(t) else {
        return 0;
    };
    let mut tm = std::mem::MaybeUninit::<libc::tm>::uninit();
    // SAFETY: `secs` is a `time_t` that outlives the call and `tm` is a writable `tm`.
    // `localtime_r` returns null without initializing `tm` on failure, so `assume_init` runs only
    // after a successful call.
    let tm = unsafe {
        if libc::localtime_r(&secs, tm.as_mut_ptr()).is_null() {
            return 0;
        }
        tm.assume_init()
    };
    fits::<libc::c_long, i64>(tm.tm_gmtoff).unwrap_or(0)
}

/// Converts `a` to `B`, or returns `None` when it does not fit, as `time_t` may be 32 bits.
#[cfg(unix)]
fn fits<A, B: TryFrom<A>>(a: A) -> Option<B> {
    B::try_from(a).ok()
}

/// Returns zero: Windows keeps the times in UTC.
#[cfg(not(unix))]
fn offset_at(_t: i64) -> i64 {
    0
}
