// SPDX-License-Identifier: MPL-2.0
//! Owns the broker's wire format: one JSON request per line, one JSON response per line.

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// A tool call from the sandbox, as `{"tool": "...", "args": {...}}`.
#[derive(Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "tool", content = "args", rename_all = "snake_case")]
pub enum Request {
    GitPush(GitPushArgs),
    Gh(GhArgs),
    OpenUrl(OpenUrlArgs),
    RunOutside(RunOutsideArgs),
}

#[derive(Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct GitPushArgs {
    #[serde(default)]
    pub remote: Option<String>,
    #[serde(default)]
    pub force: bool,
}

#[derive(Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct GhArgs {
    pub args: Vec<String>,
}

#[derive(Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct OpenUrlArgs {
    pub url: String,
}

#[derive(Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RunOutsideArgs {
    pub command: String,
}

/// How a request ended.
#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    /// The command ran; `exit_code` and `output` say how it went.
    Done,
    /// The user (or the dialog's time limit) said no.
    Denied,
    /// The request was refused or could not run; `message` says why.
    Error,
}

/// The broker's answer to one request.
#[derive(Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct Response {
    pub status: Status,
    #[serde(default)]
    pub exit_code: Option<i32>,
    #[serde(default)]
    pub output: String,
    #[serde(default)]
    pub message: String,
}

impl Response {
    pub fn done(exit_code: i32, output: String) -> Self {
        Self {
            status: Status::Done,
            exit_code: Some(exit_code),
            output,
            message: String::new(),
        }
    }

    pub fn denied() -> Self {
        Self {
            status: Status::Denied,
            exit_code: None,
            output: String::new(),
            message: "the user denied the request".to_string(),
        }
    }

    pub fn error(message: impl Into<String>) -> Self {
        Self {
            status: Status::Error,
            exit_code: None,
            output: String::new(),
            message: message.into(),
        }
    }
}

/// Why a request is refused before anyone is asked.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum Refusal {
    #[error("{0} is longer than {1} characters, so it cannot be reviewed in full")]
    TooLong(&'static str, usize),
    #[error("{0} contains the character U+{1:04X}, which a request may not hold")]
    Forbidden(&'static str, u32),
    #[error("this repository cannot be used: {0}")]
    NoRepo(String),
    #[error("the git directory is no longer the one recorded when the run started")]
    RepoChanged,
    #[error("gh needs at least one argument")]
    NoGhArgs,
    #[error("{0} is empty")]
    Empty(&'static str),
    #[error("only http and https URLs can be opened, without spaces or control characters")]
    BadUrl,
    #[error("the working directory is on a detached HEAD, so there is no branch to push")]
    DetachedHead,
    #[error("{0:?} is not a remote of this repository")]
    UnknownRemote(String),
    #[error("{0} is not a git repository, or git failed: {1}")]
    Git(String, String),
    #[error("could not start {0}: {1}")]
    Spawn(String, String),
}
