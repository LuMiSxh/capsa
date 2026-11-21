// SPDX-License-Identifier: MPL-2.0
//! The interface through which a script reaches the program that embeds the engine.

use serde_json::Value;
use std::time::Instant;

/// A failed host call. It reaches the script as an `Error` with the hint in `.hint`.
#[derive(Debug, thiserror::Error)]
#[error("{message}")]
pub struct HostError {
    pub message: String,
    pub hint: Option<String>,
}

impl HostError {
    /// Returns an error without a hint.
    pub fn new(message: impl Into<String>) -> Self {
        HostError {
            message: message.into(),
            hint: None,
        }
    }

    /// Returns the error with the fix a reader should try.
    #[must_use]
    pub fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }
}

/// The functions a script may call. They are the script's only way out of the engine.
///
/// Calls run on worker threads, several at once, so a host is shared (`Sync`) and its calls take
/// `&self`. A script's calls start in the order the script makes them.
pub trait Host: Send + Sync + 'static {
    /// Returns the names installed as global functions.
    fn names(&self) -> &[&str];

    /// Runs the function `name` with the JSON form of the script's arguments.
    ///
    /// A returned object `{"kind": K, "items": [...]}` becomes an array carrying `K`, which the
    /// array methods keep.
    ///
    /// `deadline` is when the script's time runs out. A call that can run long should end by then
    /// (`run()` kills its command): the engine waits only a short grace for a running call before
    /// the process exits.
    ///
    /// # Errors
    /// The error is thrown into the script.
    fn call(&self, name: &str, args: Vec<Value>, deadline: Instant) -> Result<Value, HostError>;

    /// Returns whether the call `name` must run alone: it waits for every earlier call to end and
    /// holds back every later one until it ends, as a call that writes must.
    fn exclusive(&self, _name: &str) -> bool {
        false
    }

    /// Returns the source of the module a script imports as `specifier`, which always has a
    /// scheme (`repo:name@2`): the engine refuses relative and bare specifiers itself.
    ///
    /// # Errors
    /// The error is thrown into the importing script.
    fn module(&self, specifier: &str) -> Result<String, HostError> {
        Err(HostError::new(format!(
            "cannot import {specifier}: scripts cannot import modules here"
        )))
    }
}
