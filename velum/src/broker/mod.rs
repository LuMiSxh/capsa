// SPDX-License-Identifier: MPL-2.0
//! Owns the approval broker: a unix socket in the run directory where the sandboxed agent
//! asks for actions that need the network or credentials. Each request is approved by the
//! person and then run outside the sandbox.

mod approval;
mod dialog;
mod exec;
mod git;
mod handler;
mod log;
mod repo;
mod request;
mod text;
mod tools;

#[cfg(test)]
mod real_git;
#[cfg(test)]
mod tests;

pub use request::{Response, Status};

use anyhow::{Context, Result};
use handler::Handler;
use std::io::{ErrorKind, Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// The socket's file name inside the run directory.
pub const SOCKET_NAME: &str = "broker.sock";

/// The environment variable that tells the sandboxed side where the socket is.
pub const ENV: &str = "VELUM_BROKER";

/// Below `$HOME`: the broker's own files. The sandbox cannot write this directory.
const DIR: &str = "Library/Application Support/velum";

/// A request line is at most this long.
const MAX_REQUEST: u64 = 256 * 1024;

/// How long a client may take, in total, to send its request line.
const READ_DEADLINE: Duration = Duration::from_secs(30);

/// How long the answer may take to be written.
const WRITE_TIMEOUT: Duration = Duration::from_secs(30);

/// The most connections served at once.
const MAX_CONNECTIONS: usize = 32;

/// How often the accept loop looks at the stop flag.
const POLL: Duration = Duration::from_millis(50);

/// A running broker. When this value drops the accept loop stops and every running action and
/// open dialog is killed.
pub struct Broker {
    stop: Arc<AtomicBool>,
    registry: Arc<exec::Registry>,
}

impl Drop for Broker {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        self.registry.close();
    }
}

/// Where the broker keeps its log.
pub fn log_path(home: &str) -> PathBuf {
    Path::new(home).join(DIR).join("broker.log")
}

/// Where "for this repo" approvals are stored.
pub fn approvals_path(home: &str) -> PathBuf {
    Path::new(home).join(DIR).join("approvals.toml")
}

/// Starts the broker on `socket` for the working directory `cwd`, with the macOS dialog and
/// the real programs. `settings` are the srt settings of the run: the repository around `cwd`
/// is recorded now, and only if the sandbox cannot write its git configuration.
/// Returns the broker and the socket's canonical path.
pub fn start(
    home: &str,
    socket: &Path,
    cwd: &Path,
    settings: &serde_json::Value,
) -> Result<(Broker, PathBuf)> {
    let cwd = std::fs::canonicalize(cwd).context("resolving the working directory")?;
    let registry = Arc::new(exec::Registry::default());
    let handler = Handler {
        repo: repo::Recorded::resolve(&cwd, settings),
        cwd,
        approver: Box::new(dialog::Osascript {
            give_up_secs: dialog::GIVE_UP_SECS,
            registry: Arc::clone(&registry),
        }),
        executor: Box::new(exec::System {
            timeout: exec::TIMEOUT,
            registry: Arc::clone(&registry),
        }),
        memory: approval::Memory::new(approvals_path(home)),
        log: log::Log::new(log_path(home)),
        busy: Mutex::new(()),
    };
    serve(socket, Arc::new(handler), registry)
}

fn serve(
    socket: &Path,
    handler: Arc<Handler>,
    registry: Arc<exec::Registry>,
) -> Result<(Broker, PathBuf)> {
    let listener = UnixListener::bind(socket)
        .with_context(|| format!("binding the broker socket {}", socket.display()))?;
    std::fs::set_permissions(socket, std::fs::Permissions::from_mode(0o600))
        .context("restricting the broker socket")?;
    listener.set_nonblocking(true)?;
    let path = std::fs::canonicalize(socket).unwrap_or_else(|_| socket.to_path_buf());
    let stop = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&stop);
    let active = Arc::new(AtomicUsize::new(0));
    std::thread::spawn(move || {
        while !flag.load(Ordering::SeqCst) {
            match listener.accept() {
                Ok((stream, _)) => spawn_connection(&handler, &active, stream),
                Err(_) => std::thread::sleep(POLL),
            }
        }
    });
    Ok((Broker { stop, registry }, path))
}

/// Counts an open connection and releases it when dropped.
struct Slot(Arc<AtomicUsize>);

impl Drop for Slot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Serves `stream` on its own thread, so a client that stalls blocks nobody else. Past
/// `MAX_CONNECTIONS` the client is told to try again.
fn spawn_connection(handler: &Arc<Handler>, active: &Arc<AtomicUsize>, stream: UnixStream) {
    if active.fetch_add(1, Ordering::SeqCst) >= MAX_CONNECTIONS {
        active.fetch_sub(1, Ordering::SeqCst);
        reply(
            &stream,
            &Response::error("busy: too many connections; try again later"),
        );
        return;
    }
    let slot = Slot(Arc::clone(active));
    let handler = Arc::clone(handler);
    std::thread::spawn(move || {
        let _slot = slot;
        answer(&handler, &stream);
    });
}

/// Reads the request line from `stream` within `READ_DEADLINE` in total, however slowly the
/// client sends it. `Ok(None)` is a client that sent nothing.
fn read_request(stream: &UnixStream) -> Result<Option<String>, String> {
    let deadline = Instant::now() + READ_DEADLINE;
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err("the request did not arrive in time".to_string());
        }
        stream
            .set_read_timeout(Some(left))
            .map_err(|err| err.to_string())?;
        match (&*stream).read(&mut chunk) {
            Ok(0) if buffer.is_empty() => return Ok(None),
            Ok(0) => return Err("the request is incomplete".to_string()),
            Ok(n) => {
                buffer.extend_from_slice(&chunk[..n]);
                if let Some(end) = buffer.iter().position(|byte| *byte == b'\n') {
                    buffer.truncate(end);
                    return String::from_utf8(buffer)
                        .map(Some)
                        .map_err(|err| err.to_string());
                }
                if buffer.len() as u64 > MAX_REQUEST {
                    return Err("the request is too long".to_string());
                }
            }
            Err(err) if err.kind() == ErrorKind::Interrupted => {}
            Err(err) if matches!(err.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {
                return Err("the request did not arrive in time".to_string());
            }
            Err(err) => return Err(err.to_string()),
        }
    }
}

fn reply(stream: &UnixStream, response: &Response) {
    let _ = stream.set_write_timeout(Some(WRITE_TIMEOUT));
    if let Ok(text) = serde_json::to_string(response) {
        let _ = writeln!(&mut &*stream, "{text}");
    }
}

/// Reads one request line from `stream`, answers it and closes the connection.
fn answer(handler: &Handler, stream: &UnixStream) {
    let _ = stream.set_nonblocking(false);
    let response = match read_request(stream) {
        Ok(None) => return,
        Ok(Some(line)) => handler.handle(&line),
        Err(why) => Response::error(why),
    };
    reply(stream, &response);
}
