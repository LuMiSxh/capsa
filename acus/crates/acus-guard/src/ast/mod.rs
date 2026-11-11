// SPDX-License-Identifier: MPL-2.0
//! Owns the split of a command line into the simple commands the rules judge, each with its stdin,
//! stdout and nesting. Each shell has a tree-sitter backend behind a `guard-*` feature
//! (`bash`, `zsh`, `powershell`), and the hand-written splitter in `legacy` serves a build without
//! grammars.

#[cfg(any(feature = "bash", feature = "zsh"))]
mod bash;
#[cfg(any(feature = "bash", feature = "zsh", feature = "powershell"))]
mod climb;
mod legacy;
#[cfg(feature = "powershell")]
mod powershell;
#[cfg(feature = "zsh")]
mod zsh;

use super::*;
use std::ops::Range;

/// Parses `src` in `lang` with this thread's parser, so no parser is built per command.
#[cfg(any(feature = "bash", feature = "zsh", feature = "powershell"))]
pub(super) fn parse_tree(src: &str, lang: &tree_sitter::Language) -> Option<tree_sitter::Tree> {
    thread_local! {
        static PARSER: std::cell::RefCell<tree_sitter::Parser> =
            std::cell::RefCell::new(tree_sitter::Parser::new());
    }
    PARSER.with_borrow_mut(|parser| {
        parser.set_language(lang).ok()?;
        parser.reset();
        parser.parse(src, None)
    })
}

#[derive(Clone, Copy, PartialEq)]
pub(super) enum Dialect {
    Bash,
    PowerShell,
}

/// Holds where a segment's stdout goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Out {
    /// To the agent: nothing redirects it.
    Agent,
    /// Into a pipe.
    Pipe,
    /// Into a file (or another descriptor).
    File,
    /// Into `$( )`, backticks, `<( )` or a function body: data for something other than the agent.
    /// The splitter cannot tell these from a group, so only the grammar backends make one.
    #[cfg_attr(
        not(any(feature = "bash", feature = "zsh", feature = "powershell")),
        expect(
            dead_code,
            reason = "only the grammar backends build a capture segment; the splitter-only build never does"
        )
    )]
    Capture,
}

/// Holds how deep in compound commands a segment sits.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum Nest {
    Top,
    /// `{ }`, `( )`, `if` or `case`.
    Group,
    /// `for`, `while` or `until`, which run the segment more than once. Only the grammar
    /// backends can tell (the splitter has no loops), so a build without them never makes one.
    #[cfg_attr(
        not(any(feature = "bash", feature = "zsh", feature = "powershell")),
        expect(
            dead_code,
            reason = "only the grammar backends see loops; the splitter-only build never makes one"
        )
    )]
    Loop,
}

/// Holds one simple command of a line, parsed once and used by every rule.
pub(super) struct Segment {
    /// The text the rules read: lowercased for PowerShell, whose cmdlets ignore case.
    pub(super) low: String,
    /// The same segment in its original case, which refusals and rewrites quote. `None` when it
    /// reads as `low` does; use `orig()`.
    orig: Option<String>,
    /// Where the segment is in the source line (Bash only; empty for PowerShell). The range
    /// holds the segment's redirects (`2>&1`) but no heredoc body.
    pub(super) range: Range<usize>,
    /// Where the command word starts in `range`, past the assignments and the space after them.
    pub(super) lead: usize,
    /// Whether it reads a pipe, a heredoc or a here-string.
    pub(super) stdin: bool,
    pub(super) out: Out,
    pub(super) nest: Nest,
    /// Whether `&&` or `||` follows, so its exit status decides what runs next.
    pub(super) gates: bool,
    /// How many `VAR=value` words precede the command word.
    pub(super) assigns: usize,
    /// Whether a heredoc is attached, whose body lies outside `range`.
    pub(super) heredoc: bool,
}

/// Returns `orig` for a segment whose rules-text is `low`, or `None` when they are the same.
pub(super) fn distinct(low: &str, orig: &str) -> Option<String> {
    (orig != low).then(|| orig.to_owned())
}

/// Returns `text` lowercased, borrowing it when ASCII has nothing to lower.
pub(super) fn lowered(text: &str) -> std::borrow::Cow<'_, str> {
    if text.is_ascii() && !text.bytes().any(|b| b.is_ascii_uppercase()) {
        std::borrow::Cow::Borrowed(text)
    } else {
        std::borrow::Cow::Owned(text.to_lowercase())
    }
}

/// Returns `text` lowercased, reusing it when ASCII has nothing to lower.
#[cfg(feature = "powershell")]
pub(super) fn lower_string(text: String) -> String {
    match lowered(&text) {
        std::borrow::Cow::Borrowed(_) => text,
        std::borrow::Cow::Owned(lower) => lower,
    }
}

/// A heredoc body with the segments of the line that opened it.
pub(super) struct Doc {
    pub(super) openers: Vec<String>,
    pub(super) body: String,
}

pub(super) struct Parsed {
    pub(super) segs: Vec<Segment>,
    pub(super) docs: Vec<Doc>,
}

/// Names a way to split a command line into segments.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Backend {
    Bash,
    Zsh,
    PowerShell,
    Legacy,
}

/// Returns which grammars this build has.
#[derive(Clone, Copy)]
pub(super) struct Grammars {
    pub(super) bash: bool,
    pub(super) zsh: bool,
    pub(super) powershell: bool,
}

impl Grammars {
    const COMPILED: Grammars = Grammars {
        bash: cfg!(feature = "bash"),
        zsh: cfg!(feature = "zsh"),
        powershell: cfg!(feature = "powershell"),
    };
}

/// Chooses the backend for a tool, from the basename of the shell the hook runs under
/// (`$SHELL`). Claude Code runs Bash-tool commands in the user's login shell, so under zsh the
/// zsh grammar reads them as the shell does; without it the Bash grammar is the next best, then
/// zsh, then the splitter. PowerShell uses its own grammar if there is one.
pub(super) fn choose(d: Dialect, shell: Option<&str>, have: Grammars) -> Backend {
    match d {
        Dialect::PowerShell if have.powershell => Backend::PowerShell,
        Dialect::PowerShell => Backend::Legacy,
        Dialect::Bash if have.zsh && shell == Some("zsh") => Backend::Zsh,
        Dialect::Bash if have.bash => Backend::Bash,
        Dialect::Bash if have.zsh => Backend::Zsh,
        Dialect::Bash => Backend::Legacy,
    }
}

/// Returns the last component of a shell path (`/bin/zsh`, `C:\msys\usr\bin\zsh.exe`), without
/// `.exe`.
pub(super) fn shell_name(path: &str) -> &str {
    let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
    name.strip_suffix(".exe").unwrap_or(name)
}

/// Returns the shell this process runs under, read once.
pub(super) fn current_shell() -> Option<String> {
    #[cfg(test)]
    return tests_shell::get().map(str::to_owned);
    #[cfg(not(test))]
    {
        static SHELL: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
        SHELL
            .get_or_init(|| {
                std::env::var("SHELL")
                    .ok()
                    .map(|s| shell_name(&s).to_owned())
            })
            .clone()
    }
}

/// Returns the segments of `src`, or `None` if it cannot be parsed; the call then runs unchanged.
pub(super) fn parse(src: &str, d: Dialect) -> Option<Parsed> {
    let shell = current_shell();
    parse_with(src, d, choose(d, shell.as_deref(), Grammars::COMPILED))
}

/// Runs `parse` on a given backend.
pub(super) fn parse_with(src: &str, d: Dialect, backend: Backend) -> Option<Parsed> {
    let legacy = || {
        Some(match d {
            Dialect::Bash => legacy::parse::<legacy::Sh>(src),
            Dialect::PowerShell => legacy::parse::<legacy::Ps>(src),
        })
    };
    match backend {
        #[cfg(feature = "bash")]
        Backend::Bash => bash::parse(src, &tree_sitter_bash::LANGUAGE.into()),
        #[cfg(feature = "zsh")]
        Backend::Zsh => zsh::parse(src),
        // A line the PowerShell grammar rejects is still read by the splitter, as before.
        #[cfg(feature = "powershell")]
        Backend::PowerShell => powershell::parse(src).or_else(legacy),
        _ => legacy(),
    }
}

impl Segment {
    /// Reports whether it reads a pipe.
    pub(super) fn piped(&self) -> bool {
        self.stdin
    }

    /// Reports whether its stdout is data rather than output the agent reads: it is redirected, or
    /// the segment sits in a substitution. Such a segment runs as it is, because acus output has
    /// headers and line numbers.
    pub(super) fn captured(&self) -> bool {
        matches!(self.out, Out::File | Out::Capture)
    }

    /// Reports whether the rules leave it alone: captured, or inside a group or loop, where its
    /// output is not the whole result of the call.
    pub(super) fn opaque(&self) -> bool {
        self.captured() || self.nest != Nest::Top
    }

    /// Reports whether its output goes into a pipe that `next` reads.
    pub(super) fn feeds(&self, next: &Segment) -> bool {
        self.out == Out::Pipe && next.stdin
    }

    /// Returns `low` without the `command ` escape, and whether it had one.
    pub(super) fn low_unescaped(&self) -> (bool, &str) {
        unescape(&self.low)
    }

    /// Returns the segment in its original case.
    pub(super) fn orig(&self) -> &str {
        self.orig.as_deref().unwrap_or(&self.low)
    }

    /// Returns `orig` without the `command ` escape, and whether it had one.
    pub(super) fn orig_unescaped(&self) -> (bool, &str) {
        unescape(self.orig())
    }

    /// Returns `text` (`low`) from its command word on, past the leading assignments
    /// (`FOO=1 cat a`).
    pub(super) fn command_part<'a>(&self, text: &'a str) -> &'a str {
        let at = split_words(text)
            .get(self.assigns)
            .map_or(text.len(), |w| w.as_ptr() as usize - text.as_ptr() as usize);
        &text[at..]
    }
}

/// Keeps tests independent of the `$SHELL` of whoever runs them: it is `bash` unless a test
/// sets it.
#[cfg(test)]
pub(super) mod tests_shell {
    use std::cell::Cell;

    thread_local! {
        static SHELL: Cell<Option<&'static str>> = const { Cell::new(None) };
    }

    pub(super) fn get() -> Option<&'static str> {
        SHELL.get()
    }

    /// Runs `f` as if the hook ran under `shell`.
    pub(crate) fn with<T>(shell: Option<&'static str>, f: impl FnOnce() -> T) -> T {
        let old = SHELL.replace(shell);
        let out = f();
        SHELL.set(old);
        out
    }
}
