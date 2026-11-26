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
fn a_sibling_reports_its_install_bulletin_and_feature_state() {
    type Case = (
        Option<&'static str>,
        &'static [&'static str],
        &'static [(&'static str, u64)],
        String,
    );
    let cases: &[Case] = &[
        (
            Some("census"),
            &["census"],
            &[("census", NOW - 300)],
            format!("installed, bulletin 5m ago, active: {FEATURE}"),
        ),
        (
            Some("census"),
            &[],
            &[],
            format!("not installed, off: {FEATURE}"),
        ),
        (
            Some("census"),
            &["census"],
            &[],
            format!("installed, no bulletin, active: {FEATURE}"),
        ),
        (
            None,
            &["census"],
            &[],
            format!("installed, active: {FEATURE}"),
        ),
    ];
    for (bulletin, installed, written, expected) in cases {
        let group = build(&[census(*bulletin)], installed, written);
        assert_eq!(group.name, "interop");
        let line = &group.checks[0];
        assert_eq!(line.name, "census");
        assert_eq!(line.status, Status::Ok, "{expected}");
        assert_eq!(line.detail, *expected);
        assert_eq!(line.hint, None, "{expected}");
    }
}
