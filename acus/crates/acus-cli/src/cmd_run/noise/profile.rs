// SPDX-License-Identifier: MPL-2.0
//! Detects the test runner from its header and its per-test lines, so pass and skip lines are
//! dropped and counted even when a new runner spells them differently.

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(in crate::cmd_run) enum Profile {
    Cargo,
    Nextest,
    Xctest,
    SwiftTesting,
    Pytest,
    GoTest,
    Vitest,
    Bun,
    Deno,
    RSpec,
    PhpUnit,
    CTest,
    Dotnet,
}

impl Profile {
    /// Detects the runner from the first few hundred lines of its output.
    pub(super) fn detect(lines: &[&str]) -> Option<Self> {
        lines.iter().take(400).find_map(|l| match l.trim_start() {
            t if t.starts_with("Nextest run ID ") || t.starts_with("Summary [") => {
                Some(Self::Nextest)
            }
            t if t.starts_with("PASS [")
                || t.starts_with("FAIL [")
                || t.starts_with("SLOW [")
                || t.starts_with("LEAK [")
                || t.starts_with("TRY ") =>
            {
                Some(Self::Nextest)
            }
            t if t.starts_with("Test Suite '") || t.starts_with("Test Case '-[") => {
                Some(Self::Xctest)
            }
            t if t.starts_with("◇ Test run started")
                || t.starts_with("✔ Test ")
                || t.starts_with("✘ Test ") =>
            {
                Some(Self::SwiftTesting)
            }
            t if t.starts_with("bun test v") => Some(Self::Bun),
            t if t.starts_with("running ") && t.contains(" tests from ") => Some(Self::Deno),
            t if t.starts_with("PHPUnit ") => Some(Self::PhpUnit),
            t if t.starts_with("Test project ") => Some(Self::CTest),
            t if t.starts_with("Passed!  - Failed:")
                || t.starts_with("Determining projects to restore")
                || t.starts_with("Test run for ") =>
            {
                Some(Self::Dotnet)
            }
            t if t.starts_with("=== RUN ")
                || t.starts_with("--- PASS: ")
                || t.starts_with("ok  \t") =>
            {
                Some(Self::GoTest)
            }
            t if t.contains("test session starts") || t.contains(" -- Python ") => {
                Some(Self::Pytest)
            }
            t if t.contains(" examples, ")
                && (t.contains(" failures") || t.contains(" failure")) =>
            {
                Some(Self::RSpec)
            }
            t if t.starts_with("Test Files ")
                || t.starts_with("Tests  ")
                || t.starts_with("Tests:") =>
            {
                Some(Self::Vitest)
            }
            t if t.starts_with("test result: ")
                || (t.starts_with("running ") && t.ends_with(" tests")) =>
            {
                Some(Self::Cargo)
            }
            t if t.starts_with("test ") && t.contains(" ... ") => Some(Self::Cargo),
            _ => None,
        })
    }

    /// The name `--explain` prints.
    pub(super) const fn name(self) -> &'static str {
        match self {
            Self::Cargo => "cargo test",
            Self::Nextest => "cargo nextest",
            Self::Xctest => "swift test (XCTest)",
            Self::SwiftTesting => "swift-testing",
            Self::Pytest => "pytest",
            Self::GoTest => "go test",
            Self::Vitest => "vitest/jest",
            Self::Bun => "bun test",
            Self::Deno => "deno test",
            Self::RSpec => "rspec",
            Self::PhpUnit => "phpunit",
            Self::CTest => "ctest",
            Self::Dotnet => "dotnet test",
        }
    }

    /// True for a line that reports a passing test.
    pub(super) fn pass(self, line: &str) -> bool {
        let l = line.trim_start();
        match self {
            Self::Cargo => l.starts_with("test ") && l.ends_with(" ... ok"),
            Self::Nextest => l.starts_with("PASS ["),
            Self::Xctest => l.starts_with("Test Case '") && l.contains("]' passed ("),
            Self::SwiftTesting => l.starts_with("✔ Test ") && l.contains(" passed after "),
            Self::Pytest => l.contains(" PASSED"),
            Self::GoTest => l.starts_with("--- PASS: "),
            Self::Vitest => ["✓ ", "✔ ", "√ ", "PASS "].iter().any(|p| l.starts_with(p)),
            Self::Bun => l.starts_with("(pass) "),
            Self::Deno => l.starts_with("test ") && l.contains(" ... ok"),
            Self::CTest => l.contains(" Test #") && l.contains("Passed"),
            Self::Dotnet => l.starts_with("Passed ") && l.contains('['),
            Self::RSpec | Self::PhpUnit => false,
        }
    }

    /// True for a line that reports a skipped test.
    pub(super) fn skip(self, line: &str) -> bool {
        let l = line.trim_start();
        match self {
            Self::Cargo => l.starts_with("test ") && l.ends_with(" ... ignored"),
            Self::Nextest => l.starts_with("SKIP ["),
            Self::Xctest => l.starts_with("Test Case '") && l.contains("]' skipped"),
            Self::SwiftTesting => l.starts_with("↷ "),
            Self::Pytest => l.contains(" SKIPPED"),
            Self::GoTest => l.starts_with("--- SKIP: "),
            Self::Deno => l.starts_with("test ") && l.contains(" ... ignored"),
            Self::Dotnet => l.starts_with("Skipped "),
            Self::Vitest | Self::Bun | Self::RSpec | Self::PhpUnit | Self::CTest => false,
        }
    }
}
