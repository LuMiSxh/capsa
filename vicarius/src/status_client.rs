// SPDX-License-Identifier: MPL-2.0
//! Talks to the running service over localhost with one plain HTTP/1.1 request on a TCP socket,
//! and reads its status leniently: a key that a service does not send is a default, not an error.

use std::io::{self, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::Deserialize;
use serde_json::Value;

/// The status endpoint of the service.
pub const STATUS: &str = "/vicarius/status";
/// The endpoint that runs one balance refresh and one agent sync.
pub const REFRESH: &str = "/vicarius/refresh";

/// The status that `GET /vicarius/status` answers with.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct ServiceStatus {
    pub started_at: Option<u64>,
    pub gate: GateStatus,
    pub counters: Counters,
    pub agents: Option<AgentsStatus>,
    pub refresh: Option<RefreshStatus>,
}

/// Whether the DeepSeek route is open, and the last balance.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct GateStatus {
    pub open: bool,
    pub reason: Option<String>,
    pub reason_text: Option<String>,
    pub balance: Option<f64>,
    pub currency: Option<String>,
    pub checked_at: Option<u64>,
}

/// The request counters of both routes.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct Counters {
    pub anthropic: Route,
    pub deepseek: Route,
}

/// The counters of one route.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct Route {
    pub requests: u64,
    pub refused: u64,
    pub input_tokens: u64,
    pub cache_creation_tokens: u64,
    pub cache_read_tokens: u64,
    pub output_tokens: u64,
}

/// The outcome of the last agent sync, one entry per derived agent file.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct AgentsStatus {
    pub entries: Vec<AgentStatus>,
}

/// One derived agent file and what the sync did with it.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct AgentStatus {
    pub agent: String,
    pub file: String,
    /// A name such as `"written"`, or an object with a message such as `{"unparsable": "..."}`.
    pub outcome: Value,
}

impl AgentStatus {
    /// Returns the name of the outcome, such as `written`, or `unparsable` for an outcome with a
    /// message.
    pub fn outcome_name(&self) -> &str {
        match &self.outcome {
            Value::String(name) => name,
            Value::Object(map) => map.keys().next().map_or("", String::as_str),
            _ => "",
        }
    }

    /// Returns the message of an outcome that carries one.
    pub fn outcome_detail(&self) -> Option<&str> {
        self.outcome.as_object()?.values().next()?.as_str()
    }
}

/// The refresh times and the last refresh error.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct RefreshStatus {
    pub last_ok_at: Option<u64>,
    pub last_error: Option<String>,
    pub next_at: Option<u64>,
}

/// A status answer, with the JSON as the service wrote it.
pub struct Fetched {
    pub raw: Value,
    pub status: ServiceStatus,
}

/// Reads the status of the service on `port`. Returns `None` unless the service answers 200 with a
/// status object within `timeout`.
pub fn fetch(port: u16, timeout: Duration) -> Option<Fetched> {
    let reply = request(port, "GET", STATUS, timeout).ok()?;
    if reply.status != 200 {
        return None;
    }
    let raw: Value = serde_json::from_slice(&reply.body).ok()?;
    let status = ServiceStatus::deserialize(&raw).ok()?;
    Some(Fetched { raw, status })
}

/// The status line and body of one reply.
#[derive(Debug)]
pub struct Reply {
    pub status: u16,
    pub body: Vec<u8>,
}

/// Sends one request with `method` for `path` to the service on `port`, and reads the whole reply.
///
/// # Errors
/// Fails when the whole request, connection to final read included, does not finish within
/// `timeout`, or when the reply is not HTTP/1.x.
pub fn request(port: u16, method: &str, path: &str, timeout: Duration) -> io::Result<Reply> {
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    let deadline = Instant::now() + timeout;
    let mut stream = TcpStream::connect_timeout(&addr, remaining(deadline)?)?;
    stream.set_write_timeout(Some(remaining(deadline)?))?;
    write!(
        stream,
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\nContent-Length: 0\r\n\r\n"
    )?;
    let mut raw = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        if let Some(reply) = parse(&raw, false)? {
            return Ok(reply);
        }
        // Each read gets only what is left of the budget, so a reply that trickles in cannot
        // stretch the call past the deadline.
        stream.set_read_timeout(Some(remaining(deadline)?))?;
        let read = stream.read(&mut chunk)?;
        if read == 0 {
            return parse(&raw, true)?.ok_or_else(|| invalid("the reply ended early"));
        }
        raw.extend_from_slice(&chunk[..read]);
    }
}

/// Returns the time left until `deadline`, or a timeout error once it has passed.
fn remaining(deadline: Instant) -> io::Result<Duration> {
    let left = deadline.saturating_duration_since(Instant::now());
    if left.is_zero() {
        return Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "the request timed out",
        ));
    }
    Ok(left)
}

/// Parses the reply in `raw`. Returns `None` while the reply is incomplete, unless `eof` says the
/// connection has closed, in which case the rest of the bytes is the body.
fn parse(raw: &[u8], eof: bool) -> io::Result<Option<Reply>> {
    let Some(split) = raw.windows(4).position(|w| w == b"\r\n\r\n") else {
        return if eof {
            Err(invalid("the reply has no end of headers"))
        } else {
            Ok(None)
        };
    };
    let head =
        std::str::from_utf8(&raw[..split]).map_err(|_| invalid("the headers are not text"))?;
    let body = &raw[split + 4..];
    let mut lines = head.split("\r\n");
    let status = lines
        .next()
        .filter(|line| line.starts_with("HTTP/1."))
        .and_then(|line| line.split_whitespace().nth(1)?.parse::<u16>().ok())
        .ok_or_else(|| invalid("the reply is not HTTP/1.x"))?;
    let length = lines
        .filter_map(|line| line.split_once(':'))
        .find(|(name, _)| name.trim().eq_ignore_ascii_case("content-length"))
        .and_then(|(_, value)| value.trim().parse::<usize>().ok());
    match length {
        Some(len) if body.len() >= len => Ok(Some(Reply {
            status,
            body: body[..len].to_vec(),
        })),
        Some(_) if eof => Err(invalid("the body is shorter than its Content-Length")),
        Some(_) => Ok(None),
        None if eof => Ok(Some(Reply {
            status,
            body: body.to_vec(),
        })),
        None => Ok(None),
    }
}

fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

/// Formats a balance: `$18.40` for USD, `18.40 CNY` for another currency.
pub fn money(amount: f64, currency: Option<&str>) -> String {
    match currency {
        Some("USD") => format!("${amount:.2}"),
        Some(code) => format!("{amount:.2} {code}"),
        None => format!("{amount:.2}"),
    }
}

/// Describes a unix time relative to now, such as `5 min ago` or `in 8 min`.
pub fn relative(at: u64) -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let (secs, past) = if at <= now {
        (now - at, true)
    } else {
        (at - now, false)
    };
    let span = match secs {
        0..60 => format!("{secs} s"),
        60..3600 => format!("{} min", secs / 60),
        _ => format!("{} h", secs / 3600),
    };
    if past {
        format!("{span} ago")
    } else {
        format!("in {span}")
    }
}

#[cfg(test)]
mod tests {
    use std::io::{Read as _, Write as _};
    use std::net::TcpListener;
    use std::thread;
    use std::time::{Duration, Instant};

    use super::{STATUS, request};

    #[test]
    fn a_reply_that_dribbles_out_times_out_at_the_total_deadline() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        thread::spawn(move || {
            for mut stream in listener.incoming().flatten() {
                let mut head = Vec::new();
                let mut chunk = [0u8; 1024];
                while !head.windows(4).any(|w| w == b"\r\n\r\n") {
                    match stream.read(&mut chunk) {
                        Ok(0) | Err(_) => return,
                        Ok(read) => head.extend_from_slice(&chunk[..read]),
                    }
                }
                for byte in b"HTTP/1.1 200 reply\r\n\r\n" {
                    if stream.write_all(&[*byte]).is_err() {
                        return;
                    }
                    thread::sleep(Duration::from_millis(20));
                }
            }
        });
        let started = Instant::now();
        let reply = request(port, "GET", STATUS, Duration::from_millis(50));
        let elapsed = started.elapsed();
        assert!(reply.is_err(), "a dribbling reply must time out");
        assert!(
            elapsed < Duration::from_millis(100),
            "request took {elapsed:?}"
        );
    }
}
