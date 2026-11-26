// SPDX-License-Identifier: MPL-2.0

use crate::render::Status;
use crate::update::{Tool, UpdateCheck};

use super::update_check;

const RUNNING: Tool = Tool {
    name: "acus",
    version: "0.2.0",
};

#[test]
fn update_check_lines_report_the_release_state() {
    let cases = [
        (
            UpdateCheck::Current,
            Status::Ok,
            "0.2.0 is the latest",
            None,
        ),
        (
            UpdateCheck::Available("0.3.0".to_owned()),
            Status::Warn,
            "0.3.0 available (running 0.2.0)",
            Some("run acus self update"),
        ),
        (
            UpdateCheck::Unknown,
            Status::Ok,
            "could not check for updates",
            None,
        ),
    ];
    for (check, status, detail, hint) in cases {
        let line = update_check(&RUNNING, &check);
        assert_eq!(line.name, "update");
        assert_eq!(line.status, status, "{detail}");
        assert_eq!(line.detail, detail);
        assert_eq!(line.hint.as_deref(), hint, "{detail}");
    }
}
