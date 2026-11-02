// SPDX-License-Identifier: MPL-2.0
//! Decides which context and limit levels the hook has newly crossed and words them.

use crate::context::ContextUse;
use crate::limits::{Forecast, Full, LimitStatus, MonthStatus, Reading, WindowStatus, when};
use capsa_core::census_snapshot::Window;
use std::collections::BTreeSet;

/// The levels already shown in one session.
pub(super) type Shown = BTreeSet<String>;

const CONTEXT_INFO_PCT: f64 = 60.0;
const CONTEXT_HANDOFF_PCT: f64 = 80.0;
/// Below this share after a compaction, the context levels show again.
const CONTEXT_REARM_PCT: f64 = 50.0;
const LIMIT_WARN_PCT: f64 = 75.0;
const LIMIT_HIGH_PCT: f64 = 90.0;
/// A weekly reset this close, with enough unused, is worth telling the user.
const WEEKLY_NEAR_S: u64 = 12 * 3600;
const WEEKLY_LEFT_PCT: f64 = 30.0;
const LIMIT_ADVICE: &str = "Avoid large parallel agent batches; tell Luca.";
/// A forecast is said only once its window is this full.
const FORECAST_MIN_PCT: f64 = 50.0;
/// The five-hour forecast waits until this much of its window has passed.
const FIVE_HOUR_MIN_ELAPSED_S: u64 = 30 * 60;
/// The weekly forecast needs this many weeks of history mapped to weekly windows.
const WEEKLY_BASIS_WEEKS: f64 = 2.0;
/// The budget warns at this share of the budget of the month, then again when it is used up.
const BUDGET_WARN_PCT: f64 = 80.0;
const BUDGET_FULL_PCT: f64 = 100.0;
/// A projection is said only after this much of the month, since the first days are noise.
const PROJECTION_MIN_ELAPSED_S: u64 = 3 * 86_400;
/// The status line comes back after this long without a prompt: the main agent's cache TTL.
const STATUS_GAP_S: u64 = 3600;
const PROMPT_KEY: &str = "prompt:";

/// Returns the messages for the levels crossed and not yet in `shown`, and records them there.
/// A context below the re-arm share clears the context levels; when one was shown, the message
/// says the context was compacted, with the tokens before and after when `compacted` gives them.
pub(super) fn decide(
    context: Option<&ContextUse>,
    limits: Option<&LimitStatus>,
    month: Option<&MonthStatus>,
    compacted: Option<(u64, u64)>,
    now: u64,
    shown: &mut Shown,
) -> Vec<String> {
    let mut out = Vec::new();
    out.extend(context.and_then(|c| context_message(c, compacted, shown)));
    let forecast = limits.and_then(|l| l.forecast.as_ref());
    let mapped_weeks = limits.map_or(0.0, |l| l.basis.mapped_weeks);
    for w in limits.iter().flat_map(|l| &l.windows) {
        out.extend(limit_message(w, forecast, now, mapped_weeks, shown));
        if w.window == Window::SevenDay {
            out.extend(weekly_left_message(w, now, shown));
        }
    }
    out.extend(month.and_then(|m| budget_message(m, shown)));
    out
}

/// Words the status line: the context and each limit window with a share, each with its reset.
/// `None` when neither is known.
pub(super) fn status_message(
    c: Option<&ContextUse>,
    limits: Option<&LimitStatus>,
    month: Option<&MonthStatus>,
) -> Option<String> {
    let mut parts = Vec::new();
    if let Some(c) = c {
        parts.push(format!(
            "context {} of {} (compaction at {})",
            tokens(c.tokens),
            tokens(c.threshold.tokens),
            tokens(c.threshold.tokens)
        ));
    }
    for w in limits.iter().flat_map(|l| &l.windows) {
        let Some(used) = w.used else {
            continue;
        };
        let label = if w.window == Window::FiveHour {
            "5h"
        } else {
            "week"
        };
        let reset = w
            .reset_at
            .map_or_else(String::new, |r| format!(" (reset {})", when(r)));
        parts.push(format!("{label} {used:.0}%{reset}"));
    }
    if let Some(m) = month {
        let budget = m
            .budget_usd
            .zip(m.percent)
            .map_or_else(String::new, |(b, p)| format!(" of {} ({p:.0}%)", usd(b)));
        parts.push(format!(
            "budget {}{budget}, month ends near {}, {} days left",
            usd(m.spent_usd),
            usd(m.projected_usd),
            m.days_left
        ));
    }
    (!parts.is_empty()).then(|| format!("census: status: {}.", parts.join("; ")))
}

fn usd(v: f64) -> String {
    format!("${v:.0}")
}

/// The budget rungs and the projection, once per month (its start time).
fn budget_message(m: &MonthStatus, shown: &mut Shown) -> Option<String> {
    let (budget, percent) = (m.budget_usd?, m.percent?);
    let key = |what: &str| format!("bud:{what}:{}", m.start);
    let mut hit = Vec::new();
    let projected =
        percent < BUDGET_FULL_PCT && m.projected_over && m.elapsed_s >= PROJECTION_MIN_ELAPSED_S;
    if projected {
        hit.push(key("proj"));
    }
    if percent >= BUDGET_WARN_PCT {
        hit.push(key("80"));
    }
    if percent >= BUDGET_FULL_PCT {
        hit.push(key("100"));
    }
    if hit.is_empty() || hit.iter().all(|k| shown.contains(k)) {
        return None;
    }
    shown.extend(hit);
    let state = if percent >= BUDGET_FULL_PCT {
        "used up".to_owned()
    } else {
        format!("{percent:.0}% used")
    };
    let projection = if projected {
        format!(", on course for {} by month end", usd(m.projected_usd))
    } else {
        String::new()
    };
    Some(format!(
        "census: monthly budget {state} ({} of {}, {} days left{projection}). {LIMIT_ADVICE}",
        usd(m.spent_usd),
        usd(budget),
        m.days_left
    ))
}

/// Records a UserPromptSubmit at `now` in `shown` and says whether the status line is due: the
/// first prompt of the session, or one after more than `STATUS_GAP_S` without a prompt.
pub(super) fn prompt_due(shown: &mut Shown, now: u64) -> bool {
    let last = shown
        .iter()
        .find_map(|k| k.strip_prefix(PROMPT_KEY)?.parse::<u64>().ok());
    shown.retain(|k| !k.starts_with(PROMPT_KEY));
    shown.insert(format!("{PROMPT_KEY}{now}"));
    last.is_none_or(|t| now.saturating_sub(t) > STATUS_GAP_S)
}

fn tokens(n: u64) -> String {
    format!("{}k", n.saturating_add(500) / 1000)
}

fn context_message(
    c: &ContextUse,
    compacted: Option<(u64, u64)>,
    shown: &mut Shown,
) -> Option<String> {
    if c.percent < CONTEXT_REARM_PCT {
        let info_shown = shown.remove("ctx60");
        let handoff_shown = shown.remove("ctx80");
        return (info_shown || handoff_shown).then(|| rearm_message(c, compacted));
    }
    let handoff = c.percent >= CONTEXT_HANDOFF_PCT;
    if c.percent < CONTEXT_INFO_PCT || shown.contains(if handoff { "ctx80" } else { "ctx60" }) {
        return None;
    }
    shown.insert("ctx60".to_owned());
    let mut text = format!(
        "census: context {} of {} (compaction at {}).",
        tokens(c.tokens),
        tokens(c.threshold.tokens),
        tokens(c.threshold.tokens)
    );
    if handoff {
        shown.insert("ctx80".to_owned());
        text.push_str(
            " At the next task boundary, suggest a handoff to Luca (session-handoff skill).",
        );
    }
    Some(text)
}

/// Words the re-arm: the context is small again, so earlier notes that it is full are outdated.
fn rearm_message(c: &ContextUse, compacted: Option<(u64, u64)>) -> String {
    let before_after = compacted.map_or_else(String::new, |(pre, post)| {
        format!(" ({} → {})", tokens(pre), tokens(post))
    });
    format!(
        "census: context was compacted{before_after}; now {} of {} (compaction at {}). \
         Earlier notes that the context is nearly full are outdated.",
        tokens(c.tokens),
        tokens(c.threshold.tokens),
        tokens(c.threshold.tokens)
    )
}

/// The share rungs and the forecast of one window, once per window instance (its reset time).
fn limit_message(
    w: &WindowStatus,
    forecast: Option<&Forecast>,
    now: u64,
    mapped_weeks: f64,
    shown: &mut Shown,
) -> Option<String> {
    let (reset, used) = (w.reset_at?, w.used?);
    let tag = window_tag(w.window);
    let key = |what: &str| format!("{tag}:{what}:{reset}");
    let armed = used >= FORECAST_MIN_PCT;
    let elapsed = w
        .start
        .is_some_and(|s| now.saturating_sub(s) >= FIVE_HOUR_MIN_ELAPSED_S);
    let mut hit = Vec::new();
    let mut projected = None;
    match (w.window, w.full, forecast) {
        (Window::FiveHour, Some(Full::At { t }), _) if armed && elapsed => {
            hit.push(key("full"));
            projected = Some(format!(", full around {}", when(t)));
        }
        (Window::SevenDay, _, Some(Forecast::RunsOut { t }))
            if armed && mapped_weeks >= WEEKLY_BASIS_WEEKS =>
        {
            hit.push(key("runs"));
            projected = Some(format!(", runs out around {}", when(*t)));
        }
        _ => {}
    }
    if used >= LIMIT_HIGH_PCT {
        hit.push(key("90"));
    }
    if used >= LIMIT_WARN_PCT {
        hit.push(key("75"));
    }
    if hit.iter().all(|k| shown.contains(k)) {
        return None;
    }
    shown.extend(hit);
    let source = match w.reading {
        Some(Reading::Estimated { .. }) => " (estimated)",
        _ => "",
    };
    Some(format!(
        "census: {tag} limit {used:.0}%{source}{}, reset {}. {LIMIT_ADVICE}",
        projected.unwrap_or_default(),
        when(reset)
    ))
}

fn weekly_left_message(w: &WindowStatus, now: u64, shown: &mut Shown) -> Option<String> {
    let (reset, used) = (w.reset_at?, w.used?);
    let left = 100.0 - used;
    let until = reset.checked_sub(now).filter(|s| *s > 0)?;
    if until > WEEKLY_NEAR_S || left < WEEKLY_LEFT_PCT || !shown.insert(format!("7d:left:{reset}"))
    {
        return None;
    }
    Some(format!(
        "census: weekly limit resets in {} h with {left:.0}% left.",
        (until + 1800) / 3600
    ))
}

fn window_tag(window: Window) -> &'static str {
    match window {
        Window::FiveHour => "5h",
        Window::SevenDay => "7d",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOUR: u64 = 3600;

    fn window(window: Window, used: f64, reset: u64, full: Option<Full>) -> WindowStatus {
        WindowStatus {
            window,
            start: Some(0),
            reset_at: Some(reset),
            used: Some(used),
            reading: None,
            calibration: None,
            full,
        }
    }

    fn month(spent: f64, elapsed_days: u64) -> MonthStatus {
        let start = 1_000_000;
        MonthStatus::new(
            spent,
            start,
            start + 30 * 86_400,
            start + elapsed_days * 86_400,
            Some(200.0),
            true,
        )
    }

    #[test]
    fn the_budget_warns_at_eighty_percent_once_and_again_when_used_up() {
        let mut shown = Shown::new();
        assert_eq!(budget_message(&month(100.0, 20), &mut shown), None);
        let warn = budget_message(&month(170.0, 20), &mut shown).unwrap();
        assert!(warn.starts_with("census: monthly budget 85% used ($170 of $200, 10 days left"));
        assert_eq!(budget_message(&month(175.0, 20), &mut shown), None);
        let full = budget_message(&month(205.0, 21), &mut shown).unwrap();
        assert!(full.contains("monthly budget used up ($205 of $200"));
    }

    #[test]
    fn a_projection_over_the_budget_is_said_once_after_three_days() {
        let mut shown = Shown::new();
        assert_eq!(budget_message(&month(60.0, 2), &mut shown), None);
        let said = budget_message(&month(60.0, 6), &mut shown).unwrap();
        assert!(said.contains("on course for $300 by month end"));
        assert_eq!(budget_message(&month(61.0, 6), &mut shown), None);
    }

    #[test]
    fn a_new_month_shows_the_budget_levels_again() {
        let mut shown = Shown::new();
        assert!(budget_message(&month(170.0, 20), &mut shown).is_some());
        let mut next = month(170.0, 20);
        next.start += 31 * 86_400;
        assert!(budget_message(&next, &mut shown).is_some());
    }

    fn five_hour(used: f64, elapsed: u64) -> Option<String> {
        let full = Some(Full::At { t: elapsed + HOUR });
        let w = window(Window::FiveHour, used, 5 * HOUR, full);
        limit_message(&w, None, elapsed, 0.0, &mut Shown::new())
    }

    #[test]
    fn a_five_hour_forecast_waits_for_half_full_and_thirty_minutes() {
        assert_eq!(five_hour(8.0, HOUR), None);
        assert_eq!(five_hour(60.0, 20 * 60), None);
        let said = five_hour(60.0, 40 * 60);
        assert!(said.is_some_and(|s| s.starts_with("census: 5h limit 60%, full around ")));
    }

    #[test]
    fn a_weekly_runs_out_forecast_needs_two_weeks_of_basis() {
        let w = window(Window::SevenDay, 60.0, 7 * 24 * HOUR, None);
        let runs = Forecast::RunsOut { t: 3 * 24 * HOUR };
        let said = |weeks| limit_message(&w, Some(&runs), HOUR, weeks, &mut Shown::new());
        assert_eq!(said(1.9), None);
        assert!(said(2.0).is_some_and(|s| s.contains("runs out around ")));
    }
}
