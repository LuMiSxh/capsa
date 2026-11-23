// SPDX-License-Identifier: MPL-2.0

use super::run::clamp_timeout;
use super::{Bindings, sleep};
use crate::config::Config;
use acus_script::Host;
use serde_json::{Value, json};
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

fn clamped(options: Value, left: Duration) -> Value {
    let Value::Object(mut options) = options else {
        panic!("options are an object");
    };
    clamp_timeout(&mut options, left);
    Value::Object(options)
}

#[test]
fn run_timeouts_are_cut_to_the_seconds_left_before_the_deadline() {
    let cases = [
        (json!({}), Duration::from_secs(500), json!({"timeout": 120})),
        (
            json!({}),
            Duration::from_millis(30_400),
            json!({"timeout": 31}),
        ),
        (
            json!({"timeout": 10}),
            Duration::from_secs(60),
            json!({"timeout": 10}),
        ),
        (
            json!({"timeout": 1000}),
            Duration::from_secs(60),
            json!({"timeout": 60}),
        ),
        (json!({}), Duration::ZERO, json!({"timeout": 1})),
        (
            json!({"timeout": "x"}),
            Duration::from_secs(5),
            json!({"timeout": "x"}),
        ),
    ];
    for (options, left, expected) in cases {
        assert_eq!(
            clamped(options.clone(), left),
            expected,
            "{options} {left:?}"
        );
    }
}

#[test]
fn an_identical_read_only_call_runs_once_but_counts_each_time() {
    let host = Bindings::new(Config::default()).unwrap();
    let args = vec![json!("memo_marker"), json!({"paths": ["src/main.rs"]})];
    let deadline = Instant::now() + Duration::from_secs(30);
    let first = host.call("find", args.clone(), deadline).unwrap();
    let second = host.call("find", args, deadline).unwrap();
    assert_eq!(first, second);
    assert_eq!(host.memo.ran(), 1);
    assert_eq!(host.calls().load(Ordering::Relaxed), 2);
}

#[test]
fn sleep_waits_its_milliseconds_and_stops_at_the_deadline() {
    let started = Instant::now();
    sleep(&[json!(40)], Instant::now() + Duration::from_secs(5)).unwrap();
    assert!(started.elapsed() >= Duration::from_millis(40));

    let started = Instant::now();
    sleep(&[json!(60_000)], Instant::now() + Duration::from_millis(50)).unwrap();
    assert!(started.elapsed() < Duration::from_secs(5));

    assert!(sleep(&[json!("soon")], Instant::now()).is_err());
    assert!(sleep(&[], Instant::now()).is_err());
}
