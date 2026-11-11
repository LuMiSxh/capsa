// SPDX-License-Identifier: MPL-2.0
//! Forwards termination signals to the agent's process group and escalates to SIGKILL.

use capsa_core::process::Signal;

use super::*;

/// Forwards `sig` to the agent's process group and arms the SIGKILL timer. Does nothing once
/// the child is reaped, because its pid is no longer ours then.
pub(super) fn forward(s: &mut Session, sig: Signal) {
    if s.status.is_none()
        && let Some(pgid) = s.child.id().or(s.pgid)
    {
        s.logger.log(
            Dir::Pons,
            format!("forwarding {} to the agent's process group", sig.name()).as_bytes(),
        );
        let _ = signal_group(pgid, sig);
        s.kill_at.as_mut().reset(Instant::now() + SIGNAL_GRACE);
        s.kill_armed = true;
    }
}

/// Sends SIGKILL to the agent's group after a forwarded signal went unanswered.
pub(super) fn escalate(s: &mut Session) {
    s.kill_armed = false;
    s.logger.log(
        Dir::Pons,
        b"agent did not exit after the forwarded signal: SIGKILL to its process group",
    );
    if let Some(pgid) = s.pgid {
        let _ = signal_group(pgid, Signal::Kill);
    }
}
