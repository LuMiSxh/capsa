// SPDX-License-Identifier: MPL-2.0
//! Reads one transcript line into a ledger row.

use super::model::Row;
use serde::Deserialize;

#[derive(Deserialize)]
struct Rec<'a> {
    #[serde(rename = "type")]
    kind: Option<&'a str>,
    timestamp: Option<&'a str>,
    #[serde(rename = "sessionId")]
    session: Option<String>,
    #[serde(rename = "agentId")]
    agent: Option<String>,
    #[serde(rename = "attributionAgent")]
    role: Option<String>,
    message: Option<Msg>,
}

#[derive(Deserialize)]
struct Msg {
    id: Option<String>,
    model: Option<String>,
    usage: Option<Usage>,
}

#[derive(Deserialize)]
struct Usage {
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    cache_read_input_tokens: u64,
    #[serde(default)]
    cache_creation_input_tokens: u64,
    cache_creation: Option<CacheCreation>,
    #[serde(default)]
    output_tokens: u64,
}

#[derive(Deserialize)]
struct CacheCreation {
    #[serde(default)]
    ephemeral_5m_input_tokens: u64,
    #[serde(default)]
    ephemeral_1h_input_tokens: u64,
}

/// Returns the request an assistant record carries, or `None` for any other line.
pub(super) fn parse_line(line: &str) -> Option<Row> {
    if !line.contains("\"usage\"") {
        return None;
    }
    let rec: Rec = serde_json::from_str(line).ok()?;
    if rec.kind != Some("assistant") {
        return None;
    }
    let msg = rec.message?;
    let (usage, id, model) = (msg.usage?, msg.id?, msg.model.unwrap_or_default());
    if model == "<synthetic>" {
        return None;
    }
    let (w5, w1h) = match usage.cache_creation {
        Some(c) => (c.ephemeral_5m_input_tokens, c.ephemeral_1h_input_tokens),
        None => (usage.cache_creation_input_tokens, 0),
    };
    let agent = rec.agent;
    Some(Row {
        id,
        t: parse_timestamp(rec.timestamp?)?,
        session: rec.session.unwrap_or_default(),
        role: agent.as_ref().and(rec.role),
        agent,
        model,
        input: usage.input_tokens,
        cache_read: usage.cache_read_input_tokens,
        cache_write_5m: w5,
        cache_write_1h: w1h,
        output: usage.output_tokens,
        gap_s: None,
    })
}

/// Converts `YYYY-MM-DDTHH:MM:SS[.fff]Z` to unix seconds.
pub(super) fn parse_timestamp(s: &str) -> Option<u64> {
    if s.len() < 19 || s.as_bytes()[10] != b'T' {
        return None;
    }
    let num = |from: usize, len: usize| s.get(from..from + len)?.parse::<i64>().ok();
    let (y, m, d) = (num(0, 4)?, num(5, 2)?, num(8, 2)?);
    let (h, mi, sec) = (num(11, 2)?, num(14, 2)?, num(17, 2)?);
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    u64::try_from(days * 86_400 + h * 3600 + mi * 60 + sec).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamp_matches_known_unix_times() {
        assert_eq!(parse_timestamp("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(
            parse_timestamp("2026-10-21T11:55:40.658Z"),
            Some(1_792_583_740)
        );
        assert_eq!(parse_timestamp("2024-02-29T23:59:59Z"), Some(1_709_251_199));
        assert_eq!(parse_timestamp("garbage"), None);
    }

    #[test]
    fn assistant_record_gives_a_row_with_cache_split() {
        let line = r#"{"type":"assistant","timestamp":"2026-10-21T11:55:40Z","sessionId":"s","agentId":"a1","attributionAgent":"Explore","message":{"id":"m","model":"claude-haiku-5-5","usage":{"input_tokens":1,"cache_creation_input_tokens":7,"cache_creation":{"ephemeral_5m_input_tokens":3,"ephemeral_1h_input_tokens":4},"cache_read_input_tokens":5,"output_tokens":2}}}"#;
        let r = parse_line(line).unwrap();
        assert_eq!(
            (r.cache_write_5m, r.cache_write_1h, r.cache_read),
            (3, 4, 5)
        );
        assert_eq!(
            (r.agent.as_deref(), r.role.as_deref()),
            (Some("a1"), Some("Explore"))
        );
    }

    #[test]
    fn role_is_dropped_for_the_main_agent() {
        let line = r#"{"type":"assistant","timestamp":"2026-10-21T11:55:40Z","attributionAgent":"x","message":{"id":"m","model":"m","usage":{}}}"#;
        assert_eq!(parse_line(line).unwrap().role, None);
    }
}
