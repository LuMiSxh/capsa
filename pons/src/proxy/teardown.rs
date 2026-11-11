// SPDX-License-Identifier: MPL-2.0
//! Tears the agent down when the session ends: stdin, then the process group signals.

use capsa_core::process::Signal;

use super::*;

/// Tears the agent down once the client is gone: it may end by itself (its stdin is
/// closed), then SIGTERM and SIGKILL its process group.
pub(super) async fn teardown(
    child: &mut Child,
    pgid: Option<u32>,
    why: &str,
    grace: Duration,
    logger: &Logger,
) {
    logger.log(
        Dir::Pons,
        format!(
            "exiting: {why}; closed the agent's stdin, waiting up to {}ms for it to exit",
            grace.as_millis()
        )
        .as_bytes(),
    );
    if let Ok(Ok(status)) = timeout(grace, child.wait()).await {
        logger.log(
            Dir::Pons,
            format!("exiting: agent exited by itself ({})", describe(status)).as_bytes(),
        );
        if let Some(pgid) = pgid {
            sweep_group(pgid, logger).await;
        }
        return;
    }
    if let Some(pgid) = pgid {
        logger.log(
            Dir::Pons,
            b"exiting: agent still running: SIGTERM to its process group",
        );
        let _ = signal_group(pgid, Signal::Term);
    }
    if let Ok(Ok(status)) = timeout(TERM_GRACE, child.wait()).await {
        logger.log(
            Dir::Pons,
            format!("exiting: agent exited after SIGTERM ({})", describe(status)).as_bytes(),
        );
        if let Some(pgid) = pgid {
            sweep_group(pgid, logger).await;
        }
        return;
    }
    logger.log(
        Dir::Pons,
        b"exiting: agent ignored SIGTERM: SIGKILL to its process group",
    );
    if let Some(pgid) = pgid {
        let _ = signal_group(pgid, Signal::Kill);
    }
    let _ = child.start_kill();
    let _ = timeout(Duration::from_secs(5), child.wait()).await;
}

/// Ends what the agent left behind in its process group.
pub(super) async fn sweep_group(pgid: u32, logger: &Logger) {
    if !group_alive(pgid) {
        return;
    }
    logger.log(
        Dir::Pons,
        b"exiting: processes of the agent's group are still running: SIGTERM",
    );
    let _ = signal_group(pgid, Signal::Term);
    if !wait_group_gone(pgid, SWEEP_GRACE).await {
        logger.log(Dir::Pons, b"exiting: SIGKILL to the leftover processes");
        let _ = signal_group(pgid, Signal::Kill);
        wait_group_gone(pgid, SWEEP_GRACE).await;
    }
}

pub(super) fn describe(status: ExitStatus) -> String {
    match capsa_core::process::exit_code(status) {
        Some(code) => format!("code {code}"),
        None => "no code".to_owned(),
    }
}
