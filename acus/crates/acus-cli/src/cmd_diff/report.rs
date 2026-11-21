// SPDX-License-Identifier: MPL-2.0
//! The data `acus diff` computes and prints: changed files, symbol groups, moves and conflicts.

use super::FileDiff;
use super::moves::Move;
use super::status::Status;
use serde::Serialize;

/// Label for changed lines outside any symbol.
pub(crate) const TOP: &str = "(top level)";

/// What `acus diff` computed: changes against a revision or the working tree, or unmerged paths.
#[derive(Debug, Serialize)]
pub(crate) enum DiffReport {
    Changes(Changes),
    Conflicts(Conflicts),
}

/// The changed files of a diff, with the symbol groups and moves the views print.
#[derive(Debug, Serialize)]
pub(crate) struct Changes {
    /// The working tree's status; None when a revision is given.
    pub(crate) header: Option<Status>,
    /// The revision argument, which the per-file rerun hint repeats.
    pub(crate) rev: Option<String>,
    /// Staged changes that the default view names in a footer.
    pub(crate) staged: Option<Staged>,
    pub(crate) files: Vec<FileReport>,
    /// Symbols removed in one place and added in another; groups refer to them by index.
    pub(crate) moves: Vec<Move>,
}

/// Staged changes counted for the footer line.
#[derive(Debug, Serialize)]
pub(crate) struct Staged {
    pub(crate) files: usize,
    pub(crate) add: usize,
    pub(crate) del: usize,
}

/// One changed file with the derived views its output needs.
#[derive(Debug, Serialize)]
pub(crate) struct FileReport {
    pub(crate) diff: FileDiff,
    /// Changed lines grouped by enclosing symbol, without moves: the JSON view.
    pub(crate) symbols: Vec<Group>,
    /// The same groups with moved symbols split out: the text view of a modified file.
    pub(crate) text_groups: Vec<Group>,
    /// The text view of an added file under `-p`; None for every other file.
    pub(crate) new_file: Option<NewFile>,
}

/// Changed lines inside one symbol, or outside any symbol.
#[derive(Debug, Serialize)]
pub(crate) struct Group {
    pub(crate) label: String,
    /// The symbol's line range in the new file; None when it is gone.
    pub(crate) range: Option<(usize, usize)>,
    pub(crate) add: usize,
    pub(crate) del: usize,
    /// (added, line number on that side, text).
    pub(crate) lines: Vec<(bool, usize, String)>,
    /// The move this group belongs to and whether it is the destination side.
    pub(crate) moved: Option<(usize, bool)>,
}

/// The text view of an added file: its outline, or its first lines when it has none.
#[derive(Debug, Serialize)]
pub(crate) enum NewFile {
    Outline(Vec<OutlineRow>),
    Lines(Vec<(bool, usize, String)>),
}

/// One symbol of a new file's outline, with the move index when it arrived from elsewhere.
#[derive(Debug, Serialize)]
pub(crate) struct OutlineRow {
    pub(crate) depth: usize,
    pub(crate) kind: &'static str,
    pub(crate) text: String,
    pub(crate) start: usize,
    pub(crate) end: usize,
    pub(crate) moved: Option<usize>,
}

/// The unmerged paths of the working tree, with the line ranges of their conflict markers.
#[derive(Debug, Serialize)]
pub(crate) struct Conflicts {
    pub(crate) header: Option<Status>,
    pub(crate) unmerged: Vec<Conflict>,
}

/// One unmerged path and the (start, end) lines of its conflict markers.
#[derive(Debug, Serialize)]
pub(crate) struct Conflict {
    pub(crate) path: String,
    pub(crate) ranges: Vec<(usize, usize)>,
}
