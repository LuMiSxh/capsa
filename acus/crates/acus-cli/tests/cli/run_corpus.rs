// SPDX-License-Identifier: MPL-2.0
//! One green and one failing capture per runner, through the real filter, so a new pattern or a
//! new tool cannot silently hide a failure.

use crate::common::*;

/// Runs `acus run --log` over a captured output in a temp dir with a real `src/lib.rs`, so the
/// `path:line` references resolve.
fn corpus(name: &str, extra: &[&str]) -> (i32, String, String) {
    let d = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(d.path().join("src")).unwrap();
    std::fs::write(d.path().join("src/lib.rs"), "fn f() {\n    boom()\n}\n").unwrap();
    let from = format!("{}/tests/fixtures/run/{name}", env!("CARGO_MANIFEST_DIR"));
    let log = d.path().join(name);
    std::fs::copy(from, &log).unwrap();
    let mut args = vec!["run", "--log", log.to_str().unwrap()];
    args.extend_from_slice(extra);
    acus_in(d.path(), &args, "")
}

#[test]
fn cargo_green_collapses_to_its_counts() {
    let (code, out, err) = corpus("cargo_green.log", &[]);
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("; 3 passed, 1 skipped"), "{out}");
    assert!(!out.contains("test tests::a"), "{out}");
    assert!(!out.contains("test result: ok"), "{out}");
}

#[test]
fn cargo_fail_keeps_the_failure_and_its_code() {
    let (_, out, _) = corpus("cargo_fail.log", &[]);
    assert!(out.contains("test tests::c ... FAILED"), "{out}");
    assert!(out.contains("test result: FAILED"), "{out}");
    assert!(out.contains("error: test failed"), "{out}");
    assert!(out.contains("; 2 passed, 1 failed"), "{out}");
    assert!(out.contains("== src/lib.rs#f"), "{out}");
    assert!(!out.contains("test tests::a ... ok"), "{out}");
}

#[test]
fn nextest_green_collapses_to_its_counts() {
    let (_, out, _) = corpus("nextest_green.log", &[]);
    assert!(
        out.contains("… 10 noise lines dropped; 4 passed, 2 skipped"),
        "{out}"
    );
    assert!(!out.contains("PASS ["), "{out}");
    assert!(!out.contains("Nextest run ID"), "{out}");
}

#[test]
fn nextest_fail_keeps_fail_and_its_output() {
    let (_, out, _) = corpus("nextest_fail.log", &[]);
    assert!(out.contains("FAIL [   0.019s] foo::bin tests::c"), "{out}");
    assert!(
        out.contains("--- STDERR:              foo::bin tests::c ---"),
        "{out}"
    );
    assert!(out.contains("error: test run failed"), "{out}");
    assert!(out.contains("; 1 passed, 1 failed"), "{out}");
    assert!(out.contains("== src/lib.rs#f"), "{out}");
    assert!(!out.contains("PASS ["), "{out}");
}

#[test]
fn xctest_fail_folds_the_nested_summaries() {
    let (_, out, _) = corpus("xctest_fail.log", &[]);
    assert!(out.contains("FooTests.swift:2: error"), "{out}");
    assert!(out.contains("Test Suite 'All tests' failed"), "{out}");
    assert_eq!(out.matches("Executed 3 tests").count(), 1, "{out}");
    assert!(out.contains("; 2 passed, 1 failed"), "{out}");
    assert!(!out.contains("testA]'"), "{out}");
    assert!(!out.contains("Build complete!"), "{out}");
    assert!(!out.contains("Building for debugging"), "{out}");
}

#[test]
fn swift_testing_fail_keeps_the_issue_only() {
    let (_, out, _) = corpus("swift_testing_fail.log", &[]);
    assert!(out.contains("✘ Test \"c\" recorded an issue"), "{out}");
    assert!(out.contains("✘ Test run with 3 tests failed"), "{out}");
    assert!(out.contains("; 2 passed, 1 failed"), "{out}");
    assert!(!out.contains("✔ Test \"a\" passed"), "{out}");
    assert!(!out.contains("Testing Library Version"), "{out}");
    assert!(!out.contains("◇"), "{out}");
}

#[test]
fn pytest_fail_keeps_the_failure_summary() {
    let (_, out, _) = corpus("pytest_fail.log", &[]);
    assert!(out.contains("FAILED tests/test_a.py::test_c"), "{out}");
    assert!(out.contains("1 failed, 2 passed in 0.12s"), "{out}");
    assert!(out.contains("; 2 passed, 1 failed"), "{out}");
    assert!(!out.contains("collected 3 items"), "{out}");
}

#[test]
fn dotnet_green_collapses_to_its_counts() {
    let (_, out, _) = corpus("dotnet_green.log", &[]);
    assert!(out.contains("; 2 passed, 1 skipped"), "{out}");
    assert!(!out.contains("Passed Test1"), "{out}");
    assert!(!out.contains("Test Run Successful"), "{out}");
}

#[test]
fn xctest_green_collapses_to_its_counts() {
    let (_, out, _) = corpus("xctest_green.log", &[]);
    assert!(out.contains("; 2 tests passed"), "{out}");
    assert!(!out.contains("Test Case"), "{out}");
    assert!(!out.contains("Build complete!"), "{out}");
}

#[test]
fn swift_testing_green_collapses_to_its_counts() {
    let (_, out, _) = corpus("swift_testing_green.log", &[]);
    assert!(out.contains("; 1 test passed"), "{out}");
    assert!(!out.contains('✔'), "{out}");
    assert!(!out.contains('◇'), "{out}");
    assert!(!out.contains("Testing Library Version"), "{out}");
}

#[test]
fn pytest_green_collapses_to_its_counts() {
    let (_, out, _) = corpus("pytest_green.log", &[]);
    assert!(out.contains("; 3 tests passed"), "{out}");
    assert!(!out.contains("[100%]"), "{out}");
    assert!(!out.contains("test session starts"), "{out}");
}

#[test]
fn ctest_green_collapses_to_its_counts() {
    let (_, out, _) = corpus("ctest_green.log", &[]);
    assert!(out.contains("; 3 tests passed"), "{out}");
    assert!(!out.contains("Test #"), "{out}");
}

#[test]
fn failure_lines_are_never_dropped() {
    let is_failure = |l: &str| {
        let t = l.trim_start();
        t.starts_with("error")
            || t.starts_with("FAIL ")
            || t.starts_with("FAILED ")
            || t.starts_with("--- FAIL")
            || t.starts_with('✘')
            || t.contains("FAILED")
            || t.contains(": error")
    };
    for name in [
        "cargo_fail.log",
        "nextest_fail.log",
        "xctest_fail.log",
        "swift_testing_fail.log",
        "pytest_fail.log",
    ] {
        let text = std::fs::read_to_string(format!(
            "{}/tests/fixtures/run/{name}",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap();
        let (_, out, _) = corpus(name, &[]);
        for line in text.lines().filter(|l| is_failure(l)) {
            assert!(
                out.contains(line.trim()),
                "{name}: dropped `{line}` from\n{out}"
            );
        }
    }
}

#[test]
fn explain_names_the_tool_and_the_kinds() {
    let (_, out, _) = corpus("nextest_green.log", &["--explain"]);
    assert!(out.contains("… detected: cargo nextest"), "{out}");
    assert!(out.contains("pass 4"), "{out}");
    assert!(out.contains("summary 1"), "{out}");
    assert!(out.contains("separator 2"), "{out}");
}

#[test]
fn other_runners_keep_failures_and_drop_passes() {
    struct Case {
        green: &'static str,
        green_gone: &'static [&'static str],
        green_footer: &'static str,
        fail: &'static str,
        fail_kept: &'static [&'static str],
        fail_footer: &'static str,
    }
    let cases = [
        Case {
            green: "go_green.log",
            green_gone: &["--- PASS", "=== RUN"],
            green_footer: "",
            fail: "go_fail.log",
            fail_kept: &["--- FAIL: TestB", "a_test.go:9: boom"],
            fail_footer: "",
        },
        Case {
            green: "vitest_green.log",
            green_gone: &["✓ tests/a.test.ts", "Tests  3 passed"],
            green_footer: "; 3 tests passed",
            fail: "vitest_fail.log",
            fail_kept: &["× adds two numbers", "expected 3 to be 4"],
            fail_footer: "",
        },
        Case {
            green: "bun_green.log",
            green_gone: &["(pass) adds", " 3 pass"],
            green_footer: "; 3 tests passed",
            fail: "bun_fail.log",
            fail_kept: &["(fail) divides", "error: expect"],
            fail_footer: "",
        },
        Case {
            green: "deno_green.log",
            green_gone: &["test adds ... ok", "ok | 2 passed"],
            green_footer: "; 2 tests passed",
            fail: "deno_fail.log",
            fail_kept: &["test divides ... FAILED", "error: AssertionError"],
            fail_footer: "; 1 passed, 1 failed",
        },
        Case {
            green: "rspec_green.log",
            green_gone: &["2 examples, 0 failures"],
            green_footer: "; 2 tests passed",
            fail: "rspec_fail.log",
            fail_kept: &["Failures:", "2 examples, 1 failure"],
            fail_footer: "; 1 passed, 1 failed",
        },
        Case {
            green: "phpunit_green.log",
            green_gone: &["OK (3 tests", "3 / 3 (100%)"],
            green_footer: "; 3 tests passed",
            fail: "phpunit_fail.log",
            fail_kept: &["FAILURES!", "Tests: 3, Assertions: 5, Failures: 1."],
            fail_footer: "",
        },
    ];
    for case in cases {
        let (code, out, err) = corpus(case.green, &[]);
        assert_eq!(code, 0, "{}: {err}", case.green);
        if !case.green_footer.is_empty() {
            assert!(out.contains(case.green_footer), "{}: {out}", case.green);
        }
        for gone in case.green_gone {
            assert!(!out.contains(gone), "{} kept `{gone}`: {out}", case.green);
        }
        let (_, out, _) = corpus(case.fail, &[]);
        if !case.fail_footer.is_empty() {
            assert!(out.contains(case.fail_footer), "{}: {out}", case.fail);
        }
        for kept in case.fail_kept {
            assert!(out.contains(kept), "{} dropped `{kept}`: {out}", case.fail);
        }
    }
}
