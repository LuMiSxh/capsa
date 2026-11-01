// SPDX-License-Identifier: MPL-2.0
//! Process and I/O glue around [`Engine`]: a reader and a writer task per stdio side.
//! The agent runs in its own process group (Unix) so that signals and teardown reach it all.

use std::pin::Pin;
use std::process::{ExitStatus, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use anyhow::{Context, Result};
use capsa_core::census_snapshot::append_to;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::{Notify, mpsc};
use tokio::task::JoinHandle;
use tokio::time::{Instant, Interval, MissedTickBehavior, Sleep, interval, sleep, timeout};

use crate::engine::{Action, Config, Engine};
use crate::launcher::Spawn;
use crate::log::{Dir, Logger};

mod io;
mod session;
mod teardown;

use session::{Event, Session, finish, pump};

use io::*;
use teardown::*;

const TICK: Duration = Duration::from_millis(100);
/// How long to wait for buffered agent output after the child exited.
const DRAIN_GRACE: Duration = Duration::from_secs(1);
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

/// Counts the bytes queued in one direction.
#[derive(Default)]
struct Gate {
    queued: AtomicUsize,
    changed: Notify,
}

impl Gate {
    fn add(&self, n: usize) {
        self.queued.fetch_add(n, Ordering::SeqCst);
    }

    fn sub(&self, n: usize) {
        self.queued.fetch_sub(n, Ordering::SeqCst);
        self.changed.notify_waiters();
    }

    /// Returns once fewer than [`BACKLOG_CAP`] bytes are queued.
    async fn wait_room(&self) {
        loop {
            let notified = self.changed.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.queued.load(Ordering::SeqCst) < BACKLOG_CAP {
                return;
            }
            notified.await;
        }
    }
}

/// Runs the proxy and returns the process exit code.
pub async fn run(cfg: Config, spawn: Spawn, logger: Logger) -> Result<i32> {
    let grace = cfg.shutdown_grace;
    let (child, pgid, agent_in, agent_out) = spawn_agent(spawn)?;
    let io = wire(agent_in, agent_out, &logger);
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
        engine: Engine::new(cfg),
        signals: Signals::new()?,
        ticker,
        logger,
        io,
    };
    let exit = pump(&mut session).await?;
    finish(exit, session, grace).await
}

/// Starts the agent in its own process group and takes its stdio.
fn spawn_agent(spawn: Spawn) -> Result<(Child, Option<u32>, ChildStdin, ChildStdout)> {
    let program = spawn.program.to_string_lossy().into_owned();
    let mut command = Command::new(&spawn.program);
    command.args(&spawn.args);
    if let Some(path) = spawn.path_dirs.and_then(|d| std::env::join_paths(d).ok()) {
        command.env("PATH", path);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.as_std_mut().process_group(0);
    }
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .kill_on_drop(true)
        .spawn()
        .with_context(|| format!("failed to spawn agent `{program}`"))?;
    let pgid = child.id();
    let agent_in = child.stdin.take().context("child stdin")?;
    let agent_out = child.stdout.take().context("child stdout")?;
    Ok((child, pgid, agent_in, agent_out))
}

/// The channels and I/O tasks between the two sides and the engine loop.
struct Wiring {
    rx: mpsc::UnboundedReceiver<Event>,
    out: Out,
    c2a: Arc<Gate>,
    a2c: Arc<Gate>,
    client_writer: JoinHandle<()>,
    agent_writer: JoinHandle<()>,
}

/// Starts the reader and writer tasks for both directions.
fn wire(agent_in: ChildStdin, agent_out: ChildStdout, logger: &Logger) -> Wiring {
    let (tx, rx) = mpsc::unbounded_channel();
    let a2c = Arc::new(Gate::default());
    let c2a = Arc::new(Gate::default());
    let (agent_tx, agent_rx) = mpsc::unbounded_channel::<Vec<u8>>();
    let (client_tx, client_rx) = mpsc::unbounded_channel::<Vec<u8>>();
    let agent_writer = tokio::spawn(write_lines(agent_in, agent_rx, c2a.clone(), None));
    let client_writer = tokio::spawn(write_lines(
        tokio::io::stdout(),
        client_rx,
        a2c.clone(),
        Some(tx.clone()),
    ));
    tokio::spawn(read_lines(
        tokio::io::stdin(),
        tx.clone(),
        c2a.clone(),
        Event::Client,
        || Event::ClientClosed,
    ));
    tokio::spawn(read_lines(agent_out, tx, a2c.clone(), Event::Agent, || {
        Event::AgentClosed
    }));
    let out = Out {
        agent: Some(agent_tx),
        client: Some(client_tx),
        c2a: c2a.clone(),
        a2c: a2c.clone(),
        logger: logger.clone(),
    };
    Wiring {
        rx,
        out,
        c2a,
        a2c,
        client_writer,
        agent_writer,
    }
}

/// The two output queues; every line sent is counted in its gate.
struct Out {
    agent: Option<mpsc::UnboundedSender<Vec<u8>>>,
    client: Option<mpsc::UnboundedSender<Vec<u8>>>,
    c2a: Arc<Gate>,
    a2c: Arc<Gate>,
    logger: Logger,
}

impl Out {
    fn send(&self, actions: Vec<Action>) {
        for action in actions {
            match action {
                Action::ToAgent(mut line) => {
                    line.push(b'\n');
                    Self::queue(&self.agent, &self.c2a, line);
                }
                Action::ToClient {
                    mut line,
                    synthesized,
                } => {
                    if synthesized {
                        self.logger.log(Dir::PonsToClient, &line);
                    }
                    line.push(b'\n');
                    Self::queue(&self.client, &self.a2c, line);
                }
                Action::Census { file, snapshot } => {
                    if let Err(err) = append_to(&file, &snapshot) {
                        self.logger
                            .log(Dir::Pons, format!("census: {err}").as_bytes());
                    }
                }
                Action::Note(note) => self.logger.log(Dir::Pons, note.as_bytes()),
            }
        }
    }

    fn queue(tx: &Option<mpsc::UnboundedSender<Vec<u8>>>, gate: &Gate, line: Vec<u8>) {
        let len = line.len();
        gate.add(len);
        let sent = tx.as_ref().is_some_and(|tx| tx.send(line).is_ok());
        if !sent {
            gate.sub(len);
        }
    }
}

/// Signals pons forwards to the agent.
#[derive(Clone, Copy)]
enum Sig {
    #[cfg(unix)]
    Term,
    #[cfg(unix)]
    Hup,
    Int,
}

impl Sig {
    fn name(self) -> &'static str {
        match self {
            #[cfg(unix)]
            Self::Term => "SIGTERM",
            #[cfg(unix)]
            Self::Hup => "SIGHUP",
            Self::Int => "SIGINT",
        }
    }
}

#[cfg(unix)]
struct Signals {
    term: tokio::signal::unix::Signal,
    hup: tokio::signal::unix::Signal,
    int: tokio::signal::unix::Signal,
}

#[cfg(unix)]
impl Signals {
    fn new() -> std::io::Result<Self> {
        use tokio::signal::unix::{SignalKind, signal};
        Ok(Self {
            term: signal(SignalKind::terminate())?,
            hup: signal(SignalKind::hangup())?,
            int: signal(SignalKind::interrupt())?,
        })
    }

    async fn recv(&mut self) -> Sig {
        tokio::select! {
            _ = self.term.recv() => Sig::Term,
            _ = self.hup.recv() => Sig::Hup,
            _ = self.int.recv() => Sig::Int,
        }
    }
}

/// Windows has no SIGTERM; Ctrl+C and Ctrl+Break are the only requests.
#[cfg(windows)]
struct Signals;

#[cfg(windows)]
impl Signals {
    fn new() -> std::io::Result<Self> {
        Ok(Self)
    }

    async fn recv(&mut self) -> Sig {
        let _ = tokio::signal::ctrl_c().await;
        Sig::Int
    }
}

/// Sends `sig` to the process group `pgid`, which is the agent's pid: the
/// agent was started with `process_group(0)`. Callers only use it while the
/// agent is unreaped, or right after it was reaped to sweep what it left in
/// its group (a group id stays taken while any member lives).
#[cfg(unix)]
fn kill_group(pgid: u32, sig: i32) -> bool {
    let Ok(pgid) = i32::try_from(pgid) else {
        return false;
    };
    if pgid <= 1 {
        return false;
    }
    // SAFETY: kill(2) takes plain integers and touches no memory of ours;
    // SAFETY: `-pgid` addresses only the process group of our own child.
    unsafe { libc::kill(-pgid, sig) == 0 }
}

#[cfg(unix)]
fn group_alive(pgid: u32) -> bool {
    let Ok(id) = i32::try_from(pgid) else {
        return false;
    };
    if id <= 1 {
        return false;
    }
    // SAFETY: signal 0 only checks that the group exists.
    let rc = unsafe { libc::kill(-id, 0) };
    rc == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

#[cfg(unix)]
fn signal_term(pgid: u32) {
    kill_group(pgid, libc::SIGTERM);
}

#[cfg(unix)]
fn signal_kill(pgid: u32) {
    kill_group(pgid, libc::SIGKILL);
}

#[cfg(unix)]
fn signal_forward(pgid: u32, sig: Sig) {
    kill_group(
        pgid,
        match sig {
            Sig::Term => libc::SIGTERM,
            Sig::Hup => libc::SIGHUP,
            Sig::Int => libc::SIGINT,
        },
    );
}

/// Ends the child and its descendants on Windows, which has no process groups to
/// signal and no graceful request for an arbitrary child. Without a Job object (it
/// needs more than the standard library), `taskkill /T` ends them on a best-effort basis.
#[cfg(windows)]
fn taskkill_tree(pid: u32) {
    let _ = std::process::Command::new("taskkill")
        .args(["/PID", &pid.to_string(), "/T", "/F"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

#[cfg(windows)]
fn group_alive(_pgid: u32) -> bool {
    false
}

#[cfg(windows)]
fn signal_term(pid: u32) {
    taskkill_tree(pid);
}

#[cfg(windows)]
fn signal_kill(pid: u32) {
    taskkill_tree(pid);
}

#[cfg(windows)]
fn signal_forward(pid: u32, _sig: Sig) {
    taskkill_tree(pid);
}

#[cfg(unix)]
fn exit_code_of(status: ExitStatus) -> Option<i32> {
    use std::os::unix::process::ExitStatusExt;
    status.code().or_else(|| status.signal().map(|s| 128 + s))
}

#[cfg(not(unix))]
fn exit_code_of(status: ExitStatus) -> Option<i32> {
    status.code()
}

fn exit_code(status: ExitStatus) -> i32 {
    exit_code_of(status).unwrap_or(1)
}
