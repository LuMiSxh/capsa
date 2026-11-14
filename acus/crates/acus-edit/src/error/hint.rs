// SPDX-License-Identifier: MPL-2.0
//! The fix shown under each patch failure, as `hint:` on the line after the error.

use super::{EditError, UPDATE_HINT};
use capsa_core::hint::Hint;
use std::borrow::Cow;

impl Hint for EditError {
    fn hint(&self) -> Option<Cow<'_, str>> {
        if let Some(text) = self.conflict_hint() {
            return Some(text);
        }
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
            Self::ResolveOverlap { .. } => Cow::Borrowed(
                "put them in separate patches: both count lines from the file before the patch",
            ),
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
            Self::AppendMissing { path } => Cow::Owned(format!(
                "use `*** Add File: {path}` to create it, or `*** Update File: {path}` to edit it"
            )),
            Self::Many { .. } => {
                Cow::Borrowed("fix each failing block from its message and resend the whole patch")
            }
            _ => return None,
        };
        Some(text)
    }
}

impl EditError {
    /// The fix for a failure of `*** Resolve Conflict`.
    fn conflict_hint(&self) -> Option<Cow<'_, str>> {
        Some(match self {
            Self::ResolveBadMode { .. } => Cow::Borrowed(
                "put the mode after the path: `--ours`, `--theirs`, `--both` or `--base`",
            ),
            Self::ResolveBadLine { .. } => {
                Cow::Borrowed("line numbers are 1-based, as `acus show PATH:A-B` prints them")
            }
            Self::NoConflict { .. } => {
                Cow::Borrowed("resolve the file with `*** Update File:` SEARCH/REPLACE blocks")
            }
            Self::UnterminatedConflict { .. } => Cow::Borrowed(
                "add the missing `=======` and `>>>>>>>` markers, or edit the file by hand",
            ),
            Self::NoConflictAtLine { path, .. } => Cow::Owned(format!("acus show {path}")),
            Self::ConflictAlreadyResolved { .. } => {
                Cow::Borrowed("use one directive per conflict section")
            }
            Self::NoConflictBase { .. } => {
                Cow::Borrowed("use `--ours`, `--theirs` or `--both` for a two-way conflict")
            }
            _ => return None,
        })
    }
}
