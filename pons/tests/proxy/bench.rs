// SPDX-License-Identifier: MPL-2.0

use std::time::Instant;

use super::harness::{Harness, temp_path};

const LINES: u32 = 20_000;

/// Timing harness, not a check: run with
/// `cargo test --profile release-unwind -p pons --test proxy -- --ignored --nocapture proxy_line_cost`.
/// It sends one prompt to the mock agent, which answers with `LINES` small updates, and times
/// how long the lines take to come out of the proxy, with and without the wire log.
#[tokio::test]
#[ignore = "timing harness, run in release-unwind with --nocapture"]
async fn proxy_line_cost() {
    let log = temp_path("bench.log");
    let log_arg = log.to_string_lossy().into_owned();
    for (name, args) in [
        ("log off", vec![]),
        ("log default", vec!["--log", log_arg.as_str()]),
    ] {
        let _ = std::fs::remove_file(&log);
        let mut h = Harness::start(&args);
        h.prompt(1, &format!("burst:{LINES}")).await;
        let start = Instant::now();
        let mut seen: u32 = 0;
        loop {
            seen += 1;
            if h.recv_line().await.contains("\"id\":1") {
                break;
            }
        }
        let per_line = start.elapsed().as_nanos() / u128::from(seen);
        println!("proxy, {name}: {per_line} ns/line over {seen} lines");
    }
    let _ = std::fs::remove_file(&log);
}
