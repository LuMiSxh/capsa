// SPDX-License-Identifier: MPL-2.0
//! Maps the follow rate of a guard rule, which census publishes in its bulletin, to the number of
//! calls between two shown hints.

use capsa_core::bulletin::{self, Stamped};
use serde::Deserialize;
use std::collections::BTreeMap;

/// The part of the census bulletin that guard reads. Other fields are ignored, so census can add
/// some.
#[derive(Deserialize)]
struct Census {
    guard_follow: Stamped<BTreeMap<String, f32>>,
}

/// Returns the current follow rate of `rule` in 0..=1, or `None` when the bulletin is missing,
/// stale or unreadable, or has no rate for the rule.
pub fn rate(rule: &str) -> Option<f32> {
    let census: Census = bulletin::read("census")?;
    let r = *census.guard_follow.current(bulletin::now())?.get(rule)?;
    r.is_finite().then(|| r.clamp(0.0, 1.0))
}

/// Returns the calls between two shown hints: `base` without a rate, `lo` for a rate of 0, `hi` for
/// a rate of 1, and a straight line between those points through `base` at a rate of 0.5.
///
/// `lo` is held to `1..=base` and `hi` to at least `base`.
pub fn interval(base: usize, lo: usize, hi: usize, rate: Option<f32>) -> usize {
    let Some(rate) = rate else {
        return base;
    };
    let lo = lo.clamp(1, base);
    let hi = hi.max(base);
    let (from, to, t) = if rate < 0.5 {
        (lo, base, rate * 2.0)
    } else {
        (base, hi, (rate - 0.5) * 2.0)
    };
    #[expect(
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "the result lies between `from` and `to`, which are small counts"
    )]
    let n = (from as f64 + (to as f64 - from as f64) * f64::from(t)).round() as usize;
    n.clamp(lo, hi)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_rate_keeps_the_base_interval() {
        assert_eq!(interval(10, 5, 40, None), 10);
    }

    #[test]
    fn rate_one_gives_the_maximum_and_rate_zero_the_minimum() {
        assert_eq!(interval(10, 5, 40, Some(1.0)), 40);
        assert_eq!(interval(10, 5, 40, Some(0.0)), 5);
    }

    #[test]
    fn rate_between_scales_through_the_base() {
        assert_eq!(interval(10, 5, 40, Some(0.5)), 10);
        assert_eq!(interval(10, 5, 40, Some(0.75)), 25);
        assert_eq!(interval(10, 5, 40, Some(0.25)), 8);
    }

    #[test]
    fn bounds_around_the_base_are_enforced() {
        assert_eq!(interval(10, 0, 40, Some(0.0)), 1);
        assert_eq!(interval(10, 20, 5, Some(0.0)), 10);
        assert_eq!(interval(10, 20, 5, Some(1.0)), 10);
    }
}
