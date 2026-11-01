// SPDX-License-Identifier: MPL-2.0

use super::calibrate::calibrate;
use super::clock::{DAY_S, HOUR_S};
#[cfg(unix)]
use super::clock::{day_start, next_day_start};
use super::cost::Costed;
use super::forecast::week;
use super::pattern::{Expect, cycle_index};
use super::window::{Span, current};
#[cfg(unix)]
use super::when;
use super::{Forecast, Full, History, Kind, LimitStatus, Reading, compute};
use crate::ledger::{Hourly, Row, Tokens as LedgerTokens};
use capsa_core::census_snapshot::{Snapshot, Source, Window};

const BASE: u64 = 1_800_000_000;

/// A request of `claude-opus-5-5` that costs `usd` at list price ($4 per million input tokens).
fn request(t: u64, usd: f64) -> Row {
    Row {
        id: format!("m{t}"),
        t,
        session: "s".to_owned(),
        agent: None,
        role: None,
        model: "claude-opus-5-5".to_owned(),
        input: (usd * 250_000.0).round() as u64,
        cache_read: 0,
        cache_write_5m: 0,
        cache_write_1h: 0,
        output: 0,
        gap_s: None,
    }
}

/// An hourly aggregate of `claude-haiku` that costs `usd` at list price ($0.10 per million input).
fn aggregate(hour: u64, usd: f64) -> Hourly {
    Hourly {
        hour,
        family: "haiku".to_owned(),
        sub: false,
        tokens: LedgerTokens {
            input: (usd * 1e7).round() as u64,
            cache_read: 0,
            cache_write_5m: 0,
            cache_write_1h: 0,
            output: 0,
            requests: 1,
        },
    }
}

fn snap(window: Window, t: u64, used: f64, resets_at: u64) -> Snapshot {
    Snapshot {
        t,
        source: Source::Statusline,
        window,
        used_percentage: used,
        resets_at,
    }
}

/// Five five-hour snapshots, ten minutes apart from `BASE`, all in the window that ends five hours
/// after `BASE`.
fn five_snapshots(used: [f64; 5]) -> Vec<Snapshot> {
    used.iter()
        .enumerate()
        .map(|(i, u)| {
            snap(
                Window::FiveHour,
                BASE + (i as u64 + 1) * 600,
                *u,
                BASE + 5 * HOUR_S,
            )
        })
        .collect()
}

fn status_at(rows: &[Row], snapshots: &[Snapshot], now: u64) -> LimitStatus {
    compute(
        &History {
            rows,
            hourly: &[],
            snapshots,
        },
        now,
    )
}

#[test]
fn k_is_the_median_of_the_points_and_spread_their_interquartile_range() {
    let costs = Costed::new(&[request(BASE + 1, 1.0)], &[]);
    let snaps = five_snapshots([10.0, 20.0, 30.0, 40.0, 50.0]);
    let cal = calibrate(Window::FiveHour, &snaps, &costs, BASE + 3600).expect("five points");
    assert_eq!(cal.points, 5);
    assert!((cal.k - 30.0).abs() < 1e-9);
    let spread = cal.spread.expect("four points give a spread");
    assert!((spread - 2.0 / 3.0).abs() < 1e-9);
}

#[test]
fn a_window_without_cost_or_share_has_no_calibration() {
    let priced = Costed::new(&[request(BASE + 1, 1.0)], &[]);
    let unpriced = Costed::new(&[], &[]);
    let unused = five_snapshots([0.0; 5]);
    assert!(calibrate(Window::FiveHour, &unused, &priced, BASE + 3600).is_none());
    let used = five_snapshots([10.0; 5]);
    assert!(calibrate(Window::FiveHour, &used, &unpriced, BASE + 3600).is_none());
}

#[test]
fn a_fresh_snapshot_is_the_reading_and_an_older_one_is_estimated_from_k() {
    let rows = [request(BASE + 1, 1.0)];
    let snaps = five_snapshots([10.0, 20.0, 30.0, 40.0, 50.0]);
    let older = status_at(&rows, &snaps, BASE + 3000 + 1800).windows[0].clone();
    assert_eq!(
        older.reading,
        Some(Reading::Estimated {
            snapshot_age_s: Some(1800)
        })
    );
    assert!((older.used.expect("estimated share") - 30.0).abs() < 1e-9);
    let fresh = status_at(&rows, &snaps, BASE + 3000 + 60).windows[0].clone();
    assert_eq!(fresh.reading, Some(Reading::Snapshot { age_s: 60 }));
    assert_eq!(fresh.used, Some(50.0));
}

#[test]
fn five_hour_window_without_a_future_snapshot_starts_at_the_first_request_after_the_reset() {
    let reset = BASE + 5 * HOUR_S;
    let rows = [
        request(reset - 50, 1.0),
        request(reset + 600, 0.1),
        request(reset + 600 + 2 * HOUR_S, 0.1),
        request(reset + 600 + 5 * HOUR_S + 30, 0.1),
    ];
    let costs = Costed::new(&rows, &[]);
    let latest = snap(Window::FiveHour, reset - 100, 80.0, reset);
    let start = reset + 600;
    assert_eq!(
        current(Window::FiveHour, &latest, &costs, start + 3 * HOUR_S),
        Some(Span {
            start,
            end: start + 5 * HOUR_S
        })
    );
    let next = start + 5 * HOUR_S + 30;
    assert_eq!(
        current(Window::FiveHour, &latest, &costs, start + 5 * HOUR_S + 100),
        Some(Span {
            start: next,
            end: next + 5 * HOUR_S
        })
    );
}

#[test]
fn full_around_when_the_rate_fills_the_window_before_its_reset_else_lasts_until_reset() {
    let now = BASE + 2 * HOUR_S;
    let snaps = five_snapshots([30.0; 5]);
    let full_for = |last_hour_usd: f64| {
        let rows = [request(BASE + 1, 1.0), request(now - 60, last_hour_usd)];
        status_at(&rows, &snaps, now).windows[0].full
    };
    match full_for(1.0) {
        Some(Full::At { t }) => assert!(t > now && t < BASE + 5 * HOUR_S, "full at {t}"),
        other => panic!("expected full around a time, got {other:?}"),
    }
    assert_eq!(full_for(0.1), Some(Full::AfterReset));
}

#[test]
fn without_a_snapshot_there_is_no_calibration_only_the_burn_rate() {
    let now = BASE + 2 * HOUR_S;
    let rows = [request(now - 60, 0.5)];
    let status = status_at(&rows, &[], now);
    assert!(status.data);
    assert!(
        status
            .windows
            .iter()
            .all(|w| w.used.is_none() && w.calibration.is_none() && w.reset_at.is_none())
    );
    assert!((status.burn.usd_per_hour_60 - 0.5).abs() < 1e-9);
    assert!(status.forecast.is_none());
}

#[test]
fn an_empty_history_has_no_data() {
    let status = status_at(&[], &[], BASE);
    assert!(!status.data);
    assert_eq!(status.basis.hours, 0);
}

#[test]
fn recent_weeks_weigh_more_than_old_ones() {
    let now = BASE + 30 * DAY_S;
    let expected = |old: f64, recent: f64| {
        let hourly = [
            aggregate(now - 28 * DAY_S, old),
            aggregate(now - 21 * DAY_S, 0.0),
            aggregate(now - 14 * DAY_S, 0.0),
            aggregate(now - 7 * DAY_S, recent),
        ];
        Expect::build(&Costed::new(&[], &hourly), vec![], now).at(now, None)
    };
    // Weights 0.5, 0.59, 0.71 and 0.84 for the four weeks, oldest first.
    let recent_high = expected(0.0, 1.0);
    let old_high = expected(1.0, 0.0);
    assert!((0.31..0.33).contains(&recent_high), "{recent_high}");
    assert!((0.18..0.20).contains(&old_high), "{old_high}");
}

#[test]
fn the_cycle_key_follows_each_weeks_own_reset() {
    let first = Span {
        start: BASE,
        end: BASE + 7 * DAY_S,
    };
    let early = BASE + 7 * DAY_S - 5 * HOUR_S;
    let second = Span {
        start: early,
        end: early + 7 * DAY_S,
    };
    let windows = [first, second];
    assert_eq!(cycle_index(&windows, BASE + 2 * HOUR_S), Some(2));
    assert_eq!(cycle_index(&windows, BASE + 7 * DAY_S - HOUR_S), Some(4));
    assert_eq!(cycle_index(&windows, BASE + 7 * DAY_S + HOUR_S), Some(6));
    assert_eq!(cycle_index(&windows, BASE - 1), None);
}

#[test]
fn weekly_forecast_runs_out_or_leaves_a_share_unused() {
    let now = BASE + 30 * DAY_S;
    let span = Span {
        start: now,
        end: now + 3 * DAY_S,
    };
    let expect_for = |usd_per_hour: f64| {
        let hourly: Vec<Hourly> = (1..=28 * 24)
            .map(|h: u64| aggregate(now - h * HOUR_S, usd_per_hour))
            .collect();
        Expect::build(&Costed::new(&[], &hourly), vec![], now)
    };
    // 40 % used, 30 % per USD, $0.10 an hour: 3 points an hour reach 100 % after 20 hours.
    match week(40.0, 30.0, 0.1, now, span, &expect_for(0.1)) {
        Forecast::RunsOut { t } => {
            assert!(t.abs_diff(now + 20 * HOUR_S) < 60, "runs out at {t}");
        }
        other => panic!("expected a run out, got {other:?}"),
    }
    // $0.001 an hour for 72 hours adds 2.16 points: 57.84 % unused.
    match week(40.0, 30.0, 0.001, now, span, &expect_for(0.001)) {
        Forecast::Unused { pct } => assert!((pct - 57.84).abs() < 0.01, "{pct}"),
        other => panic!("expected a share left, got {other:?}"),
    }
}

#[test]
fn tables_have_one_row_per_weekday_hour_and_cycle_bucket() {
    let costs = Costed::new(&[request(BASE + 1, 1.0)], &[]);
    let expect = Expect::build(&costs, vec![], BASE + 2 * HOUR_S);
    let weekday = expect.table(Kind::Weekday);
    assert_eq!(weekday.len(), 7);
    assert_eq!(expect.table(Kind::Hour).len(), 24);
    let cycle = expect.table(Kind::Cycle);
    assert_eq!(cycle.len(), 168);
    assert_eq!(cycle.iter().map(|b| b.hours).sum::<usize>(), 0);
    let share: f64 = weekday.iter().map(|b| b.share).sum();
    assert!((share - 1.0).abs() < 1e-9);
    assert_eq!(weekday.iter().map(|b| b.hours).sum::<usize>(), 2);
}

#[cfg(unix)]
#[test]
fn berlin_clock_follows_the_dst_changes_of_2026() {
    // The child runs with TZ set, so the process zone is not changed for the other tests.
    if std::env::var_os("CENSUS_TZ_CHILD").is_none() {
        let out = std::process::Command::new(std::env::current_exe().expect("the test binary"))
            .args([
                "--exact",
                "limits::tests::berlin_clock_follows_the_dst_changes_of_2026",
            ])
            .env("TZ", "Europe/Berlin")
            .env("CENSUS_TZ_CHILD", "1")
            .output()
            .expect("runs the test binary");
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stdout)
        );
        return;
    }
    // 2026-10-25 01:00 UTC: CEST (+2) turns back to CET (+1), so 02:00 happens twice.
    assert_eq!(when(1_792_886_400), "Sun 02:00");
    assert_eq!(when(1_792_889_999), "Sun 02:59");
    assert_eq!(when(1_792_890_000), "Sun 02:00");
    assert_eq!(when(1_792_893_600), "Sun 03:00");
    // The local day of 2026-10-25 is 25 hours long and starts at 2026-10-24 22:00 UTC.
    let day = day_start(1_792_890_000);
    assert_eq!(day, 1_792_879_200);
    assert_eq!(next_day_start(day) - day, 25 * HOUR_S);
    // The local day of 2026-03-29 is 23 hours long and starts at 2026-03-28 23:00 UTC.
    let spring = day_start(1_774_738_800 + 12 * HOUR_S);
    assert_eq!(spring, 1_774_738_800);
    assert_eq!(next_day_start(spring) - spring, 23 * HOUR_S);
    // Both 02:00 hours of 2026-10-25 land in the Sunday 02:00 bucket, so it counts two hours.
    let hourly: Vec<Hourly> = (0..4)
        .map(|h| aggregate(1_792_886_400 + h * HOUR_S, 1.0))
        .collect();
    let expect = Expect::build(
        &Costed::new(&[], &hourly),
        vec![],
        1_792_886_400 + 4 * HOUR_S,
    );
    let hours = |label: &str| {
        expect
            .table(Kind::Hour)
            .iter()
            .find(|b| b.label == label)
            .map(|b| b.hours)
    };
    assert_eq!(hours("02:00"), Some(2));
    assert_eq!(hours("03:00"), Some(1));
}
