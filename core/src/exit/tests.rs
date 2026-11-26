// SPDX-License-Identifier: MPL-2.0
use std::borrow::Cow;
use std::io;

use anyhow::Context;

use super::*;
use crate::hint::{Hint, ResultExt};

#[derive(Debug, thiserror::Error)]
enum Failure {
    #[error("cannot read config")]
    Read(#[source] io::Error),
}

impl Hint for Failure {
    fn hint(&self) -> Option<Cow<'_, str>> {
        Some(Cow::Borrowed("check the file permissions"))
    }
}

fn failing() -> anyhow::Result<()> {
    let source = io::Error::new(io::ErrorKind::PermissionDenied, "denied");
    Err(Failure::Read(source))
        .hinted()
        .context("loading settings")
}

fn render(err: &anyhow::Error, format: Format, color: bool) -> String {
    let mut out = Vec::new();
    write_error(&mut out, err, format, color).expect("write to Vec cannot fail");
    String::from_utf8(out).expect("output is UTF-8")
}

#[test]
fn non_json_reports_render_the_chain_and_hint() {
    let err = failing().unwrap_err();
    let expected = "error: loading settings\n  caused by: cannot read config\n  caused by: denied\nhint: check the file permissions\n";
    assert_eq!(render(&err, Format::Human, false), expected);

    let expected =
        "error: loading settings: cannot read config: denied\nhint: check the file permissions\n";
    assert_eq!(render(&err, Format::Agent, false), expected);

    let text = render(&err, Format::Human, true);
    assert!(text.starts_with("\x1b[31merror:\x1b[0m loading settings\n"));
    assert!(text.ends_with("\x1b[36mhint:\x1b[0m check the file permissions\n"));
}

#[test]
fn json_reports_include_causes_and_a_null_hint() {
    let err = failing().unwrap_err();
    assert_eq!(
        render(&err, Format::Json, false),
        "{\"error\":\"loading settings\",\"causes\":[\"cannot read config\",\"denied\"],\"hint\":\"check the file permissions\"}\n"
    );
    let err = anyhow::anyhow!("boom");
    assert_eq!(
        render(&err, Format::Json, false),
        "{\"error\":\"boom\",\"causes\":[],\"hint\":null}\n"
    );
}

#[test]
fn outcome_maps_to_exit_codes() {
    assert_eq!(Outcome::Found.code(), 0);
    assert_eq!(Outcome::Empty.code(), 1);
    assert_eq!(Outcome::Code(7).code(), 7);
}
