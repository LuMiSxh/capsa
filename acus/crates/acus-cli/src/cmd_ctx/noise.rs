// SPDX-License-Identifier: MPL-2.0
//! Drops the output lines that only cost tokens: progress, status and passing-test lines,
//! colour codes, `\r` redraws and repeats. Failures and program output stay.

use regex::{Regex, RegexSet};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

/// Progress, status and passing-test lines of common toolchains. Anchored and specific, so
/// program output and failures stay; `[ctx] drop` in the config adds more.
const NOISE: &[&str] = &[
    // cargo indents its status words; requiring that keeps program output like "Running x".
    r"^\s{2,}(Compiling|Checking|Downloaded|Downloading|Fresh|Blocking|Updating|Locking|Adding|Removing|Finished|Running|Doc-tests|Documenting|Packaging|Verifying|Installing|Installed|Replacing|Replaced) ",
    r"^test .* \.\.\. ok$",
    r"^running \d+ tests?$",
    // Dot progress (cargo -q, pytest -q) without failures.
    r"^[.s]+\s*(\[\s*\d+%\])?$",
    r"^\S+\.py [.s]+\s*(\[\s*\d+%\])?$",
    // SwiftPM, ninja, cmake.
    r"^\[\d+/\d+\] (Compiling|Emitting|Linking|Write|Build|Planning|Applying|Building|Generating|Copying)",
    r"^\[\s*\d+%\] (Building|Linking|Built target|Generating)",
    r"^make(\[\d+\])?: (Entering|Leaving|Nothing to be done)",
    // xcodebuild steps and the command lines under them.
    r"^(CompileC|CompileSwift|CompileSwiftSources|SwiftCompile|SwiftDriver|SwiftDriverJobDiscovery|SwiftEmitModule|SwiftMergeGeneratedHeaders|EmitSwiftModule|Ld|Libtool|ProcessInfoPlistFile|CodeSign|CpResource|CopySwiftLibs|ProcessProductPackaging|ProcessProductPackagingDER|GenerateDSYMFile|Touch|RegisterExecutionPolicyException|RegisterWithLaunchServices|Validate|ValidateEmbeddedBinary|WriteAuxiliaryFile|MkDir|CreateBuildDirectory|PhaseScriptExecution|ExtractAppIntentsMetadata|AppIntentsSSUTraining|ClangStatCache|LinkAssetCatalog|CompileAssetCatalog|CompileAssetCatalogVariant|GenerateAssetSymbols|CopyPlistFile|SymLink|ComputeTargetDependencyGraph|ComputePackagePrebuildTargetDependencyGraph|CreateUniversalBinary|Copy|Ditto|ScanDependencies|PrecompileModule|SwiftExplicitDependencyCompileModuleFromInterface|SwiftExplicitDependencyGeneratePcm|ProcessXCFramework|CompileStoryboard|CompileXIB|LinkStoryboards|GenerateTAPI|Strip|SetMode|SetOwnerAndGroup) ",
    r"^    (cd |export |builtin-|/Applications/Xcode|/usr/bin/|/Library/Developer/)",
    r"^(Prepare packages|Resolve Package Graph|Resolved source packages:|Command line invocation:|Build settings from command line:|User defaults from command line:|Writing result bundle at path:|Computing target dependency graph and provisioning inputs|Create build description|Build description path:|note: Building targets in dependency order|note: Run script build phase|note: Target dependency graph)",
    r"^Test (Case|Suite) '.*' (passed|started)",
    // swift-testing: "✔ Test foo() passed after 0.001 seconds."
    r"^\S{1,2} (Test|Suite) .*(started\.|passed after [\d.]+ seconds\.)$",
    // vitest / jest / mocha / node:test.
    r"^\s*[✓✔√] ",
    r"^ ?PASS ",
    r"^ok \d+ - ",
    // pytest headers and -v passes.
    r"^=+ test session starts =+$",
    r"^(platform \S+ -- Python|rootdir: |plugins: |cachedir: |configfile: |testpaths: |collecting \.\.\.|collected \d+ items?)",
    r"^\S+::\S+ PASSED",
    // go test -v.
    r"^ok  \t",
    r"^=== (RUN|PAUSE|CONT|NAME) ",
    r"^\s*--- PASS: ",
    r"^PASS$",
    // npm, pnpm, yarn, bun.
    r"^> \S+@\S+ \S+",
    r"^(Progress: resolved|Packages: [+-]|Done in [\d.]+m?s|Already up to date|Lockfile is up to date|Scope: all \d+ workspace|Resolving: total|Downloading: total|\$ \S+ \S+ \S+|added \d+ packages?|up to date, audited|found 0 vulnerabilities|\d+ packages? (are|is) looking for funding|  run `npm fund`)",
    r"^[+-]{2,}$",
    // pip, uv, poetry.
    r"^(Requirement already satisfied: |Collecting |  Downloading \S+|  Using cached |Using cached |Installing collected packages: |(Resolved|Prepared|Installed|Audited|Uninstalled|Built) \d+ packages? in )",
    // gradle.
    r"^> Task :\S+( UP-TO-DATE| NO-SOURCE| FROM-CACHE| SKIPPED)?$",
    // svelte-check, mix.
    r"^(Loading svelte-check|Getting Svelte diagnostics|====================================)",
    r"^(==> \S+$|Compiling \d+ files? \(\.\w+\)$|Generated \S+ app$)",
    // Stack frames inside the toolchain or dependencies; frames in your own code stay.
    r"^\s+\d+: +(std|core|alloc|test|rust_begin_unwind|__rust|<unknown>|_?_?pthread|thread_start|_start\b|start_thread|clone3?\b)",
    r"^\s+at (/rustc/|.*(node:internal|node_modules[/\\]|/\.cargo/registry/))",
    r"^note: (run with `RUST_BACKTRACE=1`|Some details are omitted, run with `RUST_BACKTRACE=full`)",
    // GitHub Actions log groups.
    r"^##\[endgroup\]",
];

/// A Python traceback frame outside the project; the code lines indented below it go too.
#[expect(clippy::unwrap_used, reason = "a constant pattern")]
static PY_LIB_FRAME: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"^(\s*)File "[^"]*(site-packages|dist-packages|/lib/python\d[\d.]*/|<frozen )"#)
        .unwrap()
});

#[expect(clippy::unwrap_used, reason = "a constant pattern list")]
static NOISE_SET: LazyLock<RegexSet> = LazyLock::new(|| RegexSet::new(NOISE).unwrap());

/// Writes the full output to a temp file named after the directory and command, so a rerun
/// replaces its previous log.
pub(super) fn save_log(cwd: &Path, command: &str, text: &str) -> Option<PathBuf> {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    (cwd, command).hash(&mut h);
    let dir = std::env::temp_dir().join("acus-ctx");
    std::fs::create_dir_all(&dir).ok()?;
    let path = dir.join(format!("{:016x}.log", h.finish()));
    std::fs::write(&path, text).ok()?;
    Some(path)
}

/// The output as a terminal would show it: no colour codes, and only the final state of lines
/// a progress bar redrew with `\r`.
pub(super) fn clean(text: &str) -> String {
    #[expect(clippy::unwrap_used, reason = "a constant pattern")]
    static ANSI: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"\x1b(\[[0-9;?]*[ -/]*[@-~]|\][^\x07\x1b]*(\x07|\x1b\\)|[()][0-9A-B])").unwrap()
    });
    // `gh run view --log` lines start with the job, step and timestamp fields.
    #[expect(clippy::unwrap_used, reason = "a constant pattern")]
    static GH_PREFIX: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r"(?m)^[^\t\n]*\t[^\t\n]*\t\x{FEFF}?\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d+)?Z ?",
        )
        .unwrap()
    });
    let text = ANSI.replace_all(text, "");
    GH_PREFIX
        .replace_all(&text, "")
        .lines()
        .map(|l| l.rsplit('\r').find(|s| !s.is_empty()).unwrap_or(""))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Drops noise lines, blank runs and repeats. Returns the kept lines, the passed tests and the
/// dropped count.
pub(super) fn quiet(lines: &[&str], extra: &RegexSet) -> (Vec<String>, usize, usize) {
    let mut passed = 0;
    let mut v: Vec<String> = Vec::new();
    let mut repeat = 0;
    let flush = |v: &mut Vec<String>, repeat: &mut usize| {
        if *repeat > 0 {
            if let Some(last) = v.last_mut() {
                *last = format!("{last} (×{})", *repeat + 1);
            }
            *repeat = 0;
        }
    };
    let mut frame: Option<usize> = None;
    let mut noise = |l: &str| {
        let ind = l.len() - l.trim_start().len();
        if let Some(f) = frame {
            if ind > f && !l.trim().is_empty() {
                return true;
            }
            frame = None;
        }
        if let Some(c) = PY_LIB_FRAME.captures(l) {
            frame = Some(c[1].len());
            return true;
        }
        NOISE_SET.is_match(l) || extra.is_match(l)
    };
    for &l in lines.iter().filter(|l| !noise(l)) {
        if let Some(r) = l.trim_start().strip_prefix("test result: ok. ") {
            passed += r
                .split(' ')
                .next()
                .and_then(|n| n.parse::<usize>().ok())
                .unwrap_or(0);
            continue;
        }
        if l.trim().is_empty() && v.last().is_none_or(|p| p.trim().is_empty()) {
            continue;
        }
        if !l.trim().is_empty() && v.last().is_some_and(|p| p == l) {
            repeat += 1;
            continue;
        }
        flush(&mut v, &mut repeat);
        v.push(l.to_string());
    }
    flush(&mut v, &mut repeat);
    let dropped = lines.len() - v.len();
    (v, passed, dropped)
}

#[cfg(test)]
mod tests {
    use super::{clean, quiet};
    use regex::RegexSet;

    #[test]
    fn drops_build_noise_but_keeps_failures() {
        let out = "   Compiling foo v0.1.0 (/x)\n    Finished `test` profile\n     Running unittests src/lib.rs\n\nrunning 2 tests\ntest a ... ok\ntest b ... FAILED\n\n\nfailures:\ntest result: FAILED. 1 passed; 1 failed\n\nrunning 0 tests\ntest result: ok. 0 passed; 0 failed\n[3/9] Compiling Foo Bar.swift\nerror: x\nRunning is a word";
        let l: Vec<&str> = out.lines().collect();
        let (v, passed, dropped) = quiet(&l, &RegexSet::empty());
        assert_eq!((passed, dropped), (0, 10));
        assert_eq!(
            v,
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
        let l: Vec<&str> = text.lines().collect();
        let (v, _, _) = quiet(&l, &RegexSet::new(["^custom noise"]).unwrap());
        assert_eq!(
            v,
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
    fn drops_library_stack_frames() {
        let out = "thread 'main' panicked at src/main.rs:3:5:\nboom\nstack backtrace:\n   0: rust_begin_unwind\n             at /rustc/abc/library/std/src/panicking.rs:665:5\n   1: app::run\n             at ./src/main.rs:3:5\nnote: Some details are omitted, run with `RUST_BACKTRACE=full` for a verbose backtrace.\nTraceback (most recent call last):\n  File \"/x/app.py\", line 3, in <module>\n    go()\n  File \"/usr/lib/python3.12/site-packages/lib.py\", line 9, in go\n    raise X\n    ^^^^^^^\nX: bad\n    at run (src/a.ts:4:9)\n    at Module._compile (node:internal/modules/cjs/loader:1105:14)";
        let l: Vec<&str> = out.lines().collect();
        let (v, _, _) = quiet(&l, &RegexSet::empty());
        assert_eq!(
            v,
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
}
