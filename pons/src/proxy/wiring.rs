// SPDX-License-Identifier: MPL-2.0
//! Wires the stdio of both sides to the engine loop: reader and writer tasks, the byte gates
//! and the output queues.

use super::*;

/// Counts the bytes queued in one direction.
#[derive(Default)]
pub(super) struct Gate {
    pub(super) queued: AtomicUsize,
    changed: Notify,
}

impl Gate {
    pub(super) fn add(&self, n: usize) {
        self.queued.fetch_add(n, Ordering::SeqCst);
    }

    pub(super) fn sub(&self, n: usize) {
        self.queued.fetch_sub(n, Ordering::SeqCst);
        self.changed.notify_waiters();
    }

    /// Returns once fewer than [`BACKLOG_CAP`] bytes are queued.
    pub(super) async fn wait_room(&self) {
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

/// The channels and I/O tasks between the two sides and the engine loop.
pub(super) struct Wiring {
    pub(super) rx: mpsc::UnboundedReceiver<Event>,
    pub(super) out: Out,
    pub(super) c2a: Arc<Gate>,
    pub(super) a2c: Arc<Gate>,
    pub(super) client_writer: oneshot::Receiver<()>,
    pub(super) agent_writer: JoinHandle<()>,
}

/// Starts the reader and writer tasks for both directions.
pub(super) fn wire(agent_in: ChildStdin, agent_out: ChildStdout, logger: &Logger) -> Wiring {
    let (tx, rx) = mpsc::unbounded_channel();
    let a2c = Arc::new(Gate::default());
    let c2a = Arc::new(Gate::default());
    let (agent_tx, agent_rx) = mpsc::unbounded_channel::<Vec<u8>>();
    let (client_tx, client_rx) = mpsc::unbounded_channel::<Vec<u8>>();
    let agent_writer = tokio::spawn(write_lines(agent_in, agent_rx, c2a.clone(), logger.clone()));
    let client_writer = spawn_client_writer(client_rx, a2c.clone(), tx.clone(), logger.clone());
    tokio::spawn(read_lines(
        tokio::io::stdin(),
        tx.clone(),
        c2a.clone(),
        Lane {
            line: Event::Client,
            oversized: Event::ClientOversized,
            closed: || Event::ClientClosed,
        },
        MAX_LINE,
    ));
    tokio::spawn(read_lines(
        agent_out,
        tx,
        a2c.clone(),
        Lane {
            line: Event::Agent,
            oversized: Event::AgentOversized,
            closed: || Event::AgentClosed,
        },
        MAX_LINE,
    ));
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
pub(super) struct Out {
    pub(super) agent: Option<mpsc::UnboundedSender<Vec<u8>>>,
    pub(super) client: Option<mpsc::UnboundedSender<Vec<u8>>>,
    c2a: Arc<Gate>,
    a2c: Arc<Gate>,
    logger: Logger,
}

impl Out {
    pub(super) fn send(&self, actions: Vec<Action>) {
        for action in actions {
            match action {
                Action::ToAgent(line) => Self::queue(&self.agent, &self.c2a, line),
                Action::ToClient { line, synthesized } => {
                    if synthesized {
                        self.logger.log(Dir::PonsToClient, &line);
                    }
                    Self::queue(&self.client, &self.a2c, line);
                }
                Action::Census { file, snapshot } => {
                    let logger = self.logger.clone();
                    tokio::task::spawn_blocking(move || {
                        if let Err(err) = append_to(&file, &snapshot) {
                            logger.log(Dir::Pons, format!("census: {err}").as_bytes());
                        }
                    });
                }
                Action::Note(note) => self.logger.log(Dir::Pons, note.as_bytes()),
            }
        }
    }

    fn queue(tx: &Option<mpsc::UnboundedSender<Vec<u8>>>, gate: &Gate, line: Vec<u8>) {
        let len = line.len() + 1; // the writer adds the newline
        gate.add(len);
        let sent = tx.as_ref().is_some_and(|tx| tx.send(line).is_ok());
        if !sent {
            gate.sub(len);
        }
    }
}
