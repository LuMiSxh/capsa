// SPDX-License-Identifier: MPL-2.0

use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::time::{sleep, timeout};

/// Runs `pons` and drives its stdio as the client.
pub(crate) struct Harness {
    pub(crate) child: Child,
    pub(crate) stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

impl Harness {
    pub(crate) fn start(extra: &[&str]) -> Self {
        let mut command = pons_command(extra);
        command.arg("--").arg(mock_agent());
        Self::spawn(command)
    }

    pub(crate) fn spawn(mut command: Command) -> Self {
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .kill_on_drop(true)
            .spawn()
            .expect("spawn pons");
        let stdin = child.stdin.take().unwrap();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        Self {
            child,
            stdin,
            stdout,
        }
    }

    pub(crate) async fn send(&mut self, msg: Value) {
        self.send_raw(&msg.to_string()).await;
    }

    pub(crate) async fn send_raw(&mut self, line: &str) {
        self.stdin.write_all(line.as_bytes()).await.unwrap();
        self.stdin.write_all(b"\n").await.unwrap();
        self.stdin.flush().await.unwrap();
    }

    /// Returns the next line pons writes, without its newline.
    pub(crate) async fn recv_line(&mut self) -> String {
        let mut line = String::new();
        let n = timeout(Duration::from_secs(5), self.stdout.read_line(&mut line))
            .await
            .expect("timed out waiting for pons output")
            .unwrap();
        assert!(n > 0, "pons closed stdout");
        line.trim_end_matches('\n').to_owned()
    }

    pub(crate) async fn recv(&mut self) -> Value {
        serde_json::from_str(&self.recv_line().await).unwrap()
    }

    /// Reads messages until `done` accepts one; returns all of them.
    pub(crate) async fn recv_until(&mut self, done: impl Fn(&Value) -> bool) -> Vec<Value> {
        let mut seen = Vec::new();
        loop {
            let msg = self.recv().await;
            let stop = done(&msg);
            seen.push(msg);
            if stop {
                return seen;
            }
        }
    }

    pub(crate) async fn prompt(&mut self, id: i64, text: &str) {
        self.send(
            json!({"jsonrpc": "2.0", "id": id, "method": "session/prompt",
            "params": {"sessionId": "s1", "prompt": [{"type": "text", "text": text}]}}),
        )
        .await;
    }

    pub(crate) async fn cancel(&mut self) {
        self.send(json!({"jsonrpc": "2.0", "method": "session/cancel",
            "params": {"sessionId": "s1"}}))
            .await;
    }

    /// Starts the subagent scenario and waits until the folded log is complete.
    pub(crate) async fn start_subagent_turn(&mut self) -> Vec<Value> {
        self.prompt(1, "subagent").await;
        self.recv_until(|m| folded_text(m).is_some_and(|t| t.contains("reviewing the file")))
            .await
    }
}

pub(crate) fn mock_agent() -> PathBuf {
    let pons = PathBuf::from(env!("CARGO_BIN_EXE_pons"));
    let name = format!("mock_agent{}", std::env::consts::EXE_SUFFIX);
    pons.parent().unwrap().join("examples").join(name)
}

/// Builds a `pons` command with the wire log off, so a test run never writes to the
/// user's log directory. A test that passes `--log` or `--log-dir` keeps its own log.
pub(crate) fn pons_command(args: &[&str]) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_pons"));
    if !args.iter().any(|a| a.starts_with("--log")) {
        command.arg("--no-log");
    }
    command.args(args);
    command
}

pub(crate) fn temp_path(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("pons-it-{}-{name}", std::process::id()))
}

pub(crate) fn has_flat_child_update(m: &Value) -> bool {
    m["params"]["update"]["_meta"]["claudeCode"]["parentToolUseId"].is_string()
}

pub(crate) fn is_parent_update(m: &Value) -> bool {
    m["params"]["update"]["toolCallId"] == "toolu_parent"
}

pub(crate) fn folded_text(m: &Value) -> Option<String> {
    let u = &m["params"]["update"];
    (is_parent_update(m) && u["sessionUpdate"] == "tool_call_update")
        .then(|| {
            u["content"][0]["content"]["text"]
                .as_str()
                .map(str::to_owned)
        })
        .flatten()
}

/// Starts a turn whose background subagent is running, and waits for its card.
pub(crate) async fn start_background_turn(h: &mut Harness) {
    h.prompt(1, "background").await;
    h.recv_until(|m| {
        let u = &m["params"]["update"];
        u["toolCallId"] == "toolu_bg" && u["sessionUpdate"] == "tool_call_update"
    })
    .await;
}

/// Sends a request the agent echoes and returns everything that came before
/// the echo: a barrier that shows what else the proxy emitted by then.
pub(crate) async fn barrier(h: &mut Harness, id: i64) -> Vec<Value> {
    h.send(json!({"jsonrpc": "2.0", "id": id, "method": "echo/barrier", "params": {}}))
        .await;
    h.recv_until(|m| m["id"] == id).await
}

pub(crate) fn is_cancel_seen(m: &Value) -> bool {
    m["method"] == "mock/cancel_seen"
}

pub(crate) fn is_stop_mark(m: &Value) -> bool {
    let u = &m["params"]["update"];
    u["status"] == "failed"
        || u["toolCallId"]
            .as_str()
            .is_some_and(|i| i.starts_with("pons-finished"))
}

/// Counts the responses (result or error) that answer request `id`.
pub(crate) fn answers(seen: &[Value], id: i64) -> usize {
    seen.iter()
        .filter(|m| m["id"] == id && m["method"].is_null())
        .count()
}

pub(crate) async fn initialize_agent(h: &mut Harness) -> Value {
    h.send(json!({"jsonrpc": "2.0", "id": 0, "method": "initialize",
        "params": {"protocolVersion": 1, "clientCapabilities": {}}}))
        .await;
    h.recv().await
}

/// Sends a message the way Zed does: `session/cancel`, then `session/prompt` about
/// 100 ms later.
pub(crate) async fn send_message(h: &mut Harness, id: i64, text: &str) {
    h.cancel().await;
    sleep(Duration::from_millis(100)).await;
    h.prompt(id, text).await;
}

/// Starts `pons` in front of the mock agent, with extra arguments for both.
pub(crate) fn start_with_agent_args(pons_args: &[&str], agent_args: &[&str]) -> Harness {
    let mut command = pons_command(pons_args);
    command.arg("--").arg(mock_agent()).args(agent_args);
    Harness::spawn(command)
}
