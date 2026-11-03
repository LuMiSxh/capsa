// SPDX-License-Identifier: MPL-2.0
//! Reads token usage from a response while its bytes pass, without holding them back.

use reqwest::StatusCode;
use reqwest::header::{CONTENT_ENCODING, CONTENT_TYPE, HeaderMap};
use serde::{Deserialize, Serialize};

/// The most bytes the tap keeps for one line or one JSON body.
const KEEP_LIMIT: usize = 1 << 20;

/// Counts the tokens of one request.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Usage {
    pub input_tokens: u64,
    pub cache_creation_tokens: u64,
    pub cache_read_tokens: u64,
    pub output_tokens: u64,
}

#[derive(Deserialize, Default)]
struct RawUsage {
    input_tokens: Option<u64>,
    cache_creation_input_tokens: Option<u64>,
    cache_read_input_tokens: Option<u64>,
    output_tokens: Option<u64>,
}

#[derive(Deserialize)]
struct Start {
    message: Option<Message>,
}

#[derive(Deserialize)]
struct Message {
    usage: Option<RawUsage>,
}

#[derive(Deserialize)]
struct Delta {
    usage: Option<RawUsage>,
}

#[derive(Deserialize)]
struct Whole {
    usage: Option<RawUsage>,
    input_tokens: Option<u64>,
}

impl Usage {
    fn apply(&mut self, raw: &RawUsage) {
        let set = |slot: &mut u64, value: Option<u64>| {
            if let Some(v) = value {
                *slot = v;
            }
        };
        set(&mut self.input_tokens, raw.input_tokens);
        set(
            &mut self.cache_creation_tokens,
            raw.cache_creation_input_tokens,
        );
        set(&mut self.cache_read_tokens, raw.cache_read_input_tokens);
        set(&mut self.output_tokens, raw.output_tokens);
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Sse,
    Json,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Event {
    Start,
    Delta,
    Other,
}

/// Collects the usage of one response from the bytes handed to `feed`.
pub struct UsageTap {
    mode: Mode,
    usage: Usage,
    /// The unfinished line (SSE) or the body so far (JSON).
    pending: Vec<u8>,
    event: Event,
    /// Set while the rest of an oversized line or body is being skipped.
    skipping: bool,
}

impl UsageTap {
    /// Returns a tap for a successful response whose bytes are readable, else `None`.
    ///
    /// A compressed body is not readable: it passes through untouched and counts no tokens.
    pub fn for_response(status: StatusCode, headers: &HeaderMap) -> Option<Self> {
        if !status.is_success() {
            return None;
        }
        let encoded = headers
            .get(CONTENT_ENCODING)
            .is_some_and(|v| !v.as_bytes().eq_ignore_ascii_case(b"identity"));
        if encoded {
            return None;
        }
        let kind = headers
            .get(CONTENT_TYPE)?
            .to_str()
            .ok()?
            .to_ascii_lowercase();
        let mode = if kind.starts_with("text/event-stream") {
            Mode::Sse
        } else if kind.starts_with("application/json") {
            Mode::Json
        } else {
            return None;
        };
        Some(UsageTap {
            mode,
            usage: Usage::default(),
            pending: Vec::new(),
            event: Event::Other,
            skipping: false,
        })
    }

    /// Reads one chunk of the response body.
    pub fn feed(&mut self, chunk: &[u8]) {
        match self.mode {
            Mode::Json => self.keep(chunk),
            Mode::Sse => {
                for piece in chunk.split_inclusive(|&b| b == b'\n') {
                    self.sse_piece(piece);
                }
            }
        }
    }

    /// Returns the usage read so far, parsing a JSON body that has ended.
    pub fn finish(mut self) -> Usage {
        if self.mode == Mode::Json
            && !self.skipping
            && let Ok(whole) = serde_json::from_slice::<Whole>(&self.pending)
        {
            if let Some(raw) = whole.usage {
                self.usage.apply(&raw);
            }
            if let Some(n) = whole.input_tokens {
                self.usage.input_tokens = n;
            }
        }
        self.usage
    }

    fn keep(&mut self, bytes: &[u8]) {
        if self.skipping {
            return;
        }
        if self.pending.len() + bytes.len() > KEEP_LIMIT {
            self.pending.clear();
            self.skipping = true;
        } else {
            self.pending.extend_from_slice(bytes);
        }
    }

    fn sse_piece(&mut self, piece: &[u8]) {
        let complete = piece.ends_with(b"\n");
        if !complete {
            self.keep(piece);
            return;
        }
        if self.skipping {
            self.skipping = false;
            return;
        }
        if self.pending.is_empty() {
            self.line(piece);
        } else {
            let mut line = std::mem::take(&mut self.pending);
            line.extend_from_slice(piece);
            self.line(&line);
            line.clear();
            self.pending = line;
        }
    }

    fn line(&mut self, line: &[u8]) {
        let line = line.trim_ascii_end();
        if line.is_empty() {
            self.event = Event::Other;
        } else if let Some(name) = line.strip_prefix(b"event:") {
            self.event = match name.trim_ascii() {
                b"message_start" => Event::Start,
                b"message_delta" => Event::Delta,
                _ => Event::Other,
            };
        } else if let Some(data) = line.strip_prefix(b"data:") {
            self.data(data.trim_ascii());
        }
    }

    fn data(&mut self, data: &[u8]) {
        let raw = match self.event {
            Event::Start => serde_json::from_slice::<Start>(data)
                .ok()
                .and_then(|s| s.message)
                .and_then(|m| m.usage),
            Event::Delta => serde_json::from_slice::<Delta>(data)
                .ok()
                .and_then(|d| d.usage),
            Event::Other => None,
        };
        if let Some(raw) = raw {
            self.usage.apply(&raw);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::header::HeaderValue;

    const STREAM: &str = "event: message_start\n\
data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":12,\"cache_creation_input_tokens\":3,\"cache_read_input_tokens\":5,\"output_tokens\":1}}}\n\n\
event: content_block_delta\n\
data: {\"type\":\"content_block_delta\",\"delta\":{\"text\":\"hi\"}}\n\n\
event: message_delta\r\n\
data: {\"type\":\"message_delta\",\"usage\":{\"output_tokens\":42}}\r\n\r\n";

    fn tap(kind: &'static str) -> UsageTap {
        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static(kind));
        UsageTap::for_response(StatusCode::OK, &headers).unwrap()
    }

    const EXPECTED: Usage = Usage {
        input_tokens: 12,
        cache_creation_tokens: 3,
        cache_read_tokens: 5,
        output_tokens: 42,
    };

    #[test]
    fn reads_start_and_delta_from_one_chunk() {
        let mut t = tap("text/event-stream");
        t.feed(STREAM.as_bytes());
        assert_eq!(t.finish(), EXPECTED);
    }

    #[test]
    fn reads_events_split_at_every_byte() {
        let mut t = tap("text/event-stream");
        for b in STREAM.as_bytes().chunks(1) {
            t.feed(b);
        }
        assert_eq!(t.finish(), EXPECTED);
    }

    #[test]
    fn reads_a_json_body() {
        let mut t = tap("application/json");
        t.feed(br#"{"usage":{"input_tokens":7,"output_tokens""#);
        t.feed(br#":9}}"#);
        let u = t.finish();
        assert_eq!((u.input_tokens, u.output_tokens), (7, 9));
    }

    #[test]
    fn reads_count_tokens_answers() {
        let mut t = tap("application/json");
        t.feed(br#"{"input_tokens":31}"#);
        assert_eq!(t.finish().input_tokens, 31);
    }

    #[test]
    fn skips_errors_and_compressed_bodies() {
        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        assert!(UsageTap::for_response(StatusCode::BAD_REQUEST, &headers).is_none());
        headers.insert(CONTENT_ENCODING, HeaderValue::from_static("gzip"));
        assert!(UsageTap::for_response(StatusCode::OK, &headers).is_none());
    }

    #[test]
    fn an_oversized_line_is_skipped_and_the_next_one_still_counts() {
        let mut t = tap("text/event-stream");
        t.feed(b"event: content_block_delta\ndata: ");
        t.feed(&vec![b'x'; KEEP_LIMIT + 1]);
        t.feed(b"\nevent: message_delta\ndata: {\"usage\":{\"output_tokens\":4}}\n");
        assert_eq!(t.finish().output_tokens, 4);
    }
}
