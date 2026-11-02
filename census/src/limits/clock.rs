// SPDX-License-Identifier: MPL-2.0
//! Owns the calendar of the limits: weekdays, hours and day starts of unix seconds in the local
//! zone.

/// Seconds in a day.
pub const DAY_S: u64 = 86_400;
/// Seconds in an hour.
pub const HOUR_S: u64 = 3_600;
/// English weekday names, Monday first.
pub const WEEKDAYS: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
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

/// Returns the local weekday and clock time of `t`, such as `Sat 21:00`.
pub fn when(t: u64) -> String {
    let local = local_secs(t);
    let s = local.rem_euclid(DAY_I);
    format!(
        "{} {:02}:{:02}",
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

/// Returns the offset of the local zone from UTC at `t`, in seconds east, from the zone rules
/// Windows has for the year of `t`.
#[cfg(windows)]
fn offset_at(t: i64) -> i64 {
    use windows_sys::Win32::Foundation::{FILETIME, SYSTEMTIME};
    use windows_sys::Win32::System::Time::{
        FileTimeToSystemTime, GetTimeZoneInformationForYear, SystemTimeToFileTime,
        SystemTimeToTzSpecificLocalTime, TIME_ZONE_INFORMATION,
    };
    // Seconds from 1601, the FILETIME epoch, to 1970, the unix epoch.
    const UNIX_TO_FILETIME_S: i64 = 11_644_473_600;
    const TICKS_PER_S: i64 = 10_000_000;

    let Some(ticks) = t
        .checked_add(UNIX_TO_FILETIME_S)
        .and_then(|s| s.checked_mul(TICKS_PER_S))
        .and_then(|ticks| u64::try_from(ticks).ok())
    else {
        return 0;
    };
    let utc_file = FILETIME {
        dwLowDateTime: ticks as u32,
        dwHighDateTime: (ticks >> 32) as u32,
    };
    let mut utc_sys: SYSTEMTIME = zeroed();
    let mut zone: TIME_ZONE_INFORMATION = zeroed();
    let mut local_sys: SYSTEMTIME = zeroed();
    let mut local_file: FILETIME = zeroed();
    // SAFETY: `utc_file` is read and `utc_sys` written, both valid for the call.
    if unsafe { FileTimeToSystemTime(&utc_file, &mut utc_sys) } == 0 {
        return 0;
    }
    // SAFETY: a null pointer selects the current zone, which is not read; `zone` is written.
    if unsafe { GetTimeZoneInformationForYear(utc_sys.wYear, std::ptr::null(), &mut zone) } == 0 {
        return 0;
    }
    // SAFETY: `zone` and `utc_sys` are read and `local_sys` written, all valid for the call.
    if unsafe { SystemTimeToTzSpecificLocalTime(&zone, &utc_sys, &mut local_sys) } == 0 {
        return 0;
    }
    // SAFETY: `local_sys` is read and `local_file` written, both valid for the call.
    if unsafe { SystemTimeToFileTime(&local_sys, &mut local_file) } == 0 {
        return 0;
    }
    let local = (u64::from(local_file.dwHighDateTime) << 32) | u64::from(local_file.dwLowDateTime);
    (local as i64 - ticks as i64) / TICKS_PER_S
}

/// Returns an all-zero Windows struct, which the calls that fill it overwrite before any read.
#[cfg(windows)]
fn zeroed<T>() -> T {
    // SAFETY: the Windows structs used here are plain C data, for which all zeros is valid.
    unsafe { std::mem::zeroed() }
}

/// Returns zero where the platform has no zone rules to ask, so the times are UTC.
#[cfg(not(any(unix, windows)))]
fn offset_at(_t: i64) -> i64 {
    0
}

#[cfg(all(test, windows))]
mod windows_tests {
    use super::{offset_at, zeroed};
    use std::time::{SystemTime, UNIX_EPOCH};
    use windows_sys::Win32::System::Time::{GetTimeZoneInformation, TIME_ZONE_INFORMATION};

    /// The offset now must be the bias the system reports for the current zone state.
    #[test]
    fn offset_now_matches_the_system_bias() {
        const TIME_ZONE_ID_DAYLIGHT: u32 = 2;
        let mut zone: TIME_ZONE_INFORMATION = zeroed();
        // SAFETY: `zone` is valid for the call to write.
        let id = unsafe { GetTimeZoneInformation(&mut zone) };
        let bias_min = if id == TIME_ZONE_ID_DAYLIGHT {
            zone.Bias + zone.DaylightBias
        } else {
            zone.Bias + zone.StandardBias
        };
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(0));
        assert_eq!(offset_at(now), -i64::from(bias_min) * 60);
    }
}

use crate::usage::time::{civil_from_days, days_from_civil};

/// Returns the first second of the local month of `t`, its local midnight on the 1st.
pub fn month_start(t: u64) -> u64 {
    month_boundary(t, 0)
}

/// Returns the first second of the local month after the one of `t`.
pub(super) fn next_month_start(t: u64) -> u64 {
    month_boundary(t, 1)
}

/// Returns the local midnight on the 1st of the month `months_ahead` after the month of `t`.
fn month_boundary(t: u64, months_ahead: i64) -> u64 {
    let (year, month, _) = civil_from_days(local_secs(t).div_euclid(DAY_I));
    let index = year * 12 + month - 1 + months_ahead;
    let local = days_from_civil(index.div_euclid(12), index.rem_euclid(12) + 1, 1) * DAY_I;
    // The offset on the 1st can differ from the one at `t`, so it is read at the first guess.
    let guess = local - offset_at(signed(t));
    u64::try_from(local - offset_at(guess)).unwrap_or(0)
}

#[cfg(test)]
mod month_tests {
    use super::*;

    fn utc(y: i64, m: i64, d: i64, secs: i64) -> u64 {
        u64::try_from(days_from_civil(y, m, d) * DAY_I + secs).unwrap_or(0)
    }

    fn local_first(y: i64, m: i64) -> i64 {
        days_from_civil(y, m, 1) * DAY_I
    }

    #[test]
    fn month_start_is_local_midnight_of_the_first() {
        let t = utc(2026, 11, 17, 12 * HOUR_I);
        assert_eq!(local_secs(month_start(t)), local_first(2026, 11));
    }

    #[test]
    fn month_start_changes_exactly_at_the_local_month_boundary() {
        let first = month_start(utc(2026, 11, 17, 0));
        assert_eq!(month_start(first), first);
        assert_eq!(local_secs(month_start(first - 1)), local_first(2026, 10));
        assert_eq!(next_month_start(first - 1), first);
    }

    #[test]
    fn next_month_start_rolls_over_the_year() {
        let t = utc(2026, 12, 31, 12 * HOUR_I);
        assert_eq!(local_secs(next_month_start(t)), local_first(2027, 1));
    }
}
