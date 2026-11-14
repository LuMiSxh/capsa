// SPDX-License-Identifier: MPL-2.0
//! Owns the hint contract: a library error names its fix, and the binary shows it.
//! It keeps that hint in the `anyhow` chain for [`crate::exit`] to render.

mod feature;
#[cfg(test)]
mod tests;

pub use feature::{uninstall_unavailable, update_unavailable};

use std::borrow::Cow;
use std::error::Error;
use std::fmt;

/// A fix the user can apply, when a failure has one.
pub trait Hint {
    /// The hint text, or `None` when no fix is known.
    fn hint(&self) -> Option<Cow<'_, str>>;
}

/// An error that keeps its message, its causes and a hint.
///
/// It stands in for the original error type in an `anyhow` chain: the message
/// appears once, the causes are kept as messages, and [`find_hint`] reads the
/// hint back.
#[derive(Debug)]
pub struct Hinted {
    message: String,
    hint: String,
    cause: Option<Box<Cause>>,
}

impl Hinted {
    /// Creates an error with a message and a hint, and no underlying cause.
    pub fn new(message: impl Into<String>, hint: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            hint: hint.into(),
            cause: None,
        }
    }

    /// Captures the message and source chain of `err`, with `hint` attached.
    fn from_error(err: &(dyn Error + 'static), hint: String) -> Self {
        Self {
            message: err.to_string(),
            hint,
            cause: Cause::chain_of(err),
        }
    }
}

impl fmt::Display for Hinted {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl Error for Hinted {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        self.cause
            .as_deref()
            .map(|cause| cause as &(dyn Error + 'static))
    }
}

/// Holds one link of a captured source chain. The original error types are gone;
/// their messages stay.
#[derive(Debug)]
struct Cause {
    message: String,
    next: Option<Box<Cause>>,
}

impl Cause {
    fn chain_of(err: &(dyn Error + 'static)) -> Option<Box<Self>> {
        err.source().map(|source| {
            Box::new(Self {
                message: source.to_string(),
                next: Self::chain_of(source),
            })
        })
    }
}

impl fmt::Display for Cause {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl Error for Cause {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        self.next
            .as_deref()
            .map(|next| next as &(dyn Error + 'static))
    }
}

/// Converts a library result into `anyhow`, keeping its hint.
pub trait ResultExt<T> {
    /// Converts like `?` into [`anyhow::Result`]. A hinted error carries a [`Hinted`]
    /// in its chain; any other error is wrapped unchanged.
    fn hinted(self) -> anyhow::Result<T>;
}

impl<T, E> ResultExt<T> for Result<T, E>
where
    E: Error + Hint + Send + Sync + 'static,
{
    fn hinted(self) -> anyhow::Result<T> {
        self.map_err(|err| {
            let Some(hint) = err.hint().map(Cow::into_owned) else {
                return anyhow::Error::new(err);
            };
            anyhow::Error::new(Hinted::from_error(&err, hint))
        })
    }
}

/// Returns the hint of the first [`Hinted`] in the chain of `err`, if any.
pub fn find_hint(err: &anyhow::Error) -> Option<&str> {
    err.chain()
        .find_map(|link| link.downcast_ref::<Hinted>())
        .map(|hinted| hinted.hint.as_str())
}

/// Builds an [`anyhow::Error`] for a failure the user can fix: `message` says what failed and
/// `hint` the fix. [`find_hint`] reads the hint back.
pub fn hinted(message: impl Into<String>, hint: impl Into<String>) -> anyhow::Error {
    anyhow::Error::new(Hinted::new(message, hint))
}
