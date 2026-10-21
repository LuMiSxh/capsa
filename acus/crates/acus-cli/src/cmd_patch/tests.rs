// SPDX-License-Identifier: MPL-2.0

use similar::{Algorithm, capture_diff_slices};

#[test]
fn regions_follow_the_formatter() {
    let old = ["a", "b", "  c", "d"];
    let new = ["x", "a", "b", "c", "d"];
    let ops = capture_diff_slices(Algorithm::Myers, &old, &new);
    assert_eq!(
        (super::map(&ops, 1, false), super::map(&ops, 2, true)),
        (2, 4)
    );
}
