// SPDX-License-Identifier: MPL-2.0
//! Decides which context and limit levels the hook has newly crossed and words them.

use crate::context::ContextUse;
use crate::limits::{Forecast, Full, LimitStatus, Reading, WindowStatus, when};
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

/// Returns the messages for the levels crossed and not yet in `shown`, and records them there.
/// A context below the re-arm share clears the context levels; when one was shown, the message
/// says the context was compacted, with the tokens before and after when `compacted` gives them.
pub(super) fn decide(
    context: Option<&ContextUse>,
    limits: Option<&LimitStatus>,
    compacted: Option<(u64, u64)>,
    now: u64,
    shown: &mut Shown,
) -> Vec<String> {
    let mut out = Vec::new();
    out.extend(context.and_then(|c| context_message(c, compacted, shown)));
    for w in limits.iter().flat_map(|l| &l.windows) {
        let forecast = limits.and_then(|l| l.forecast.as_ref());
        out.extend(limit_message(w, forecast, shown));
        if w.window == Window::SevenDay {
            out.extend(weekly_left_message(w, now, shown));
        }
    }
    out
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
    shown: &mut Shown,
) -> Option<String> {
    let (reset, used) = (w.reset_at?, w.used?);
    let tag = window_tag(w.window);
    let key = |what: &str| format!("{tag}:{what}:{reset}");
    let mut hit = Vec::new();
    let mut projected = None;
    match (w.window, w.full, forecast) {
        (Window::FiveHour, Some(Full::At { t }), _) => {
            hit.push(key("full"));
            projected = Some(format!(", full around {}", when(t)));
        }
        (Window::SevenDay, _, Some(Forecast::RunsOut { t })) => {
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
