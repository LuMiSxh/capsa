// SPDX-License-Identifier: MPL-2.0

use crate::render::Status;
use crate::update::{Tool, UpdateCheck};

use super::update_check;

const RUNNING: Tool = Tool {
    name: "acus",
    version: "0.2.0",
};

#[test]
fn the_newest_release_is_an_ok_line_that_says_so() {
    let line = update_check(&RUNNING, &UpdateCheck::Current);
    assert_eq!(line.name, "update");
    assert_eq!(line.status, Status::Ok);
    assert_eq!(line.detail, "0.2.0 is the latest");
    assert_eq!(line.hint, None);
}

#[test]
fn a_newer_release_is_a_warning_with_the_self_update_fix() {
    let line = update_check(&RUNNING, &UpdateCheck::Available("0.3.0".to_owned()));
    assert_eq!(line.status, Status::Warn);
    assert_eq!(line.detail, "0.3.0 available (running 0.2.0)");
    assert_eq!(line.hint.as_deref(), Some("run acus self update"));
}

#[test]
fn an_unanswered_check_is_an_ok_line_that_says_so() {
    let line = update_check(&RUNNING, &UpdateCheck::Unknown);
    assert_eq!(line.status, Status::Ok);
    assert_eq!(line.detail, "could not check for updates");
    assert_eq!(line.hint, None);
}
