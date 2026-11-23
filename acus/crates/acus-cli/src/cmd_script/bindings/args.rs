// SPDX-License-Identifier: MPL-2.0
//! Reads a host call's arguments into the command line of the acus command it stands for, and
//! reads its single string and whole-number options.

use super::super::argv::Call;
use super::host_error;
use crate::{Cli, Cmd};
use acus_script::HostError;
use clap::Parser;
use serde_json::Value;

/// Parses the call as the command line `acus NAME FLAGS -- POSITIONALS`, with the command's own
/// clap definition.
pub(super) fn parse(name: &str, call: &Call, skip: &[&str]) -> Result<Cmd, HostError> {
    let mut words = vec!["acus".to_owned(), name.to_owned()];
    words.extend(call.argv(name, skip)?);
    Cli::try_parse_from(&words)
        .map(|cli| cli.cmd)
        .map_err(|e| host_error(&crate::parse_error(&e, &words)))
}

/// Returns the single string argument of the function `name`, such as a path or a name.
pub(super) fn one_string<'a>(call: &'a Call, name: &str) -> Result<&'a str, HostError> {
    match call.positionals.as_slice() {
        [text] => Ok(text.as_str()),
        _ => Err(HostError::new(format!("{name}() takes one string"))),
    }
}

/// Returns the whole-number option `key`, or `None` when it is absent.
pub(super) fn count_option(call: &Call, key: &str) -> Result<Option<usize>, HostError> {
    match call.options.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_u64()
            .and_then(|n| usize::try_from(n).ok())
            .map(Some)
            .ok_or_else(|| HostError::new(format!("option {key} must be a whole number"))),
    }
}

pub(super) fn internal(name: &str) -> HostError {
    HostError::new(format!("{name}() is not available in scripts"))
}
