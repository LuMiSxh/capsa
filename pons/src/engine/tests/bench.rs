// SPDX-License-Identifier: MPL-2.0

use super::*;
use std::hint::black_box;

const LINES: u32 = 100_000;

fn time_lines(name: &str, lines: &[Vec<u8>], client: bool) {
    let mut e = engine();
    let now = Instant::now();
    let start = Instant::now();
    let mut actions = 0;
    for raw in lines
        .iter()
        .cycle()
        .take(usize::try_from(LINES).unwrap_or(0))
    {
        let out = if client {
            e.on_client(black_box(raw), now)
        } else {
            e.on_agent(black_box(raw), now)
        };
        actions += black_box(out).len();
    }
    let per_line = start.elapsed().as_nanos() / u128::from(LINES);
    println!("{name}: {per_line} ns/line over {LINES} lines ({actions} actions)");
}

/// Timing harness, not a check: run with
/// `cargo test --profile release-unwind -p pons -- --ignored --nocapture engine_line_cost`.
#[test]
#[ignore = "timing harness, run in release-unwind with --nocapture"]
fn engine_line_cost() {
    let client = vec![prompt(2)];
    let agent = vec![
        chunk("Reading the file now."),
        update(json!({"sessionUpdate": "tool_call", "toolCallId": "t1",
            "title": "Read src/lib.rs", "kind": "read", "status": "pending"})),
        update(
            json!({"sessionUpdate": "tool_call_update", "toolCallId": "t1",
            "status": "completed"}),
        ),
    ];
    time_lines("client prompt", &client, true);
    time_lines("agent session/update mix", &agent, false);
}
