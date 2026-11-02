// SPDX-License-Identifier: MPL-2.0

use super::*;

fn lane() -> Lane {
    Lane {
        line: Event::Agent,
        oversized: Event::AgentOversized,
        closed: || Event::AgentClosed,
    }
}

async fn read_all(input: &'static [u8], max: usize) -> Vec<Event> {
    let (tx, mut rx) = mpsc::unbounded_channel();
    read_lines(input, tx, Arc::new(Gate::default()), lane(), max).await;
    let mut events = Vec::new();
    while let Ok(event) = rx.try_recv() {
        events.push(event);
    }
    events
}

#[tokio::test]
async fn a_line_over_the_cap_is_handed_over_whole_and_marked() {
    let events = read_all(b"short\n0123456789abcdef\nlast\n", 10).await;
    assert_eq!(events.len(), 4);
    assert!(matches!(&events[0], Event::Agent(l) if l == b"short"));
    assert!(matches!(&events[1], Event::AgentOversized(l) if l == b"0123456789abcdef"));
    assert!(matches!(&events[2], Event::Agent(l) if l == b"last"));
    assert!(matches!(events[3], Event::AgentClosed));
}

#[tokio::test]
async fn a_line_at_the_cap_is_not_oversized() {
    let events = read_all(b"0123456789\n", 10).await;
    assert!(matches!(&events[0], Event::Agent(l) if l == b"0123456789"));
}

/// A writer that records each `write` call and `flush`.
#[derive(Clone, Default)]
struct Calls(Arc<std::sync::Mutex<Vec<String>>>);

impl std::io::Write for Calls {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0
            .lock()
            .unwrap()
            .push(format!("write:{}", String::from_utf8_lossy(buf)));
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.0.lock().unwrap().push("flush".into());
        Ok(())
    }
}

#[test]
fn the_client_writer_keeps_order_and_flushes_once_the_queue_is_empty() {
    let (tx, rx) = mpsc::unbounded_channel();
    let gate = Gate::default();
    for l in ["one", "two", "three"] {
        gate.add(l.len() + 1);
        tx.send(l.as_bytes().to_vec()).unwrap();
    }
    drop(tx);
    let calls = Calls::default();
    client_thread(calls.clone(), rx, &gate, |_| panic!("no failure expected"));
    let calls = calls.0.lock().unwrap().clone();
    assert_eq!(calls, ["write:one\ntwo\nthree\n", "flush", "flush"]);
    assert_eq!(gate.queued.load(Ordering::SeqCst), 0, "every byte released");
}

struct Broken;

impl std::io::Write for Broken {
    fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
        Err(std::io::ErrorKind::BrokenPipe.into())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Err(std::io::ErrorKind::BrokenPipe.into())
    }
}

#[test]
fn a_broken_client_pipe_is_reported_once_and_the_queue_still_drains() {
    let (tx, rx) = mpsc::unbounded_channel();
    let gate = Gate::default();
    for _ in 0..3 {
        gate.add(2);
        tx.send(b"x".to_vec()).unwrap();
    }
    drop(tx);
    let mut breaks = 0;
    client_thread(Broken, rx, &gate, |_| breaks += 1);
    assert_eq!(breaks, 1);
    assert_eq!(gate.queued.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn the_agent_writer_adds_the_newline_and_logs_a_broken_pipe_once() {
    let path = std::env::temp_dir().join(format!("pons-writer-{}.log", std::process::id()));
    let _ = std::fs::remove_file(&path);
    let logger = Logger::open(Some(&path), crate::log::LogLevel::All, 0).unwrap();
    let (tx, rx) = mpsc::unbounded_channel();
    let gate = Arc::new(Gate::default());
    for _ in 0..3 {
        gate.add(2);
        tx.send(b"x".to_vec()).unwrap();
    }
    drop(tx);
    let (broken, _keep) = tokio::io::duplex(1);
    drop(_keep);
    write_lines(broken, rx, gate.clone(), logger).await;
    let log = std::fs::read_to_string(&path).unwrap();
    let _ = std::fs::remove_file(&path);
    assert_eq!(
        log.matches("fail-safe: writing to the agent failed")
            .count(),
        1
    );
    assert_eq!(gate.queued.load(Ordering::SeqCst), 0);
}
