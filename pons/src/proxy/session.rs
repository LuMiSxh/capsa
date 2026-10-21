// SPDX-License-Identifier: MPL-2.0

//! Runs one proxied session: pumps stdio through the engine and finishes it.

use super::*;

pub(super) enum Event {
    Client(Vec<u8>),
    ClientClosed,
    Agent(Vec<u8>),
    AgentClosed,
    ClientWriteFailed,
}

/// Why the main loop ended.
pub(super) enum Exit {
    /// The client went away (closed stdin, or its pipe broke).
    Client(&'static str),
    /// The agent exited on its own.
    Agent,
}

/// What the proxy keeps while the agent runs.
pub(super) struct Session {
    pub(super) child: Child,
    pub(super) pgid: Option<u32>,
    pub(super) status: Option<ExitStatus>,
    pub(super) agent_closed: bool,
    pub(super) drain: Pin<Box<Sleep>>,
    pub(super) kill_at: Pin<Box<Sleep>>,
    pub(super) kill_armed: bool,
    pub(super) engine: Engine,
    pub(super) signals: Signals,
    pub(super) ticker: Interval,
    pub(super) logger: Logger,
    pub(super) io: Wiring,
}

/// Runs the main loop until the client or the agent ends the session.
pub(super) async fn pump(s: &mut Session) -> Result<Exit> {
    loop {
        let actions = tokio::select! {
            event = s.io.rx.recv() => match event {
                Some(Event::Client(line)) => {
                    s.io.c2a.sub(line.len() + 1);
                    s.logger.log(Dir::ClientToAgent, &line);
                    s.engine.on_client(&line, Instant::now().into_std())
                }
                Some(Event::Agent(line)) => {
                    s.io.a2c.sub(line.len() + 1);
                    s.logger.log(Dir::AgentToClient, &line);
                    s.engine.on_agent(&line, Instant::now().into_std())
                }
                Some(Event::ClientClosed) => return Ok(Exit::Client("client closed stdin")),
                Some(Event::ClientWriteFailed) => {
                    return Ok(Exit::Client("writing to the client failed"));
                }
                Some(Event::AgentClosed) | None => {
                    s.agent_closed = true;
                    if s.status.is_some() {
                        return Ok(Exit::Agent);
                    }
                    continue;
                }
            },
            _ = s.ticker.tick() => s.engine.tick(Instant::now().into_std()),
            exited = s.child.wait(), if s.status.is_none() => {
                s.status = Some(exited.context("waiting for agent")?);
                if s.agent_closed {
                    return Ok(Exit::Agent);
                }
                s.drain.as_mut().reset(Instant::now() + DRAIN_GRACE);
                continue;
            }
            () = &mut s.drain, if s.status.is_some() => return Ok(Exit::Agent),
            () = &mut s.kill_at, if s.kill_armed && s.status.is_none() => {
                s.kill_armed = false;
                s.logger.log(
                    Dir::Pons,
                    b"agent did not exit after the forwarded signal: SIGKILL to its process group",
                );
                if let Some(pgid) = s.pgid {
                    signal_kill(pgid);
                }
                continue;
            }
            sig = s.signals.recv() => {
                // Only while the child is unreaped, so its pid is still ours.
                if s.status.is_none()
                    && let Some(pgid) = s.child.id().or(s.pgid)
                {
                    s.logger.log(
                        Dir::Pons,
                        format!("forwarding {} to the agent's process group", sig.name()).as_bytes(),
                    );
                    signal_forward(pgid, sig);
                    s.kill_at.as_mut().reset(Instant::now() + SIGNAL_GRACE);
                    s.kill_armed = true;
                }
                continue;
            }
        };
        s.io.out.send(actions);
    }
}

/// Ends the session: tears the agent down when the client went away, or
/// flushes the output when the agent exited.
pub(super) async fn finish(exit: Exit, s: Session, grace: Duration) -> Result<i32> {
    let Session {
        mut child,
        pgid,
        status,
        mut engine,
        logger,
        io:
            Wiring {
                mut out,
                client_writer,
                agent_writer,
                ..
            },
        ..
    } = s;
    match exit {
        Exit::Client(why) => {
            out.agent = None; // the writer flushes what is queued, then closes stdin
            drop(out);
            teardown(&mut child, pgid, why, grace, &logger).await;
            Ok(0)
        }
        Exit::Agent => {
            let status = status.context("agent exit status")?;
            logger.log(
                Dir::Pons,
                format!("exiting: agent exited ({})", describe(status)).as_bytes(),
            );
            // Prompts the agent never answered would show "working" forever.
            out.send(engine.on_agent_exit(&describe(status)));
            if let Some(pgid) = pgid {
                sweep_group(pgid, &logger).await;
            }
            out.agent = None;
            out.client = None;
            drop(out);
            let flush = async {
                let _ = client_writer.await;
                let _ = agent_writer.await;
            };
            if timeout(FLUSH_GRACE, flush).await.is_err() {
                logger.log(
                    Dir::Pons,
                    b"exiting: output was still queued after 2s, dropped",
                );
            }
            Ok(exit_code(status))
        }
    }
}
