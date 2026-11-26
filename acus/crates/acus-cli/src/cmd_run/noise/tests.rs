// SPDX-License-Identifier: MPL-2.0
use super::patterns::Stats;
use super::{Profile, Summary, clean, keep, quiet, summary};
use regex::RegexSet;

fn run(out: &str) -> super::Quiet {
    let lines: Vec<&str> = out.lines().collect();
    quiet(&lines, &RegexSet::empty(), false)
}

fn shown(q: &super::Quiet) -> Vec<&str> {
    q.lines.iter().map(String::as_str).collect()
}

#[test]
fn drops_build_noise_but_keeps_failures() {
    let out = "   Compiling foo v0.1.0 (/x)\n    Finished `test` profile\n     Running unittests src/lib.rs\n\nrunning 2 tests\ntest a ... ok\ntest b ... FAILED\n\n\nfailures:\ntest result: FAILED. 1 passed; 1 failed\n\nrunning 0 tests\ntest result: ok. 0 passed; 0 failed\n[3/9] Compiling Foo Bar.swift\nerror: x\nRunning is a word";
    let q = run(out);
    assert_eq!((q.summary.passed, q.summary.failed), (1, 1));
    assert_eq!(
        shown(&q),
        [
            "test b ... FAILED",
            "",
            "failures:",
            "test result: FAILED. 1 passed; 1 failed",
            "",
            "error: x",
            "Running is a word"
        ]
    );
}

#[test]
fn drops_noise_of_other_toolchains() {
    let out = "\x1b[32m ✓ src/a.test.ts (3 tests)\x1b[0m\n \x1b[31m× src/b.test.ts > fails\x1b[0m\n=== RUN   TestX\n--- PASS: TestX (0.00s)\n--- FAIL: TestY (0.00s)\nok  \tpkg/a\t0.1s\nCompileSwift normal arm64 /x/A.swift\n    cd /x\n/x/A.swift:3:1: error: nope\nTest Case '-[T a]' passed (0.001 seconds).\nTest Case '-[T b]' failed (0.001 seconds).\n============================= test session starts ==============================\nplatform darwin -- Python 3.13.0\ncollected 4 items\n\ntests/test_a.py ..F.  [100%]\ntests/test_b.py ....  [100%]\nwarn: x\nwarn: x\nwarn: x\ncustom noise 1\n> app@1.0.0 build\nProgress: resolved 1, reused 1\rProgress: resolved 9, reused 9\ndone";
    let text = clean(out);
    let lines: Vec<&str> = text.lines().collect();
    let q = quiet(&lines, &RegexSet::new(["^custom noise"]).unwrap(), false);
    assert_eq!(
        shown(&q),
        [
            " × src/b.test.ts > fails",
            "--- FAIL: TestY (0.00s)",
            "/x/A.swift:3:1: error: nope",
            "Test Case '-[T b]' failed (0.001 seconds).",
            "",
            "tests/test_a.py ..F.  [100%]",
            "warn: x (×3)",
            "done"
        ]
    );
}

#[test]
fn drops_nextest_noise_and_counts_a_green_run() {
    let out = "────────────\n Nextest run ID 698fc30d-87af-47ed-84b3-908653f29c9e with nextest profile: default\n    Starting 2 tests across 1 binary (1 test skipped)\n        PASS [   0.018s] (  1/2) pkg::bin a::b\n        PASS [   0.019s] (  2/2) pkg::bin c::d\n────────────\n     Summary [   0.020s] 2 tests run: 2 passed, 1 skipped";
    let q = run(out);
    assert!(q.lines.is_empty(), "{:?}", q.lines);
    assert_eq!(
        q.summary,
        Summary {
            passed: 2,
            failed: 0,
            skipped: 1
        }
    );
    assert_eq!(q.profile, Some("cargo nextest"));
}

#[test]
fn keeps_nextest_failures_and_their_output() {
    let out = "────────────\n Nextest run ID x with nextest profile: default\n    Starting 2 tests across 1 binary\n        PASS [   0.018s] pkg::bin a::b\n        FAIL [   0.019s] pkg::bin c::d\n\n--- STDOUT:              pkg::bin c::d ---\nhello\n\n--- STDERR:              pkg::bin c::d ---\nthread 'c::d' panicked at src/lib.rs:10:5:\nboom\n────────────\n     Summary [   0.020s] 2 tests run: 1 passed, 1 failed, 0 skipped\n        FAIL [   0.019s] pkg::bin c::d\nerror: test run failed";
    let q = run(out);
    assert_eq!(
        (q.summary.passed, q.summary.failed, q.summary.skipped),
        (1, 1, 0)
    );
    assert_eq!(
        shown(&q),
        [
            "        FAIL [   0.019s] pkg::bin c::d",
            "",
            "--- STDOUT:              pkg::bin c::d ---",
            "hello",
            "",
            "--- STDERR:              pkg::bin c::d ---",
            "thread 'c::d' panicked at src/lib.rs:10:5:",
            "boom",
            "     Summary [   0.020s] 2 tests run: 1 passed, 1 failed, 0 skipped",
            "        FAIL [   0.019s] pkg::bin c::d",
            "error: test run failed"
        ]
    );
}

#[test]
fn counts_green_summaries_of_each_runner() {
    let cases = [
        (
            "test result: ok. 3 passed; 0 failed; 1 ignored; 0 measured",
            3,
            1,
        ),
        (
            "     Summary [   0.020s] 4 tests run: 4 passed, 2 skipped",
            4,
            2,
        ),
        (
            "Executed 5 tests, with 0 failures (0 unexpected) in 0.1 (0.1) seconds",
            5,
            0,
        ),
        ("✔ Test run with 6 tests passed after 0.1 seconds.", 6, 0),
        ("7 passed, 1 skipped in 0.12s", 7, 1),
        (" 8 pass", 8, 0),
        ("ok | 9 passed | 0 failed (10ms)", 9, 0),
        ("10 examples, 0 failures", 10, 0),
        ("OK (11 tests, 20 assertions)", 11, 0),
        ("100% tests passed, 0 tests failed out of 12", 12, 0),
        (
            "Passed!  - Failed:     0, Passed:    13, Skipped:     1, Total: 14",
            13,
            1,
        ),
        ("Tests  14 passed (14)", 14, 0),
        ("15 passing (10ms)", 15, 0),
        ("# pass 16", 16, 0),
    ];
    for (line, passed, skipped) in cases {
        let (s, _) = summary::green(line).unwrap_or_else(|| panic!("{line}"));
        assert_eq!((s.passed, s.skipped), (passed, skipped), "{line}");
    }
}

#[test]
fn keeps_failure_summaries() {
    for line in [
        "test result: FAILED. 2 passed; 1 failed",
        "     Summary [   0.020s] 2 tests run: 1 passed, 1 failed, 0 skipped",
        "Executed 3 tests, with 1 failure (0 unexpected) in 0.1 (0.1) seconds",
        "✘ Test run with 3 tests failed after 0.1 seconds with 1 issue.",
        "1 failed, 2 passed in 0.12s",
        "1 example, 1 failure",
        "50% tests passed, 1 tests failed out of 3",
        "Tests  2 passed, 1 failed (3)",
        "Passed!  - Failed:     1, Passed:     2, Skipped:     0, Total: 3",
    ] {
        assert!(summary::green(line).is_none(), "{line}");
    }
}

#[test]
fn red_ignores_a_green_summary() {
    for line in [
        "     Summary [   0.020s] 4 tests run: 4 passed, 2 skipped",
        "Executed 5 tests, with 0 failures (0 unexpected) in 0.1 (0.1) seconds",
        "2 examples, 0 failures",
        "100% tests passed, 0 tests failed out of 3",
        "Passed!  - Failed:     0, Passed:    13, Skipped:     1, Total: 14",
        "# fail 0",
    ] {
        assert!(summary::red(line).is_none(), "{line}");
    }
}

#[test]
fn counts_failure_summaries() {
    let cases = [
        (
            "test result: FAILED. 2 passed; 1 failed; 0 ignored",
            2,
            1,
            0,
        ),
        (
            "     Summary [   0.020s] 2 tests run: 1 passed, 1 failed, 1 skipped",
            1,
            1,
            1,
        ),
        (
            "Executed 3 tests, with 1 failure (0 unexpected) in 0.1 (0.1) seconds",
            2,
            1,
            0,
        ),
        (
            "✘ Test run with 3 tests failed after 0.1 seconds with 1 issue.",
            2,
            1,
            0,
        ),
        ("1 failed, 2 passed in 0.12s", 2, 1, 0),
        ("1 error, 2 passed in 0.12s", 2, 1, 0),
        ("FAILED | 1 passed | 1 failed (10ms)", 1, 1, 0),
        ("1 example, 1 failure", 0, 1, 0),
        ("50% tests passed, 1 tests failed out of 3", 2, 1, 0),
        (
            "Failed!  - Failed:     1, Passed:     2, Skipped:     0, Total: 3",
            2,
            1,
            0,
        ),
        ("# fail 1", 0, 1, 0),
    ];
    for (line, passed, failed, skipped) in cases {
        let (s, _) = summary::red(line).unwrap_or_else(|| panic!("{line}"));
        assert_eq!(
            (s.passed, s.failed, s.skipped),
            (passed, failed, skipped),
            "{line}"
        );
    }
}

#[test]
fn drops_noise_of_more_test_runners() {
    let out = "bun test v1.1.0\n(pass) adds [0.02ms]\n 3 pass\n 0 fail\n 2 expect() calls\nRan 3 tests across 1 file. [10.00ms]\nrunning 2 tests from ./a_test.ts\nok | 2 passed | 0 failed (10ms)\nFinished in 0.00234 seconds (files took 0.1 seconds to load)\n2 examples, 0 failures\nPHPUnit 11.0.0 by Sebastian Bergmann and contributors.\nRuntime:       PHP 8.3.0\nConfiguration: /x/phpunit.xml\nTime: 00:00.001, Memory: 6.00 MB\n...                                                                 3 / 3 (100%)\nOK (3 tests, 5 assertions)\nTest project /x/build\n    Start 1: test_a\n1/3 Test #1: test_a ...........................   Passed    0.01 sec\n100% tests passed, 0 tests failed out of 3\nTotal Test time (real) =   0.03 sec\nwarn: keep me\n 1 fail\n1 example, 1 failure\nFAILED | 1 passed | 1 failed (10ms)\n3/3 Test #3: test_c ...........................***Failed    0.01 sec\n50% tests passed, 1 tests failed out of 3";
    let q = run(out);
    assert_eq!(
        shown(&q),
        [
            "warn: keep me",
            " 1 fail",
            "1 example, 1 failure",
            "FAILED | 1 passed | 1 failed (10ms)",
            "3/3 Test #3: test_c ...........................***Failed    0.01 sec",
            "50% tests passed, 1 tests failed out of 3"
        ]
    );
}

#[test]
fn folds_a_run_of_status_lines_of_an_unknown_runner() {
    let out = "        SUCCESS [   0.018s] pkg a::b\n        SUCCESS [   0.019s] pkg c::d\n        SUCCESS [   0.020s] pkg e::f\n        SUCCESS [   0.021s] pkg g::h\nerror: boom";
    let q = run(out);
    assert_eq!(
        shown(&q),
        [
            "        SUCCESS [   0.018s] pkg a::b",
            "… 2 similar lines",
            "        SUCCESS [   0.021s] pkg g::h",
            "error: boom"
        ]
    );
}

#[test]
fn keeps_diagnostic_and_free_form_runs() {
    let out = "FAIL [   0.018s] a\nFAIL [   0.019s] b\nFAIL [   0.020s] c\nFAIL [   0.021s] d\n?? x\n?? y\n?? z\n?? w\n?? v";
    assert_eq!(run(out).lines.len(), 9);
}

#[test]
fn folds_nested_xctest_suite_summaries() {
    let out = "Test Suite 'FooTests' failed at 2026-11-26 10:00:00.003\n     Executed 3 tests, with 1 failure (0 unexpected) in 0.003 (0.003) seconds\nTest Suite 'FooTests.xctest' failed at 2026-11-26 10:00:00.004\n     Executed 3 tests, with 1 failure (0 unexpected) in 0.003 (0.003) seconds\nTest Suite 'All tests' failed at 2026-11-26 10:00:00.005\n     Executed 3 tests, with 1 failure (0 unexpected) in 0.003 (0.003) seconds\nerror: test failed";
    let q = run(out);
    assert_eq!(
        shown(&q),
        [
            "Test Suite 'All tests' failed at 2026-11-26 10:00:00.005",
            "     Executed 3 tests, with 1 failure (0 unexpected) in 0.003 (0.003) seconds",
            "error: test failed"
        ]
    );
}

#[test]
fn detects_the_runner() {
    let cases = [
        (
            "Nextest run ID x with nextest profile: default",
            Profile::Nextest,
        ),
        ("        PASS [   0.018s] pkg a::b", Profile::Nextest),
        ("Test Suite 'FooTests' started at x", Profile::Xctest),
        ("◇ Test run started.", Profile::SwiftTesting),
        ("bun test v1.1.0", Profile::Bun),
        ("running 2 tests from ./a_test.ts", Profile::Deno),
        ("PHPUnit 11.0.0 by Sebastian Bergmann", Profile::PhpUnit),
        ("Test project /x/build", Profile::CTest),
        ("Passed!  - Failed:     0, Passed: 2", Profile::Dotnet),
        ("=== RUN   TestX", Profile::GoTest),
        ("===== test session starts =====", Profile::Pytest),
        ("2 examples, 0 failures", Profile::RSpec),
        ("Tests  3 passed (3)", Profile::Vitest),
        ("running 2 tests", Profile::Cargo),
    ];
    for (line, expected) in cases {
        assert_eq!(Profile::detect(&[line]), Some(expected), "{line}");
    }
}

#[test]
fn explain_counts_each_kind() {
    let out = "────────────\n Nextest run ID x with nextest profile: default\n    Starting 2 tests across 1 binary\n        PASS [   0.018s] pkg a\n        PASS [   0.019s] pkg b\n────────────\n     Summary [   0.020s] 2 tests run: 2 passed, 0 skipped";
    let lines: Vec<&str> = out.lines().collect();
    let q = quiet(&lines, &RegexSet::empty(), true);
    let kind = q.stats.render();
    assert!(kind.contains("pass 2"), "{kind}");
    assert!(kind.contains("summary 1"), "{kind}");
    assert!(kind.contains("separator 2"), "{kind}");
    assert!(Stats::new(false).is_empty());
}

#[test]
fn budget_keeps_failures_and_the_tail() {
    let mut lines: Vec<String> = (0..30).map(|i| format!("line {i}")).collect();
    lines.insert(10, "error: boom".into());
    lines.insert(11, String::new());
    let (kept, dropped) = keep(lines, 5, 40);
    assert!(kept.iter().any(|l| l == "error: boom"), "{kept:?}");
    assert_eq!(kept.last().map(String::as_str), Some("line 29"));
    assert!(
        kept.iter()
            .any(|l| l.contains("lines dropped (--max-output 5)")),
        "{kept:?}"
    );
    assert!(dropped > 0);
}

#[test]
fn drops_library_stack_frames() {
    let out = "thread 'main' panicked at src/main.rs:3:5:\nboom\nstack backtrace:\n   0: rust_begin_unwind\n             at /rustc/abc/library/std/src/panicking.rs:665:5\n   1: app::run\n             at ./src/main.rs:3:5\nnote: Some details are omitted, run with `RUST_BACKTRACE=full` for a verbose backtrace.\nTraceback (most recent call last):\n  File \"/x/app.py\", line 3, in <module>\n    go()\n  File \"/usr/lib/python3.12/site-packages/lib.py\", line 9, in go\n    raise X\n    ^^^^^^^\nX: bad\n    at run (src/a.ts:4:9)\n    at Module._compile (node:internal/modules/cjs/loader:1105:14)";
    let q = run(out);
    assert_eq!(
        shown(&q),
        [
            "thread 'main' panicked at src/main.rs:3:5:",
            "boom",
            "stack backtrace:",
            "   1: app::run",
            "             at ./src/main.rs:3:5",
            "Traceback (most recent call last):",
            "  File \"/x/app.py\", line 3, in <module>",
            "    go()",
            "X: bad",
            "    at run (src/a.ts:4:9)"
        ]
    );
}

#[test]
fn strips_github_actions_prefixes() {
    assert_eq!(clean("job\tstep\t2026-01-05T10:00:00Z hello"), "hello");
}
