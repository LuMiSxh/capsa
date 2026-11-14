// SPDX-License-Identifier: MPL-2.0
//! Reads one transcript line into a ledger row.

use super::model::Row;
use crate::transcript::Record;
use crate::usage::parse_ts;

/// Returns the request an assistant record carries, or `None` for any other line.
pub(super) fn parse_line(line: &str) -> Option<Row> {
    if !line.contains("\"usage\"") {
        return None;
    }
    let rec: Record<'_> = serde_json::from_str(line).ok()?;
    let req = rec.request()?;
    let id = req.id?;
    let t = parse_ts(rec.timestamp?)? / 1000;
    let agent = rec.agent_id;
    let tokens = req.tokens;
    Some(Row {
        id,
        t,
        session: rec.session_id.unwrap_or_default(),
        role: agent.as_ref().and(rec.attribution_agent),
        agent,
        model: req.model,
        input: tokens.input,
        cache_read: tokens.cache_read,
        cache_write_5m: tokens.cache_write_5m,
        cache_write_1h: tokens.cache_write_1h,
        output: tokens.output,
        gap_s: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

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
