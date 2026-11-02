// SPDX-License-Identifier: MPL-2.0
//! Owns the calendar arithmetic for transcript timestamps, `YYYY-MM-DD` dates and ISO weeks.
//! Everything is UTC, so no date crate is needed.

const DAY_MS: u64 = 86_400_000;

/// Milliseconds since the epoch of a transcript timestamp such as `2026-10-14T07:53:43.386Z`,
/// or 0 when it does not parse.
pub fn parse_ts(s: &str) -> u64 {
    let num = |r: std::ops::Range<usize>| s.get(r).and_then(|t| t.parse::<i64>().ok());
    let (Some(y), Some(mo), Some(d), Some(h), Some(mi), Some(se)) = (
        num(0..4),
        num(5..7),
        num(8..10),
        num(11..13),
        num(14..16),
        num(17..19),
    ) else {
        return 0;
    };
    let frac: String = s
        .get(19..)
        .and_then(|t| t.strip_prefix('.'))
        .unwrap_or("")
        .chars()
        .take_while(char::is_ascii_digit)
        .take(3)
        .collect();
    let ms = format!("{frac:0<3}").parse::<i64>().unwrap_or(0);
    let secs = days_from_civil(y, mo, d) * 86_400 + h * 3_600 + mi * 60 + se;
    (secs * 1000 + ms).max(0) as u64
}

/// Milliseconds since the epoch at 00:00 UTC of a `YYYY-MM-DD` date.
pub fn parse_date(s: &str) -> Option<u64> {
    let mut parts = s.split('-').map(|p| p.parse::<i64>().ok());
    let (Some(Some(y)), Some(Some(m)), Some(Some(d)), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return None;
    };
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    u64::try_from(days_from_civil(y, m, d))
        .ok()
        .map(|days| days * DAY_MS)
}

/// The ISO week of a timestamp as `YYYY-Www`; weeks run Monday to Sunday.
pub fn iso_week(ms: u64) -> String {
    let days = (ms / DAY_MS) as i64;
    // 1970-01-01 was a Thursday, so `(days + 3) % 7` counts from Monday; the Thursday of the
    // week decides its ISO year.
    let thursday = days - (days + 3).rem_euclid(7) + 3;
    let (year, _, _) = civil_from_days(thursday);
    let week = (thursday - days_from_civil(year, 1, 1)) / 7 + 1;
    format!("{year}-W{week:02}")
}

/// Days since 1970-01-01 of a proleptic Gregorian date.
pub(crate) fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The (year, month, day) of a day count since 1970-01-01.
pub(crate) fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_dates_round_trip() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(
            civil_from_days(days_from_civil(2026, 10, 14)),
            (2026, 10, 14)
        );
        assert_eq!(civil_from_days(days_from_civil(2028, 2, 29)), (2028, 2, 29));
    }

    #[test]
    fn weeks_follow_iso_years() {
        // 2026-10-14 is a Wednesday of week 42; 2027-01-01 is a Friday of week 53 of 2026.
        assert_eq!(iso_week(parse_ts("2026-10-14T00:00:00Z")), "2026-W42");
        assert_eq!(iso_week(parse_ts("2027-01-01T12:00:00Z")), "2026-W53");
    }
}
