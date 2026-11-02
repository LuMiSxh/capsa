// SPDX-License-Identifier: MPL-2.0
//! End-to-end tests: `pons` runs in front of the mock agent and talks ACP over stdio.

#![expect(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "test code: a failed setup or assertion should panic"
)]

mod background;
mod bench;
mod cancel;
mod census;
mod cli;
mod failsafe;
mod folding;
mod forwarding;
mod harness;
mod initialize;
mod launcher;
mod notices;
mod permissions;
mod shutdown;
mod steering;
mod turns;
