// SPDX-License-Identifier: MPL-2.0

use super::Sibling;
use super::interop::interop_group_with;
use crate::render::{Group, Status};

const NOW: u64 = 1_000_000;
const FEATURE: &str = "guard blocks in the census status line";

fn census(bulletin: Option<&'static str>) -> Sibling<'static> {
    Sibling {
        tool: "census",
        feature: FEATURE,
        bulletin,
    }
}

fn build(siblings: &[Sibling<'_>], installed: &[&str], written: &[(&str, u64)]) -> Group {
    interop_group_with(
        siblings,
        |tool| installed.contains(&tool),
        |name| {
            written
                .iter()
                .find(|(bulletin, _)| *bulletin == name)
                .map(|(_, at)| *at)
        },
        NOW,
    )
}

#[test]
fn an_installed_sibling_reports_its_bulletin_age_and_that_its_feature_is_active() {
    let group = build(
        &[census(Some("census"))],
        &["census"],
        &[("census", NOW - 300)],
    );
    assert_eq!(group.name, "interop");
    let line = &group.checks[0];
    assert_eq!(line.name, "census");
    assert_eq!(line.status, Status::Ok);
    assert_eq!(
        line.detail,
        format!("installed, bulletin 5m ago, active: {FEATURE}")
    );
    assert_eq!(line.hint, None);
}

#[test]
fn a_missing_sibling_is_off_and_its_line_is_still_ok() {
    let group = build(&[census(Some("census"))], &[], &[]);
    let line = &group.checks[0];
    assert_eq!(line.status, Status::Ok);
    assert_eq!(line.detail, format!("not installed, off: {FEATURE}"));
}

#[test]
fn an_installed_sibling_without_its_bulletin_says_so() {
    let group = build(&[census(Some("census"))], &["census"], &[]);
    assert_eq!(
        group.checks[0].detail,
        format!("installed, no bulletin, active: {FEATURE}")
    );
}

#[test]
fn a_sibling_without_a_bulletin_name_reports_no_age() {
    let group = build(&[census(None)], &["census"], &[]);
    assert_eq!(
        group.checks[0].detail,
        format!("installed, active: {FEATURE}")
    );
}
