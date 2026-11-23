// SPDX-License-Identifier: MPL-2.0
use crate::{Host, HostError, Limits, ScriptError, check_module, run};
use serde_json::{Value, json};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

struct Fake;

impl Host for Fake {
    fn names(&self) -> &[&str] {
        &["rows", "fail"]
    }

    fn call(&self, name: &str, args: Vec<Value>, _: Instant) -> Result<Value, HostError> {
        match name {
            "rows" => Ok(json!({"kind": "hits", "items": [
                {"path": "a.rs", "text": "alpha"},
                {"path": "a.rs", "text": "beta"},
                {"path": "b.rs", "text": "alpha two"},
            ], "echo": args})),
            _ => Err(HostError::new("no such file").with_hint("run acus files")),
        }
    }
}

fn limits() -> Limits {
    Limits {
        deadline: Duration::from_secs(1),
        memory: 32 << 20,
        ..Limits::default()
    }
}

#[test]
fn prints_come_before_the_returned_value_and_keep_their_kind() {
    let out = run(
        "print('x', 2); const r = (await rows(1)).grep('alpha'); print(r.paths()); return r.head(1);",
        Fake,
        &limits(),
    )
    .unwrap();
    assert_eq!(out.prints, [json!("x"), json!(2), json!(["a.rs", "b.rs"])]);
    assert_eq!(
        out.value,
        Some(json!({"kind": "hits", "items": [{"path": "a.rs", "text": "alpha"}]}))
    );
}

#[test]
fn a_host_error_is_catchable_and_carries_its_hint() {
    let out = run(
        "try { await fail(); } catch (e) { return e.message + '|' + e.hint; }",
        Fake,
        &limits(),
    )
    .unwrap();
    assert_eq!(out.value, Some(json!("no such file|run acus files")));
}

#[test]
fn an_uncaught_host_error_names_the_program_line_and_the_hint() {
    let err = run("const a = 1;\nawait fail();", Fake, &limits()).unwrap_err();
    let ScriptError::Uncaught {
        line,
        message,
        hint,
        ..
    } = err
    else {
        panic!("expected an uncaught error");
    };
    assert_eq!(line, Some(2));
    assert_eq!(message, "no such file");
    assert_eq!(hint.as_deref(), Some("run acus files"));
}

#[test]
fn prints_before_an_uncaught_throw_come_back_with_the_error() {
    let err = run(
        "print('before');\nthrow new Error('boom');",
        Fake,
        &limits(),
    )
    .unwrap_err();
    let ScriptError::Uncaught { partial, .. } = err else {
        panic!("expected an uncaught error");
    };
    assert_eq!(partial.prints, [json!("before")]);
}

#[test]
fn prints_before_the_deadline_come_back_with_the_timeout() {
    let quick = Limits {
        deadline: Duration::from_millis(200),
        ..limits()
    };
    let err = run("print('before');\nwhile (true) {}", Fake, &quick).unwrap_err();
    let ScriptError::Timeout { partial, .. } = err else {
        panic!("expected a timeout");
    };
    assert_eq!(partial.prints, [json!("before")]);
}

#[test]
fn a_call_nobody_awaited_comes_back_when_the_program_throws() {
    let err = run("rows(1);\nthrow new Error('boom');", Fake, &limits()).unwrap_err();
    let ScriptError::Uncaught { partial, .. } = err else {
        panic!("expected an uncaught error");
    };
    assert_eq!(partial.completed.len(), 1);
    assert_eq!(partial.completed[0]["kind"], "hits");
}

#[test]
fn failures_map_to_their_own_variants() {
    type Check = fn(&ScriptError) -> bool;
    let cases: [(&str, Check); 5] = [
        (
            "function f() { f(); } f();",
            |e| matches!(e, ScriptError::Uncaught { message, .. } if message.contains("stack")),
        ),
        ("while (true) {}", |e| {
            matches!(e, ScriptError::Timeout { secs: 1, .. })
        }),
        (
            "const a = []; while (true) a.push('x'.repeat(1 << 20));",
            |e| matches!(e, ScriptError::Memory { mib: 32, .. }),
        ),
        (
            "const a = 1;\nconst b = ;",
            |e| matches!(e, ScriptError::Uncaught { line: Some(2), message, .. } if message.starts_with("SyntaxError")),
        ),
        ("await new Promise(() => {});", |e| {
            matches!(e, ScriptError::Engine(_))
        }),
    ];
    for (code, expected) in cases {
        let err = run(code, Fake, &limits()).unwrap_err();
        assert!(expected(&err), "{code}: {err:?}");
    }
}

#[test]
fn checking_a_module_stops_at_the_deadline_and_has_no_host_functions() {
    let quick = Limits {
        deadline: Duration::from_millis(200),
        ..limits()
    };
    let err = check_module("while (true) {}", Fake, &quick).unwrap_err();
    assert!(matches!(err, ScriptError::Timeout { .. }), "{err:?}");
    let module = "await rows(1);\nexport const description = 'd';\nexport const params = {};\nexport default async function () {}";
    let err = check_module(module, Fake, &limits()).unwrap_err();
    assert!(
        matches!(&err, ScriptError::Uncaught { message, .. } if message.contains("rows")),
        "{err:?}"
    );
}

#[test]
fn nothing_but_the_host_functions_reaches_outside() {
    let out = run(
        "return [typeof require, typeof std, typeof os, typeof fetch, typeof process].join()",
        Fake,
        &limits(),
    )
    .unwrap();
    assert_eq!(
        out.value,
        Some(json!("undefined,undefined,undefined,undefined,undefined"))
    );
}

/// Sleeps in `nap`, records what ran in `log`, and treats `write` as a call that must run alone.
struct Slow {
    log: Arc<Mutex<Vec<String>>>,
    live: AtomicUsize,
    peak: Arc<AtomicUsize>,
}

impl Slow {
    fn note(&self, entry: &str) {
        self.log.lock().unwrap().push(entry.to_owned());
    }
}

impl Host for Slow {
    fn names(&self) -> &[&str] {
        &["nap", "write", "read", "hold", "boom", "fail"]
    }

    fn exclusive(&self, name: &str) -> bool {
        name == "write"
    }

    fn call(&self, name: &str, args: Vec<Value>, deadline: Instant) -> Result<Value, HostError> {
        let ms = args.first().and_then(Value::as_u64).unwrap_or(0);
        match name {
            "nap" => {
                let live = self.live.fetch_add(1, Ordering::SeqCst) + 1;
                self.peak.fetch_max(live, Ordering::SeqCst);
                std::thread::sleep(Duration::from_millis(ms));
                self.live.fetch_sub(1, Ordering::SeqCst);
                self.note("nap:end");
            }
            "write" => {
                self.note("write:start");
                std::thread::sleep(Duration::from_millis(50));
                self.note("write:end");
            }
            "read" => self.note("read"),
            "hold" => {
                // Ends just past the deadline, inside the expiry grace, as a clamped `run()` does.
                let end = deadline + Duration::from_millis(100);
                std::thread::sleep(end.saturating_duration_since(Instant::now()));
                self.note("hold:end");
            }
            "boom" => panic!("boom"),
            _ => return Err(HostError::new("nap refused")),
        }
        Ok(json!(ms))
    }
}

fn slow() -> (Slow, Arc<Mutex<Vec<String>>>) {
    let (host, log, _) = slow_with_peak();
    (host, log)
}

/// Also returns the most `nap` calls that ever ran at the same time.
fn slow_with_peak() -> (Slow, Arc<Mutex<Vec<String>>>, Arc<AtomicUsize>) {
    let log = Arc::new(Mutex::new(Vec::new()));
    let peak = Arc::new(AtomicUsize::new(0));
    (
        Slow {
            log: Arc::clone(&log),
            live: AtomicUsize::new(0),
            peak: Arc::clone(&peak),
        },
        log,
        peak,
    )
}

#[test]
fn calls_awaited_together_run_at_the_same_time() {
    let (host, _, peak) = slow_with_peak();
    let patient = Limits {
        deadline: Duration::from_secs(20),
        ..limits()
    };
    let out = run(
        "return await Promise.all([nap(200), nap(200), nap(200), nap(200)]);",
        host,
        &patient,
    )
    .unwrap();
    assert_eq!(out.value, Some(json!([200, 200, 200, 200])));
    assert!(peak.load(Ordering::SeqCst) >= 2);
}

#[test]
fn a_write_waits_for_earlier_calls_and_holds_back_later_ones() {
    let (host, log) = slow();
    run(
        "const a = nap(150); const w = write(); const r = read(); await Promise.all([a, w, r]);",
        host,
        &limits(),
    )
    .unwrap();
    assert_eq!(
        *log.lock().unwrap(),
        ["nap:end", "write:start", "write:end", "read"]
    );
}

#[test]
fn a_call_made_after_an_await_runs_after_it() {
    let (host, log) = slow();
    let out = run(
        "const a = await nap(10); const w = await write(); return a + (await nap(20));",
        host,
        &limits(),
    )
    .unwrap();
    assert_eq!(out.value, Some(json!(30)));
    assert_eq!(
        *log.lock().unwrap(),
        ["nap:end", "write:start", "write:end", "nap:end"]
    );
}

#[test]
fn a_call_the_program_never_awaits_still_finishes() {
    let (host, log) = slow();
    run("nap(100); return 1;", host, &limits()).unwrap();
    assert_eq!(*log.lock().unwrap(), ["nap:end"]);
}

#[test]
fn a_deadline_with_calls_in_flight_times_out_without_waiting_for_them() {
    let (host, _) = slow();
    let quick = Limits {
        deadline: Duration::from_millis(200),
        ..limits()
    };
    let started = Instant::now();
    let err = run("await Promise.all([nap(4000), nap(4000)]);", host, &quick).unwrap_err();
    assert!(matches!(err, ScriptError::Timeout { .. }), "{err:?}");
    // The deadline plus the 2 s grace, well short of the calls' 4 s.
    assert!(
        started.elapsed() < Duration::from_millis(3200),
        "{:?}",
        started.elapsed()
    );
}

#[test]
fn an_error_inside_promise_all_rejects_it_with_the_host_message_and_line() {
    let (host, _) = slow();
    let err = run(
        "const a = 1;\nawait Promise.all([nap(10), fail()]);",
        host,
        &limits(),
    )
    .unwrap_err();
    let ScriptError::Uncaught { line, message, .. } = err else {
        panic!("expected an uncaught error");
    };
    assert_eq!((line, message.as_str()), (Some(2), "nap refused"));
}

#[test]
fn an_unawaited_call_past_the_deadline_times_out_and_starts_no_queued_call() {
    let (host, log) = slow();
    let quick = Limits {
        deadline: Duration::from_millis(200),
        ..limits()
    };
    let started = Instant::now();
    let err = run("nap(4000); write(); return 1;", host, &quick).unwrap_err();
    assert!(matches!(err, ScriptError::Timeout { .. }), "{err:?}");
    assert!(
        started.elapsed() < Duration::from_millis(3200),
        "{:?}",
        started.elapsed()
    );
    assert!(log.lock().unwrap().is_empty(), "{:?}", log.lock().unwrap());
}

#[test]
fn a_call_that_ends_at_the_deadline_finishes_before_the_run_returns() {
    let (host, log) = slow();
    let quick = Limits {
        deadline: Duration::from_millis(200),
        ..limits()
    };
    let started = Instant::now();
    let err = run("await hold();", host, &quick).unwrap_err();
    assert!(matches!(err, ScriptError::Timeout { .. }), "{err:?}");
    assert_eq!(*log.lock().unwrap(), ["hold:end"]);
    assert!(
        started.elapsed() < Duration::from_millis(1500),
        "{:?}",
        started.elapsed()
    );
}

#[test]
fn a_panicking_host_call_becomes_a_script_error() {
    let (host, _) = slow();
    let out = run(
        "try { await boom(); } catch (e) { return e.message; }",
        host,
        &limits(),
    )
    .unwrap();
    assert_eq!(out.value, Some(json!("boom() panicked")));
}

#[test]
fn a_host_error_carries_the_function_name_and_its_arguments() {
    let out = run(
        "try { await fail(7, 'x'); } catch (e) { return [e.call, e.args]; }",
        Fake,
        &limits(),
    )
    .unwrap();
    assert_eq!(out.value, Some(json!(["fail", [7, "x"]])));
}

#[test]
fn all_keeps_the_order_and_turns_a_rejected_entry_into_a_failure() {
    let out = run(
        "const r = await all([rows(1), fail(7)]); return [r[0].kind, r[1]];",
        Fake,
        &limits(),
    )
    .unwrap();
    assert_eq!(
        out.value,
        Some(json!([
            "hits",
            {"ok": false, "error": "no such file", "hint": "run acus files", "call": "fail"}
        ]))
    );
}

/// Fails its first `fail_for` calls with "busy", then answers "done"; `sleep` naps as the CLI does.
struct Flaky {
    calls: Arc<AtomicUsize>,
    fail_for: usize,
}

impl Host for Flaky {
    fn names(&self) -> &[&str] {
        &["flaky", "sleep"]
    }

    fn call(&self, name: &str, args: Vec<Value>, _: Instant) -> Result<Value, HostError> {
        if name == "sleep" {
            let ms = args.first().and_then(Value::as_u64).unwrap_or(0);
            std::thread::sleep(Duration::from_millis(ms));
            return Ok(Value::Null);
        }
        let call = self.calls.fetch_add(1, Ordering::SeqCst);
        if call < self.fail_for {
            Err(HostError::new("busy"))
        } else {
            Ok(json!("done"))
        }
    }
}

#[test]
fn retry_succeeds_on_the_second_try() {
    let calls = Arc::new(AtomicUsize::new(0));
    let host = Flaky {
        calls: Arc::clone(&calls),
        fail_for: 1,
    };
    let out = run(
        "return await retry(() => flaky(), {delayMs: 5});",
        host,
        &limits(),
    )
    .unwrap();
    assert_eq!(out.value, Some(json!("done")));
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

#[test]
fn retry_stops_after_times_attempts_and_throws_the_last_error() {
    let calls = Arc::new(AtomicUsize::new(0));
    let host = Flaky {
        calls: Arc::clone(&calls),
        fail_for: 9,
    };
    let code = "try { return await retry(() => flaky(), {times: 2, delayMs: 1}); } catch (e) { return e.message; }";
    let out = run(code, host, &limits()).unwrap();
    assert_eq!(out.value, Some(json!("busy")));
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

#[test]
fn the_last_expression_is_the_result_when_the_program_neither_returns_nor_prints() {
    let cases = [
        ("1 + 1", Some(json!(2))),
        ("const a = 2;\na * 3;", Some(json!(6))),
        ("const a = 2;\na * 4 // done", Some(json!(8))),
        ("const a = 2;\na * 5\n// done\n", Some(json!(10))),
        (
            "const h = [1, 2, 3];\nh\n  .map((x) => x * 2)\n  .filter((x) => x > 2)",
            Some(json!([4, 6])),
        ),
        ("const a = 1;\nconst b = a + 1", None),
        ("const xs = [1, 2].map((x) => {\n  x + 1\n})", None),
        ("const a = 1;\nfunction f() {\n  a + 1\n}", None),
        ("const a = [1];\nif (a) {\n  a.length;\n}", None),
        ("const a = [1];\n{\n  a.length\n}", None),
    ];
    for (code, expected) in cases {
        let out = run(code, Fake, &limits()).unwrap();
        assert_eq!(out.value, expected, "{code}");
    }
}

#[test]
fn a_brace_less_body_or_label_is_not_the_result() {
    let cases = [
        (
            "let i = 0;\nfor (const x of [1, 2, 3])\n  print(x);",
            json!([1, 2, 3]),
        ),
        ("let i = 0;\nwhile (i < 3)\n  print(i++)", json!([0, 1, 2])),
        (
            "let i = 0;\ndo\n  print(i++)\nwhile (i < 3)",
            json!([0, 1, 2]),
        ),
        ("let n = 0;\nif (n === 0)\n  n = 5", json!([])),
        ("let n = 0;\nif (n === 1) n = 1;\nelse\n  n = 7", json!([])),
        ("let n = 0;\nskip:\n  n + 2", json!([])),
    ];
    for (code, prints) in cases {
        let out = run(code, Fake, &limits()).unwrap();
        assert_eq!(out.value, None, "{code}");
        assert_eq!(Value::Array(out.prints), prints, "{code}");
    }
}

#[test]
fn a_line_break_that_continues_an_expression_keeps_it_whole() {
    let cases = [
        ("let x = 1;\nx = 2\n+ 5", Some(json!(7))),
        ("let a = 1;\nlet b = 2;\na\n++b", Some(json!(3))),
    ];
    for (code, expected) in cases {
        let out = run(code, Fake, &limits()).unwrap();
        assert_eq!(out.value, expected, "{code}");
    }
}

#[test]
fn the_last_expression_after_a_brace_less_body_is_the_result() {
    let cases = [
        ("let n = 0;\nif (n === 0)\n  n = 5;\nn + 1", Some(json!(6))),
        (
            "let n = 0;\nfor (const x of [1, 2])\n  n += x\nn * 10",
            Some(json!(30)),
        ),
    ];
    for (code, expected) in cases {
        let out = run(code, Fake, &limits()).unwrap();
        assert_eq!(out.value, expected, "{code}");
    }
}

#[test]
fn a_program_that_prints_or_returns_keeps_its_own_result() {
    let out = run("print('a'); 2", Fake, &limits()).unwrap();
    assert_eq!(out.prints, [json!("a")]);
    assert_eq!(out.value, None);

    let out = run("const a = 1;\nreturn a + 1;\n", Fake, &limits()).unwrap();
    assert_eq!(out.value, Some(json!(2)));
}
