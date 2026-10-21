// SPDX-License-Identifier: MPL-2.0
use std::borrow::Cow;
use std::io;

use anyhow::Context;

use super::{Hint, Hinted, ResultExt, find_hint};

#[derive(Debug, thiserror::Error)]
enum LibError {
    #[error("config file missing: {0}")]
    Missing(String),
    #[error("cannot read config")]
    Read(#[source] io::Error),
    #[error("unexpected state")]
    Other,
}

impl Hint for LibError {
    fn hint(&self) -> Option<Cow<'_, str>> {
        match self {
            Self::Missing(path) => Some(Cow::Owned(format!("create {path}"))),
            Self::Read(_) => Some(Cow::Borrowed("check the file permissions")),
            Self::Other => None,
        }
    }
}

fn missing() -> anyhow::Result<()> {
    Err(LibError::Missing("a.toml".into()))
        .hinted()
        .context("loading settings")
}

#[test]
fn hint_is_found_through_context_layers() {
    let err = missing().unwrap_err();
    assert_eq!(find_hint(&err), Some("create a.toml"));
}

#[test]
fn message_appears_once_in_chain() {
    let err = missing().unwrap_err();
    let messages: Vec<String> = err.chain().map(ToString::to_string).collect();
    assert_eq!(
        messages,
        ["loading settings", "config file missing: a.toml"]
    );
}

#[test]
fn causes_survive_the_hint_wrapper() {
    let source = io::Error::new(io::ErrorKind::PermissionDenied, "denied");
    let err = Err::<(), _>(LibError::Read(source)).hinted().unwrap_err();
    let messages: Vec<String> = err.chain().map(ToString::to_string).collect();
    assert_eq!(messages, ["cannot read config", "denied"]);
    assert_eq!(find_hint(&err), Some("check the file permissions"));
}

#[test]
fn error_without_hint_yields_none() {
    let err = Err::<(), _>(LibError::Other).hinted().unwrap_err();
    assert_eq!(find_hint(&err), None);
    assert_eq!(err.to_string(), "unexpected state");
}

#[test]
fn hinted_new_is_found_directly() {
    let err = anyhow::Error::new(Hinted::new("bad", "try again"));
    assert_eq!(find_hint(&err), Some("try again"));
}
