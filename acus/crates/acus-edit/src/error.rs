// SPDX-License-Identifier: MPL-2.0
//! What a patch can get wrong, and the fix for each failure. Parse errors name the patch line;
//! apply errors name the file and, where it helps, the block.

use capsa_core::hint::Hint;
use std::borrow::Cow;
use std::io;

/// `Result` with [`EditError`] as the error type.
pub type Result<T, E = EditError> = std::result::Result<T, E>;

const UPDATE_HINT: &str =
    "each change is `<<<<<<< SEARCH` / `old text` / `=======` / `new text` / `>>>>>>> REPLACE`";

/// Why a patch could not be parsed or applied. Nothing is written when one occurs.
#[derive(Debug, thiserror::Error)]
pub enum EditError {
    /// A directory named by `*** Replace All` cannot be walked.
    #[error(transparent)]
    Walk(#[from] acus_walk::WalkError),

    /// A directive needs `path#Symbol`.
    #[error("patch line {line}: expected `*** {directive}: path#Symbol`")]
    ExpectedSymbol { line: usize, directive: String },
    /// An insert needs `path#Symbol` or `path:LINE`.
    #[error("patch line {line}: expected `*** {directive}: path#Symbol` or `path:LINE`")]
    ExpectedSymbolOrLine { line: usize, directive: String },
    /// `*** Move to` without an `*** Update File` before it.
    #[error("patch line {line}: `*** Move to` must follow `*** Update File`")]
    MoveToWithoutUpdate { line: usize },
    /// `*** Before:` or `*** After:` without an `*** Move Symbol` before it.
    #[error(
        "patch line {line}: `*** {directive}:` must follow `*** Move Symbol:` or `*** Move Lines:`"
    )]
    DestinationWithoutMove { line: usize, directive: String },
    /// `*** To:` without an `*** Move Symbol` or `*** Copy File` before it.
    #[error("patch line {line}: `*** To:` must follow `*** Move Symbol:` or `*** Copy File:`")]
    ToWithoutMoveOrCopy { line: usize },
    /// A `***` line that is no directive. The hint lists the accepted ones.
    #[error("patch line {line}: unknown directive `{text}`")]
    UnknownDirective {
        line: usize,
        text: String,
        hint: String,
    },
    /// A SEARCH/REPLACE block still open at the end of the patch.
    #[error("unterminated SEARCH/REPLACE block")]
    UnterminatedBlock,
    /// `*** Update File` that lists no block.
    #[error("{path}: `*** Update File` without changes")]
    NoChanges { path: String },
    /// `*** Replace All` without paths or blocks.
    #[error("`*** Replace All` needs paths and at least one SEARCH/REPLACE block")]
    ReplaceAllEmpty,
    /// `*** Move Symbol` without a destination.
    #[error("{path}#{symbol}: `*** Move Symbol` needs a destination")]
    MoveSymbolNoDestination { path: String, symbol: String },
    /// `*** Copy File` without `*** To:`.
    #[error("{from}: `*** Copy File` needs a destination")]
    CopyFileNoDestination { from: String },
    /// `*** Insert` without `+` lines.
    #[error("{path}: `*** Insert` needs `+` lines")]
    InsertNoLines { path: String },
    /// The patch holds no directive.
    #[error("empty patch")]
    EmptyPatch,
    /// A block whose SEARCH and REPLACE are equal.
    #[error("patch line {line}: SEARCH is identical to REPLACE, nothing to change")]
    IdenticalBlock { line: usize },
    /// Text where a block should open.
    #[error("patch line {line}: expected `<<<<<<< SEARCH` or `<<<<<<< REGEX`")]
    ExpectedBlock { line: usize },
    /// Replacement text outside a `*** Replace All` block.
    #[error("patch line {line}: text outside a block")]
    TextOutsideBlock { line: usize },
    /// Text outside a `*** Update File` block.
    #[error("patch line {line}: text outside a chunk")]
    TextOutsideChunk { line: usize },
    /// A SEARCH with no text to find.
    #[error("patch line {line}: empty SEARCH")]
    EmptySearch { line: usize },
    /// A line of an insert or symbol body without `+`.
    #[error("patch line {line}: every new line must start with `+`")]
    NotAddLine { line: usize },
    /// A REGEX block under `*** Update File`.
    #[error(
        "patch line {line}: REGEX blocks belong under `*** Replace All: PATH`, not `*** Update File`"
    )]
    RegexUnderUpdate { line: usize },
    /// Diff-style lines under `*** Update File`.
    #[error(
        "patch line {line}: *** Update File: takes SEARCH/REPLACE blocks, not @@/-/+ hunk lines"
    )]
    HunkLines { line: usize },
    /// A line under `*** Update File` that opens no block.
    #[error("patch line {line}: expected `<<<<<<< SEARCH` under `*** Update File`, got `{got}`")]
    ExpectedSearch { line: usize, got: String },
    /// Content after a directive that takes no body.
    #[error("patch line {line}: unexpected content after {after}")]
    UnexpectedAfter { line: usize, after: String },
    /// Content before the first directive.
    #[error("patch line {line}: content before any `***` directive")]
    ContentBeforeDirective { line: usize },
    /// A `*** Delete Lines` or `*** Move Lines` whose address is not `PATH:A-B` with 1 <= A <= B.
    #[error("patch line {line}: expected `*** {directive}: PATH:A-B` with 1 <= A <= B")]
    BadLineRange { line: usize, directive: String },
    /// A `*** Expect:` that does not follow a `*** Delete Lines` or `*** Move Lines` directly.
    #[error(
        "patch line {line}: `*** Expect:` must come on the line right after `*** Delete Lines:` or `*** Move Lines:`"
    )]
    ExpectWithoutLines { line: usize },
    /// A line directive without the `*** Expect:` that names its first line.
    #[error("{path}:{start}-{end}: `*** {directive}:` needs `*** Expect:` on the line after it")]
    LinesNoExpect {
        path: String,
        start: usize,
        end: usize,
        directive: &'static str,
    },
    /// A `*** Move Lines` without `*** After:`.
    #[error("{path}:{start}-{end}: `*** Move Lines` needs `*** After:`")]
    MoveLinesNoDestination {
        path: String,
        start: usize,
        end: usize,
    },
    /// A second `*** After:` under one `*** Move Lines`.
    #[error("patch line {line}: `*** Move Lines` takes one `*** After:`")]
    MoveLinesDestinationTwice { line: usize },

    /// `*** Add File` for a file that exists.
    #[error("{path}: already exists")]
    AlreadyExists { path: String },
    #[error("{path}: not valid UTF-8")]
    NotUtf8 { path: String },
    #[error("{path}: cannot read")]
    Read {
        path: String,
        #[source]
        source: io::Error,
    },
    #[error("{path}: not loaded")]
    NotLoaded { path: String },
    #[error("{path}: file not found")]
    NotFound { path: String },
    /// The result would break the file's syntax. `snippet` is the lines around `line`.
    #[error("patch would break the syntax of {path} near line {line}")]
    BrokenSyntax {
        path: String,
        line: usize,
        snippet: Vec<String>,
    },
    /// `*** Replace Symbol` without `+` lines.
    #[error("{path}#{symbol}: Replace Symbol needs `+` lines")]
    ReplaceNeedsLines { path: String, symbol: String },
    /// An insert past the end of the file. `len` is the file's line count.
    #[error("{path}:{line}: the file has {len} lines at this point of the patch")]
    InsertPastEnd {
        path: String,
        line: usize,
        len: usize,
    },
    /// Lines of a `*** Delete Lines` or `*** Move Lines` past the file's last line.
    #[error("{path}:{start}-{end}: the file has {len} lines")]
    RangePastEnd {
        path: String,
        start: usize,
        end: usize,
        len: usize,
    },
    /// The `*** Expect:` text is not the line the range starts with, so the file has changed.
    #[error("{path}:{line}: `*** Expect:` is `{expected}`, but line {line} is `{actual}`")]
    ExpectMismatch {
        path: String,
        line: usize,
        expected: String,
        actual: String,
    },
    /// Two line directives of one patch that cut the same line.
    #[error("{path}: lines {first} and {second} overlap in one patch")]
    LinesOverlap {
        path: String,
        first: String,
        second: String,
    },
    /// A `*** Move Lines` whose `*** After:` line is one of the lines it moves.
    #[error("{path}: `*** After:` line {line} is inside the moved lines {start}-{end}")]
    LineInsideMove {
        path: String,
        line: usize,
        start: usize,
        end: usize,
    },
    /// A move or copy onto a file that exists.
    #[error("{to}: already exists, refusing to {action} {from} over it")]
    Overwrite {
        to: String,
        action: &'static str,
        from: String,
    },
    /// A file that cannot be prepared for writing. `step` says which step failed.
    #[error("{path}: cannot {step}")]
    Commit {
        path: String,
        step: &'static str,
        #[source]
        source: io::Error,
    },
    /// A file that cannot be renamed into place. `done` are the files already written.
    #[error("{path}: rename failed: {error}")]
    Rename {
        path: String,
        error: io::Error,
        done: Vec<String>,
    },
    /// A file that cannot be deleted. `done` are the files already written.
    #[error("{path}: delete failed: {error}")]
    Delete {
        path: String,
        error: io::Error,
        done: Vec<String>,
    },
    /// A symbol that more than one symbol of the file has. `candidates` describe them.
    #[error("{path}#{symbol} is ambiguous: {}", .candidates.join(", "))]
    AmbiguousSymbol {
        path: String,
        symbol: String,
        candidates: Vec<String>,
    },
    #[error("{path}: no symbol `{symbol}`")]
    NoSymbol { path: String, symbol: String },
    /// A name that several symbols share, found by the text of the file.
    #[error("{path}: `{symbol}` is ambiguous")]
    AmbiguousName {
        path: String,
        symbol: String,
        candidates: Vec<String>,
    },
    #[error("{path}: no syntax support")]
    NoSyntax { path: String },
    /// A symbol moved into a file of another language family.
    #[error("{path}#{symbol}: cannot move {from} code into {dest} ({to})")]
    MoveAcrossLanguages {
        path: String,
        symbol: String,
        from: &'static str,
        dest: String,
        to: &'static str,
    },
    /// A symbol moved next to itself or into its own body.
    #[error("{path}#{symbol}: cannot move a symbol next to itself or into its own body")]
    MoveIntoItself { path: String, symbol: String },
    /// `*** Move Symbol` without a destination, from a patch built by hand.
    #[error("{symbol}: MoveSymbol needs a destination")]
    MoveWithoutDestination { symbol: String },
    /// `*** Copy File` without a destination, from a patch built by hand.
    #[error("{from}: copy needs a destination")]
    CopyWithoutDestination { from: String },
    /// A `*** Replace All` pattern that does not compile. `block` counts from 1.
    #[error("Replace All block {block}: {error}")]
    BadRegex { block: usize, error: regex::Error },
    /// A `*** Replace All` block that matches nothing. `regex` tells its kind.
    #[error("Replace All block {block}: `{search}` matches nothing in {paths}")]
    ReplaceNoMatch {
        block: usize,
        search: String,
        paths: String,
        regex: bool,
    },
    /// A SEARCH block that matches nothing. `nearest` is the closest window of the file.
    #[error("{path}: block {block}: SEARCH not found, first line `{first}`")]
    SearchNotFound {
        path: String,
        block: usize,
        first: String,
        nearest: String,
    },
    /// A SEARCH block that matches more than once. `lines` are where it matches, 1-based.
    #[error(
        "{path}: block {block}: SEARCH matches {} times (lines {}); add surrounding lines to make it unique, or use *** Replace All: {path}",
        .lines.len(),
        join_lines(.lines)
    )]
    SearchAmbiguous {
        path: String,
        block: usize,
        lines: Vec<usize>,
    },
}

fn join_lines(lines: &[usize]) -> String {
    lines
        .iter()
        .map(usize::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

impl Hint for EditError {
    fn hint(&self) -> Option<Cow<'_, str>> {
        let text = match self {
            Self::UnknownDirective { hint, .. } => Cow::Borrowed(hint.as_str()),
            Self::UnterminatedBlock => Cow::Borrowed("close it with `>>>>>>> REPLACE`"),
            Self::MoveSymbolNoDestination { .. } => Cow::Borrowed(
                "follow it with `*** Before: path#Sym`, `*** After: path#Sym` or `*** To: path`",
            ),
            Self::CopyFileNoDestination { .. } => Cow::Borrowed("follow it with `*** To: path`"),
            Self::InsertNoLines { .. } => {
                Cow::Borrowed("add the lines to insert, each starting with `+`")
            }
            Self::EmptyPatch => Cow::Borrowed("start with `*** Update File: PATH`"),
            Self::IdenticalBlock { .. } => {
                Cow::Borrowed("change the REPLACE lines, or drop the block")
            }
            Self::EmptySearch { .. } => Cow::Borrowed(
                "to add new lines use `*** Insert After: PATH#Symbol` or `*** Insert After: PATH:LINE`, or `*** Add File: PATH` for a new file",
            ),
            Self::RegexUnderUpdate { .. } => Cow::Borrowed(
                "use `<<<<<<< SEARCH` here, or move the block under `*** Replace All: PATH [PATH...]`",
            ),
            Self::HunkLines { .. } | Self::ExpectedSearch { .. } => Cow::Borrowed(UPDATE_HINT),
            Self::ContentBeforeDirective { .. } => {
                Cow::Borrowed("start with `*** Update File: PATH`")
            }
            Self::AlreadyExists { path } => Cow::Owned(format!("use `*** Update File: {path}`")),
            Self::BrokenSyntax { snippet, .. } => Cow::Owned(format!(
                "fix the block, or pass --allow-broken to write anyway\n{}",
                snippet.join("\n")
            )),
            Self::ReplaceNeedsLines { path, symbol } => Cow::Owned(format!(
                "to remove it use `*** Delete Symbol: {path}#{symbol}`"
            )),
            Self::BadLineRange { .. } => {
                Cow::Borrowed("line numbers are 1-based, as `acus show PATH:A-B` prints them")
            }
            Self::LinesNoExpect { .. } => Cow::Borrowed(
                "on the next line write `*** Expect: ` and the exact text of the range's first line",
            ),
            Self::MoveLinesNoDestination { .. } => Cow::Borrowed(
                "follow it with `*** After: PATH:N` (0 for the file start) or `*** After: PATH#Sym`",
            ),
            Self::RangePastEnd { path, .. } | Self::ExpectMismatch { path, .. } => {
                Cow::Owned(format!("re-read the lines with `acus show {path}`"))
            }
            Self::LinesOverlap { .. } => {
                Cow::Borrowed("each line may be deleted or moved once; merge the two ranges")
            }
            Self::LineInsideMove { .. } => {
                Cow::Borrowed("move the lines after a line outside the moved range")
            }
            Self::InsertPastEnd { path, .. } => Cow::Owned(format!("acus show {path}")),
            Self::Rename { done, .. } | Self::Delete { done, .. } if !done.is_empty() => {
                Cow::Owned(format!("already written: {}", done.join(", ")))
            }
            Self::AmbiguousSymbol { path, .. } => Cow::Owned(format!(
                "edit these lines with `*** Update File: {path}` SEARCH blocks"
            )),
            Self::NoSymbol { path, .. } => Cow::Owned(format!("acus outline {path}")),
            Self::AmbiguousName { candidates, .. } => Cow::Owned(format!(
                "{}; or edit the text with `*** Update File:` SEARCH blocks",
                candidates.join(", ")
            )),
            Self::NoSyntax { path } => Cow::Owned(format!(
                "use `*** Update File: {path}` with SEARCH/REPLACE blocks"
            )),
            Self::BadRegex { .. } => {
                Cow::Borrowed("escape ( [ { . with \\, or use a SEARCH block for literal text")
            }
            Self::ReplaceNoMatch { regex: true, .. } => Cow::Borrowed(
                "the regex runs per file with (?m); `.` does not cross lines, use \\n or (?s)",
            ),
            Self::ReplaceNoMatch { .. } => Cow::Borrowed(
                "search text is literal and whitespace-exact; use `<<<<<<< REGEX` for patterns",
            ),
            Self::SearchNotFound { nearest, .. } => Cow::Owned(format!(
                "copy the text as the file has it, a part of a line is fine; re-read it with acus show{nearest}"
            )),
            _ => return None,
        };
        Some(text)
    }
}
