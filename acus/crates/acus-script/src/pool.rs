// SPDX-License-Identifier: MPL-2.0
//! Runs host calls on other threads while the single-threaded engine waits for their results.

use crate::{Host, HostError};
use serde_json::Value;
use std::collections::VecDeque;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::Instant;

/// Most host calls in flight at once.
const MAX_CONCURRENCY: usize = 8;

/// Native stack of a worker thread, as large as a main thread's, since a command may recurse.
const WORKER_STACK: usize = 8 << 20;

struct Call {
    id: u32,
    name: String,
    args: String,
    exclusive: bool,
}

/// A finished call: its JSON result text, or the host's error.
pub(crate) struct Done {
    pub(crate) id: u32,
    pub(crate) result: Result<String, HostError>,
    exclusive: bool,
}

/// Starts submitted calls in program order, at most [`MAX_CONCURRENCY`] at a time.
///
/// An exclusive call starts only when nothing runs and holds back every call submitted after it
/// until it ends, so exclusive calls and the calls around them keep the order of their submission.
pub(crate) struct Calls<H> {
    host: Arc<H>,
    queue: VecDeque<Call>,
    running: usize,
    exclusive_running: bool,
    cap: usize,
    deadline: Instant,
    tx: Sender<Done>,
    rx: Receiver<Done>,
    unawaited: Vec<Value>,
}

impl<H: Host> Calls<H> {
    pub(crate) fn new(host: Arc<H>, deadline: Instant) -> Self {
        let (tx, rx) = channel();
        let cores = std::thread::available_parallelism().map_or(4, std::num::NonZero::get);
        Calls {
            host,
            queue: VecDeque::new(),
            running: 0,
            exclusive_running: false,
            cap: cores.min(MAX_CONCURRENCY),
            deadline,
            tx,
            rx,
            unawaited: Vec::new(),
        }
    }

    pub(crate) fn submit(&mut self, id: u32, name: String, args: String) {
        let exclusive = self.host.exclusive(&name);
        self.queue.push_back(Call {
            id,
            name,
            args,
            exclusive,
        });
    }

    /// Returns whether no call is queued or running.
    pub(crate) fn idle(&self) -> bool {
        self.running == 0 && self.queue.is_empty()
    }

    /// Forgets the calls that have not started.
    pub(crate) fn drop_queued(&mut self) {
        self.queue.clear();
    }

    /// Keeps the value of a finished call whose result the program never received.
    pub(crate) fn keep(&mut self, done: Done) {
        if let Ok(text) = done.result
            && let Ok(value) = serde_json::from_str(&text)
        {
            self.unawaited.push(value);
        }
    }

    /// Returns the values kept by [`Calls::keep`], in the order the calls finished.
    pub(crate) fn take_unawaited(&mut self) -> Vec<Value> {
        std::mem::take(&mut self.unawaited)
    }

    /// Starts what may start before the deadline, then waits for a call to finish; `None` when
    /// `until` passes first.
    pub(crate) fn wait(&mut self, until: Instant) -> Option<Done> {
        if Instant::now() < self.deadline {
            self.start_ready();
        }
        let left = until.saturating_duration_since(Instant::now());
        let done = self.rx.recv_timeout(left).ok()?;
        self.running -= 1;
        if done.exclusive {
            self.exclusive_running = false;
        }
        Some(done)
    }

    fn start_ready(&mut self) {
        while let Some(front) = self.queue.front() {
            let may_start = if front.exclusive {
                self.running == 0
            } else {
                !self.exclusive_running && self.running < self.cap
            };
            if !may_start {
                break;
            }
            let Some(call) = self.queue.pop_front() else {
                break;
            };
            self.running += 1;
            self.exclusive_running = call.exclusive;
            self.spawn(call);
        }
    }

    fn spawn(&self, call: Call) {
        let (id, exclusive) = (call.id, call.exclusive);
        let host = Arc::clone(&self.host);
        let tx = self.tx.clone();
        let deadline = self.deadline;
        let started = std::thread::Builder::new()
            .stack_size(WORKER_STACK)
            .spawn(move || {
                // A panic becomes an error where the build unwinds; `panic = "abort"` ends the
                // process before this runs.
                let result = catch_unwind(AssertUnwindSafe(|| invoke(&*host, &call, deadline)))
                    .unwrap_or_else(|_| Err(HostError::new(format!("{}() panicked", call.name))));
                // The receiver is gone once the engine has stopped waiting.
                let _ = tx.send(Done {
                    id: call.id,
                    result,
                    exclusive: call.exclusive,
                });
            });
        if let Err(e) = started {
            let _ = self.tx.send(Done {
                id,
                result: Err(HostError::new(format!("cannot start a thread: {e}"))),
                exclusive,
            });
        }
    }
}

fn invoke<H: Host>(host: &H, call: &Call, deadline: Instant) -> Result<String, HostError> {
    let args: Vec<Value> = serde_json::from_str(&call.args)
        .map_err(|e| HostError::new(format!("bad arguments to {}: {e}", call.name)))?;
    host.call(&call.name, args, deadline)
        .map(|value| value.to_string())
}
