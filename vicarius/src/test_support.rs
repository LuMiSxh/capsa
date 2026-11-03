// SPDX-License-Identifier: MPL-2.0
//! Test doubles for the binary's unit tests: a localhost service, a `launchctl` that records its
//! calls, and the install paths under a scratch directory.

use std::cell::{Cell, RefCell};
use std::io::{self, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::thread;

use crate::service::{Call, Install, Launchctl};

/// Serves `routes` on 127.0.0.1 and returns the port. A route is `METHOD /path`, a status and a
/// body; any other request gets a 404. The server thread lives until the test process exits.
pub fn serve(routes: Vec<(&'static str, u16, &'static str)>) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap_or_else(|e| panic!("cannot bind: {e}"));
    let port = listener.local_addr().map_or(0, |addr| addr.port());
    thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            answer(stream, &routes);
        }
    });
    port
}

fn answer(mut stream: TcpStream, routes: &[(&str, u16, &str)]) {
    let mut head = Vec::new();
    let mut chunk = [0u8; 1024];
    while !head.windows(4).any(|w| w == b"\r\n\r\n") {
        match stream.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(read) => head.extend_from_slice(&chunk[..read]),
        }
    }
    let text = String::from_utf8_lossy(&head);
    let key = text
        .lines()
        .next()
        .unwrap_or("")
        .split_whitespace()
        .take(2)
        .collect::<Vec<_>>()
        .join(" ");
    let (status, body) = routes
        .iter()
        .find(|(route, _, _)| *route == key)
        .map_or((404, ""), |(_, status, body)| (*status, *body));
    let reply = format!(
        "HTTP/1.1 {status} reply\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(reply.as_bytes());
}

/// Returns a port that nothing listens on, for a service that is down.
pub fn free_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap_or_else(|e| panic!("cannot bind: {e}"));
    let port = listener.local_addr().map_or(0, |addr| addr.port());
    drop(listener);
    port
}

/// A `launchctl` that records its calls. `bootstrap` and `kickstart` load the agent, `bootout`
/// unloads it, and `print` exits 0 only while it is loaded. After a `bootout`, `print` keeps
/// reporting the agent loaded for `linger` more calls, as launchd does while it tears a job down.
#[derive(Default)]
pub struct FakeLaunchctl {
    pub loaded: Cell<bool>,
    pub linger: Cell<u32>,
    pub calls: RefCell<Vec<String>>,
}

impl FakeLaunchctl {
    fn lingers(&self) -> bool {
        let left = self.linger.get();
        self.linger.set(left.saturating_sub(1));
        left > 0
    }
}

impl Launchctl for FakeLaunchctl {
    fn run(&self, args: &[&str]) -> io::Result<Call> {
        self.calls.borrow_mut().push(args.join(" "));
        let ok = match args.first().copied() {
            Some("print") => self.loaded.get() || self.lingers(),
            Some("bootstrap" | "kickstart") => {
                self.loaded.set(true);
                true
            }
            Some("bootout") => self.loaded.replace(false),
            _ => false,
        };
        Ok(Call {
            ok,
            stderr: String::new(),
        })
    }
}

/// The install paths under `dir`, with the agent in the `gui/501` domain.
pub fn install_in(dir: &Path) -> Install {
    Install {
        plist: dir.join("LaunchAgents/io.github.lumisxh.vicarius.plist"),
        log: dir.join("state/service.log"),
        settings: dir.join("settings.json"),
        config: dir.join("config.json"),
        exe: dir.join("bin dir/vicarius"),
        domain: Some("gui/501".to_owned()),
    }
}
