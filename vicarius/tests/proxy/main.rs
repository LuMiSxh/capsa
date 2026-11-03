// SPDX-License-Identifier: MPL-2.0
//! Runs the proxy against mock upstreams on 127.0.0.1.

#![expect(
    clippy::unwrap_used,
    reason = "setup helpers outside a test fn must fail the test"
)]

mod mock;
mod origin;
mod routing;
mod security;
mod streaming;
mod support;
