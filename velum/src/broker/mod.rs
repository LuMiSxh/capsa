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
mod request;
mod text;
mod tools;

#[cfg(test)]
mod tests;

pub use request::{Response, Status};

use anyhow::{Context, Result};
use handler::Handler;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// The socket's file name inside the run directory.
pub const SOCKET_NAME: &str = "broker.sock";

/// The environment variable that tells the sandboxed side where the socket is.
pub const ENV: &str = "VELUM_BROKER";

/// Below `$HOME`: the broker's own files. The sandbox cannot write this directory.
const DIR: &str = "Library/Application Support/velum";

/// A request line is at most this long.
const MAX_REQUEST: u64 = 256 * 1024;

/// How long a client may take to send its request line.
const READ_TIMEOUT: Duration = Duration::from_secs(30);

/// How often the accept loop looks at the stop flag.
const POLL: Duration = Duration::from_millis(50);

/// A running broker. The thread stops when this value drops; a request in flight is cut
/// off when velum exits.
pub struct Broker {
    stop: Arc<AtomicBool>,
}

impl Drop for Broker {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
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
/// the real programs. Returns the broker and the socket's canonical path.
pub fn start(home: &str, socket: &Path, cwd: &Path) -> Result<(Broker, PathBuf)> {
    let cwd = std::fs::canonicalize(cwd).context("resolving the working directory")?;
    let handler = Handler {
        cwd,
        approver: Box::new(dialog::Osascript {
            give_up_secs: dialog::GIVE_UP_SECS,
        }),
        executor: Box::new(exec::System {
            timeout: exec::TIMEOUT,
        }),
        memory: approval::Memory::new(approval::Store::new(approvals_path(home))),
        log: log::Log::new(log_path(home)),
    };
    serve(socket, handler)
}

fn serve(socket: &Path, handler: Handler) -> Result<(Broker, PathBuf)> {
    let listener = UnixListener::bind(socket)
        .with_context(|| format!("binding the broker socket {}", socket.display()))?;
    std::fs::set_permissions(socket, std::fs::Permissions::from_mode(0o600))
        .context("restricting the broker socket")?;
    listener.set_nonblocking(true)?;
    let path = std::fs::canonicalize(socket).unwrap_or_else(|_| socket.to_path_buf());
    let stop = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&stop);
    std::thread::spawn(move || {
        while !flag.load(Ordering::SeqCst) {
            match listener.accept() {
                Ok((stream, _)) => answer(&handler, &stream),
                Err(_) => std::thread::sleep(POLL),
            }
        }
    });
    Ok((Broker { stop }, path))
}

/// Reads one request line from `stream`, answers it and closes the connection. Requests are
/// served one at a time, so at most one dialog is open.
fn answer(handler: &Handler, stream: &UnixStream) {
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_read_timeout(Some(READ_TIMEOUT));
    let _ = stream.set_write_timeout(Some(READ_TIMEOUT));
    let mut line = String::new();
    let read = BufReader::new(stream.take(MAX_REQUEST)).read_line(&mut line);
    let response = match read {
        Ok(0) => return,
        Ok(_) if line.ends_with('\n') => handler.handle(line.trim_end()),
        Ok(_) => Response::error("the request is incomplete or too long"),
        Err(err) => Response::error(format!("could not read the request: {err}")),
    };
    if let Ok(text) = serde_json::to_string(&response) {
        let _ = writeln!(&mut &*stream, "{text}");
    }
}
