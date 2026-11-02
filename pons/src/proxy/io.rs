// SPDX-License-Identifier: MPL-2.0
//! Writes and reads the stdio lines, one ACP message per line.
//!
//! Queued lines carry no newline: the writers add it, so a line the engine forwarded
//! unchanged is never copied to make room for it. Every queued line counts `len + 1`
//! bytes in its [`Gate`].

use std::io::Write;

use tokio::io::BufWriter;

use super::*;

/// Longest line the engine parses. A longer one is forwarded unchanged and unparsed.
pub(super) const MAX_LINE: usize = 32 * 1024 * 1024;
/// Bytes a writer collects before it writes to the pipe.
const WRITE_BUFFER: usize = 64 * 1024;

/// Logs once that a writer broke; later lines for that side are dropped.
fn note_broken(logger: &Logger, side: &str, err: &std::io::Error) {
    logger.log(
        Dir::Pons,
        format!("fail-safe: writing to the {side} failed ({err}); its later lines are dropped")
            .as_bytes(),
    );
}

/// Writes the queued lines to the agent. It flushes when the queue is empty, so a line
/// waits only while more are ready.
pub(super) async fn write_lines<W: AsyncWrite + Unpin>(
    writer: W,
    mut rx: mpsc::UnboundedReceiver<Vec<u8>>,
    gate: Arc<Gate>,
    logger: Logger,
) {
    let mut writer = BufWriter::with_capacity(WRITE_BUFFER, writer);
    let mut broken = false;
    while let Some(first) = rx.recv().await {
        let mut next = Some(first);
        while let Some(line) = next.take() {
            if !broken && let Err(err) = put(&mut writer, &line).await {
                // A dead agent pipe is also picked up by the child's exit status.
                broken = true;
                note_broken(&logger, "agent", &err);
            }
            gate.sub(line.len() + 1);
            next = rx.try_recv().ok();
        }
        if !broken && let Err(err) = writer.flush().await {
            broken = true;
            note_broken(&logger, "agent", &err);
        }
    }
    let _ = writer.shutdown().await;
}

async fn put<W: AsyncWrite + Unpin>(writer: &mut W, line: &[u8]) -> std::io::Result<()> {
    writer.write_all(line).await?;
    writer.write_all(b"\n").await
}

/// Writes the queued lines to the client on a thread of its own, so that stdout does not
/// go through the blocking pool. It flushes when the queue is empty; a failure is reported
/// once on `failed`, which ends the proxy. The returned receiver resolves when the thread
/// has written what was queued and ended.
pub(super) fn spawn_client_writer(
    rx: mpsc::UnboundedReceiver<Vec<u8>>,
    gate: Arc<Gate>,
    failed: mpsc::UnboundedSender<Event>,
    logger: Logger,
) -> oneshot::Receiver<()> {
    let (done, finished) = oneshot::channel();
    // Without a thread the sender is dropped with the closure and the receiver resolves.
    let _ = std::thread::Builder::new()
        .name("pons-stdout".into())
        .spawn(move || {
            client_thread(std::io::stdout(), rx, &gate, |err| {
                note_broken(&logger, "client", err);
                let _ = failed.send(Event::ClientWriteFailed);
            });
            let _ = done.send(());
        });
    finished
}

pub(super) fn client_thread<W: Write>(
    out: W,
    mut rx: mpsc::UnboundedReceiver<Vec<u8>>,
    gate: &Gate,
    mut on_break: impl FnMut(&std::io::Error),
) {
    let mut out = std::io::BufWriter::with_capacity(WRITE_BUFFER, out);
    let mut broken = false;
    while let Some(first) = rx.blocking_recv() {
        let mut next = Some(first);
        while let Some(line) = next.take() {
            if !broken && let Err(err) = out.write_all(&line).and_then(|()| out.write_all(b"\n")) {
                broken = true;
                on_break(&err);
            }
            gate.sub(line.len() + 1);
            next = rx.try_recv().ok();
        }
        if !broken && let Err(err) = out.flush() {
            broken = true;
            on_break(&err);
        }
    }
    let _ = out.flush();
}

/// How a reader hands over its lines.
#[derive(Clone, Copy)]
pub(super) struct Lane {
    /// Builds the event of a line within the cap.
    pub(super) line: fn(Vec<u8>) -> Event,
    /// Builds the event of a longer line, which the engine does not parse.
    pub(super) oversized: fn(Vec<u8>) -> Event,
    pub(super) closed: fn() -> Event,
}

pub(super) async fn read_lines<R: AsyncRead + Unpin>(
    reader: R,
    tx: mpsc::UnboundedSender<Event>,
    gate: Arc<Gate>,
    lane: Lane,
    max: usize,
) {
    let mut reader = BufReader::new(reader);
    loop {
        gate.wait_room().await;
        let mut buf = Vec::new();
        match reader.read_until(b'\n', &mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(_) => {
                if buf.last() == Some(&b'\n') {
                    buf.pop();
                }
                if buf.is_empty() {
                    continue;
                }
                gate.add(buf.len() + 1);
                let event = if buf.len() > max {
                    (lane.oversized)(buf)
                } else {
                    (lane.line)(buf)
                };
                if tx.send(event).is_err() {
                    return;
                }
            }
        }
    }
    let _ = tx.send((lane.closed)());
}
