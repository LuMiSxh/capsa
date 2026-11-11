// SPDX-License-Identifier: MPL-2.0
//! Process and I/O glue around [`Engine`]: a reader and writer task per stdio side.
//! The agent runs in its own process group so that signals and teardown reach it all.

use std::pin::Pin;
use std::process::{ExitStatus, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use anyhow::{Context, Result};
use capsa_core::census_snapshot::append_to;
use capsa_core::hint::Hinted;
use capsa_core::process::{Signals, group_alive, own_group, signal_group, wait_group_gone};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::{Notify, mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio::time::{Instant, Interval, MissedTickBehavior, Sleep, interval, sleep, timeout};

use crate::engine::{Action, Config, Engine};
use crate::launcher::Spawn;
use crate::log::{Dir, Logger};

mod io;
mod session;
mod signals;
mod teardown;
#[cfg(test)]
mod tests;
mod wiring;

use session::{Event, Session, finish, pump};

use io::*;
use teardown::*;
use wiring::{Gate, Wiring, wire};

const TICK: Duration = Duration::from_millis(100);
/// How long to wait for the agent's stdout to reach its end after the child exited. The
/// end comes at once unless a descendant of the agent keeps the pipe open.
const DRAIN_GRACE: Duration = Duration::from_secs(5);
/// How long a child gets to honour a forwarded signal before SIGKILL.
const SIGNAL_GRACE: Duration = Duration::from_secs(5);
/// How long the agent gets after SIGTERM at teardown before SIGKILL.
const TERM_GRACE: Duration = Duration::from_secs(2);
/// How long leftover processes of the agent's group get after SIGTERM.
const SWEEP_GRACE: Duration = Duration::from_secs(1);
/// How long the writers get to flush when the agent has exited.
const FLUSH_GRACE: Duration = Duration::from_secs(2);
/// Bytes queued in one direction before its reader pauses.
const BACKLOG_CAP: usize = 32 * 1024 * 1024;

/// Runs the proxy and returns the process exit code.
pub async fn run(cfg: Config, spawn: Spawn, logger: Logger) -> Result<i32> {
    let grace = cfg.shutdown_grace;
    let (child, pgid, agent_in, agent_out) = spawn_agent(spawn)?;
    let io = wire(agent_in, agent_out, &logger);
    let mut engine = Engine::new(cfg);
    engine.want_summaries(logger.wants_summaries());
    let far = Duration::from_secs(3600 * 24 * 365);
    let mut ticker = interval(TICK);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let mut session = Session {
        child,
        pgid,
        status: None,
        agent_closed: false,
        drain: Box::pin(sleep(far)),
        kill_at: Box::pin(sleep(far)),
        kill_armed: false,
        engine,
        signals: Signals::new()?,
        ticker,
        logger,
        io,
    };
    let exit = pump(&mut session).await?;
    let logger = session.logger.clone();
    let code = finish(exit, session, grace).await;
    // Tasks still holding a clone would keep the buffer from being written by Drop.
    logger.flush();
    code
}

/// Starts the agent in its own process group and takes its stdio.
fn spawn_agent(spawn: Spawn) -> Result<(Child, Option<u32>, ChildStdin, ChildStdout)> {
    let program = spawn.program.to_string_lossy().into_owned();
    let mut command = Command::new(&spawn.program);
    command.args(&spawn.args);
    if let Some(path) = spawn.path_dirs.and_then(|d| std::env::join_paths(d).ok()) {
        command.env("PATH", path);
    }
    own_group(command.as_std_mut());
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .kill_on_drop(true)
        .spawn()
        .map_err(|err| {
            anyhow::Error::new(Hinted::new(
                format!("failed to spawn agent `{program}`: {err}"),
                "run `pons self doctor` to check how the agent command resolves and whether it can start",
            ))
        })?;
    let pgid = child.id();
    let agent_in = child.stdin.take().context("child stdin")?;
    let agent_out = child.stdout.take().context("child stdout")?;
    Ok((child, pgid, agent_in, agent_out))
}
