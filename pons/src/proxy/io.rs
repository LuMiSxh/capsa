// SPDX-License-Identifier: MPL-2.0
//! Writes and reads the stdio lines, one ACP message per line.

use super::*;

pub(super) async fn write_lines<W: AsyncWrite + Unpin>(
    mut writer: W,
    mut rx: mpsc::UnboundedReceiver<Vec<u8>>,
    gate: Arc<Gate>,
    failed: Option<mpsc::UnboundedSender<Event>>,
) {
    let mut broken = false;
    while let Some(line) = rx.recv().await {
        if !broken {
            let ok = writer.write_all(&line).await.is_ok() && writer.flush().await.is_ok();
            if !ok {
                // A dead agent pipe is picked up by the child's exit status; a
                // dead client side ends the proxy.
                broken = true;
                if let Some(tx) = &failed {
                    let _ = tx.send(Event::ClientWriteFailed);
                }
            }
        }
        gate.sub(line.len());
    }
    let _ = writer.shutdown().await;
}

pub(super) async fn read_lines<R: AsyncRead + Unpin>(
    reader: R,
    tx: mpsc::UnboundedSender<Event>,
    gate: Arc<Gate>,
    line: fn(Vec<u8>) -> Event,
    closed: fn() -> Event,
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
                if tx.send(line(buf)).is_err() {
                    return;
                }
            }
        }
    }
    let _ = tx.send(closed());
}
