// SPDX-License-Identifier: MPL-2.0
//! The noise patterns, named by kind so `--explain` can say what was dropped.

use regex::{Regex, RegexSet};
use serde::Serialize;
use serde::ser::{SerializeMap, Serializer};
use std::sync::LazyLock;

/// Why a line was dropped; the order is the order `--explain` prints.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[repr(usize)]
pub(super) enum Category {
    Phase,
    Header,
    Progress,
    Pass,
    Skip,
    Summary,
    Separator,
    Framework,
    Duplicate,
    Fold,
    Blank,
    Extra,
}

impl Category {
    pub(super) const ALL: [Self; 12] = [
        Self::Phase,
        Self::Header,
        Self::Progress,
        Self::Pass,
        Self::Skip,
        Self::Summary,
        Self::Separator,
        Self::Framework,
        Self::Duplicate,
        Self::Fold,
        Self::Blank,
        Self::Extra,
    ];

    /// The word `--explain` uses for this kind.
    pub(super) const fn name(self) -> &'static str {
        match self {
            Self::Phase => "build",
            Self::Header => "header",
            Self::Progress => "progress",
            Self::Pass => "pass",
            Self::Skip => "skip",
            Self::Summary => "summary",
            Self::Separator => "separator",
            Self::Framework => "framework",
            Self::Duplicate => "duplicate",
            Self::Fold => "folded",
            Self::Blank => "blank",
            Self::Extra => "config",
        }
    }
}

/// Progress, status and passing-test lines of common toolchains. Anchored and specific, so
/// program output and failures stay; `[run] drop` in the config adds more.
pub(super) const PATTERNS: &[(Category, &str)] = &[
    // cargo phases; the indent keeps program output like "Running x".
    (
        Category::Phase,
        r"^\s{2,}(Compiling|Checking|Downloaded|Downloading|Fresh|Blocking|Updating|Locking|Adding|Removing|Finished|Running|Doc-tests|Documenting|Packaging|Verifying|Installing|Installed|Replacing|Replaced) ",
    ),
    // SwiftPM, ninja, cmake.
    (
        Category::Phase,
        r"^\[\d+/\d+\] (Compiling|Emitting|Linking|Write|Build|Planning|Applying|Building|Generating|Copying)",
    ),
    (
        Category::Phase,
        r"^\[\s*\d+%\] (Building|Linking|Built target|Generating)",
    ),
    (
        Category::Phase,
        r"^make(\[\d+\])?: (Entering|Leaving|Nothing to be done)",
    ),
    // xcodebuild steps and the command lines under them.
    (
        Category::Phase,
        r"^(CompileC|CompileSwift|CompileSwiftSources|SwiftCompile|SwiftDriver|SwiftDriverJobDiscovery|SwiftEmitModule|SwiftMergeGeneratedHeaders|EmitSwiftModule|Ld|Libtool|ProcessInfoPlistFile|CodeSign|CpResource|CopySwiftLibs|ProcessProductPackaging|ProcessProductPackagingDER|GenerateDSYMFile|Touch|RegisterExecutionPolicyException|RegisterWithLaunchServices|Validate|ValidateEmbeddedBinary|WriteAuxiliaryFile|MkDir|CreateBuildDirectory|PhaseScriptExecution|ExtractAppIntentsMetadata|AppIntentsSSUTraining|ClangStatCache|LinkAssetCatalog|CompileAssetCatalog|CompileAssetCatalogVariant|GenerateAssetSymbols|CopyPlistFile|SymLink|ComputeTargetDependencyGraph|ComputePackagePrebuildTargetDependencyGraph|CreateUniversalBinary|Copy|Ditto|ScanDependencies|PrecompileModule|SwiftExplicitDependencyCompileModuleFromInterface|SwiftExplicitDependencyGeneratePcm|ProcessXCFramework|CompileStoryboard|CompileXIB|LinkStoryboards|GenerateTAPI|Strip|SetMode|SetOwnerAndGroup) ",
    ),
    (
        Category::Phase,
        r"^    (cd |export |builtin-|/Applications/Xcode|/usr/bin/|/Library/Developer/)",
    ),
    // npm, pnpm, yarn, bun, pip, uv, poetry, gradle.
    (Category::Phase, r"^> \S+@\S+ \S+"),
    (
        Category::Phase,
        r"^(Progress: resolved|Packages: [+-]|Done in [\d.]+m?s|Already up to date|Lockfile is up to date|Scope: all \d+ workspace|Resolving: total|Downloading: total|\$ \S+ \S+ \S+|added \d+ packages?|up to date, audited|found 0 vulnerabilities|\d+ packages? (are|is) looking for funding|  run `npm fund`)",
    ),
    (
        Category::Phase,
        r"^(Requirement already satisfied: |Collecting |  Downloading \S+|  Using cached |Using cached |Installing collected packages: |(Resolved|Prepared|Installed|Audited|Uninstalled|Built) \d+ packages? in )",
    ),
    (
        Category::Phase,
        r"^> Task :\S+( UP-TO-DATE| NO-SOURCE| FROM-CACHE| SKIPPED)?$",
    ),
    // svelte-check, mix.
    (
        Category::Phase,
        r"^(==> \S+$|Compiling \d+ files? \(\.\w+\)$|Generated \S+ app$)",
    ),
    // Headers and one-off announcements.
    (Category::Header, r"^running \d+ tests?( from \S+)?$"),
    (Category::Header, r"^\s*Nextest run ID \S+"),
    (
        Category::Header,
        r"^\s*Starting \d+ tests? across \d+ binar(?:y|ies)\b",
    ),
    (
        Category::Header,
        r"^(Prepare packages|Resolve Package Graph|Resolved source packages:|Command line invocation:|Build settings from command line:|User defaults from command line:|Writing result bundle at path:|Computing target dependency graph and provisioning inputs|Create build description|Build description path:|note: Building targets in dependency order|note: Run script build phase|note: Target dependency graph)",
    ),
    (Category::Header, r"^=+ test session starts =+$"),
    (
        Category::Header,
        r"^(platform \S+ -- Python|rootdir: |plugins: |cachedir: |configfile: |testpaths: |collecting \.\.\.|collected \d+ items?)",
    ),
    (Category::Header, r"^bun test v\d"),
    (Category::Header, r"^PHPUnit \S+ by Sebastian Bergmann"),
    (Category::Header, r"^Runtime:\s+PHP "),
    (Category::Header, r"^Configuration: .*phpunit"),
    (Category::Header, r"^Time: \d\d:\d\d\.\d+, Memory: "),
    (Category::Header, r"^Test project \S"),
    (
        Category::Header,
        r"^(Loading svelte-check|Getting Svelte diagnostics|====================================)",
    ),
    (Category::Header, r"^##\[endgroup\]"),
    // SwiftPM completion and swift-testing's library banner.
    (
        Category::Header,
        r"^Building for (debugging|testing)\.\.\.$",
    ),
    (Category::Header, r"^Build complete! \([\d.]+s\)$"),
    (Category::Header, r"^↳ Testing Library Version: "),
    // dotnet test and vitest banners.
    (
        Category::Phase,
        r"^\s*Determining projects to restore\.\.\.$",
    ),
    (
        Category::Phase,
        r"^\s*All projects are up-to-date for restore\.$",
    ),
    (Category::Phase, r"^\s*\S+ -> \S+\.dll$"),
    (
        Category::Header,
        r"^Starting test execution, please wait\.\.\.$",
    ),
    (Category::Header, r"^A total of \d+ test files? matched"),
    (Category::Header, r"^Test run for \S+\.dll "),
    (Category::Header, r"^Test Run Successful\.$"),
    (Category::Header, r"^\s*(Start at|Duration)\s+"),
    // Progress that carries no result.
    (Category::Progress, r"^\s*START\s{2,}\S"),
    (Category::Progress, r"^[.s]+\s*(\[\s*\d+%\])?$"),
    (Category::Progress, r"^\S+\.py [.s]+\s*(\[\s*\d+%\])?$"),
    (Category::Progress, r"^=== (RUN|PAUSE|CONT|NAME) "),
    (Category::Progress, r"^\s*Start\s+\d+: \S"),
    (Category::Progress, r"^[.s]*\s+\d+ / \d+ \(\s*\d+%\)$"),
    // Passing tests of every runner.
    (Category::Pass, r"^test .* \.\.\. ok$"),
    (Category::Pass, r"^\s*PASS\s+\[\s*[\d.]+m?s\]"),
    (Category::Pass, r"^Test (Case|Suite) '.*' passed"),
    (Category::Progress, r"^Test (Case|Suite) '.*' started"),
    (
        Category::Pass,
        r"^\S{1,2} (Test|Suite) .* passed after [\d.]+ seconds\.$",
    ),
    (Category::Progress, r"^\S{1,2} (Test|Suite) .* started\.$"),
    (Category::Pass, r"^\s*[✓✔√] "),
    (Category::Pass, r"^ ?PASS "),
    (Category::Pass, r"^ok \d+ - "),
    (Category::Pass, r"^\S+::\S+ PASSED"),
    (Category::Pass, r"^\s*--- PASS: "),
    (Category::Pass, r"^\(pass\) "),
    (Category::Pass, r"^\s*\d+/\d+ Test\s+#\d+: .*Passed\b"),
    (Category::Pass, r"^  Passed \S"),
    (Category::Skip, r"^  (Skipped|NotRun) "),
    // Separators and rule lines.
    (Category::Separator, r"^\s*─{3,}$"),
    (Category::Separator, r"^[+-]{2,}$"),
    // Summaries that are already counted, or carry no count to parse.
    (Category::Summary, r"^ok  \t"),
    (Category::Summary, r"^PASS$"),
    (Category::Summary, r"^\s*\d+ pass$"),
    (Category::Summary, r"^\s*0 fail$"),
    (Category::Summary, r"^\s*\d+ expect\(\) calls?$"),
    (Category::Summary, r"^Ran \d+ tests? across \d+ files?\."),
    (Category::Summary, r"^ok \| \d+ passed \| 0 failed"),
    (Category::Summary, r"^Finished in [\d.]+ seconds?"),
    (Category::Summary, r"^\d+ examples?, 0 failures?\b"),
    (Category::Summary, r"^OK \(\d+ tests?, \d+ assertions?\)"),
    (
        Category::Summary,
        r"^\s*\d+% tests passed, 0 tests failed out of \d+",
    ),
    (Category::Summary, r"^\s*Test Files\s+\d+ passed"),
    (
        Category::Summary,
        r"^Total Test time \(real\) =\s+[\d.]+ sec",
    ),
    // Stack frames inside the toolchain or dependencies; frames in your own code stay.
    (
        Category::Framework,
        r"^\s+\d+: +(std|core|alloc|test|rust_begin_unwind|__rust|<unknown>|_?_?pthread|thread_start|_start\b|start_thread|clone3?\b)",
    ),
    (
        Category::Framework,
        r"^\s+at (/rustc/|.*(node:internal|node_modules[/\\]|/\.cargo/registry/))",
    ),
    (
        Category::Framework,
        r"^note: (run with `RUST_BACKTRACE=1`|Some details are omitted, run with `RUST_BACKTRACE=full`)",
    ),
];

/// Why lines were dropped, counted for `--explain`.
#[derive(Debug)]
pub(in crate::cmd_run) struct Stats {
    enabled: bool,
    counts: [usize; Category::ALL.len()],
}

impl Default for Stats {
    fn default() -> Self {
        Self {
            enabled: false,
            counts: [0; Category::ALL.len()],
        }
    }
}

impl Stats {
    pub(in crate::cmd_run) fn new(enabled: bool) -> Self {
        Self {
            enabled,
            ..Self::default()
        }
    }

    pub(super) fn add(&mut self, category: Category) {
        self.add_n(category, 1);
    }

    pub(super) fn add_n(&mut self, category: Category, n: usize) {
        if self.enabled && n > 0 {
            self.counts[category as usize] += n;
        }
    }

    pub(in crate::cmd_run) fn is_empty(&self) -> bool {
        self.counts.iter().all(|n| *n == 0)
    }

    /// The `--explain` line, such as `build 12, pass 425, summary 1`.
    pub(in crate::cmd_run) fn render(&self) -> String {
        Category::ALL
            .iter()
            .zip(self.counts)
            .filter(|(_, n)| *n > 0)
            .map(|(c, n)| format!("{} {n}", c.name()))
            .collect::<Vec<_>>()
            .join(", ")
    }
}

impl Serialize for Stats {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(None)?;
        for (c, n) in Category::ALL.iter().zip(self.counts) {
            if n > 0 {
                map.serialize_entry(c.name(), &n)?;
            }
        }
        map.end()
    }
}

/// A Python traceback frame outside the project; the code lines indented below it go too.
#[expect(clippy::unwrap_used, reason = "a constant pattern")]
pub(super) static PY_LIB_FRAME: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"^(\s*)File "[^"]*(site-packages|dist-packages|/lib/python\d[\d.]*/|<frozen )"#)
        .unwrap()
});

#[expect(clippy::unwrap_used, reason = "a constant pattern list")]
static NOISE_SET: LazyLock<RegexSet> =
    LazyLock::new(|| RegexSet::new(PATTERNS.iter().map(|(_, p)| *p)).unwrap());

/// True when any noise pattern matches `line`.
pub(super) fn matches(line: &str) -> bool {
    NOISE_SET.is_match(line)
}

/// The kind of the first noise pattern matching `line`, for `--explain`.
pub(super) fn category(line: &str) -> Option<Category> {
    NOISE_SET.matches(line).iter().next().map(|i| PATTERNS[i].0)
}

#[cfg(test)]
mod tests {
    use super::Category;

    #[test]
    fn category_order_matches_its_all_list() {
        for (i, c) in Category::ALL.iter().enumerate() {
            assert_eq!(*c as usize, i);
        }
    }
}
