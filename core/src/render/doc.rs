// SPDX-License-Identifier: MPL-2.0
//! Owns the blocks a result builds: titles, sections, rows, marks, tables and hints.
//! Their layout lives in `text`.

use std::io::{self, Write};

use super::check::{Check, Status};
use super::paint::Tone;
use super::text;

/// The document a `Render` value fills in: an ordered list of blocks.
#[derive(Default)]
pub struct Doc {
    blocks: Vec<Block>,
}

/// How a view is written: whether colour codes are used, and whether the rich layout
/// (bars, glyphs, padding) is used.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Profile {
    pub color: bool,
    pub rich: bool,
}

impl Profile {
    /// The agent view: plain text with no padding, bars or colour.
    pub const AGENT: Self = Self {
        color: false,
        rich: false,
    };
}

/// One block of a Doc.
pub(super) enum Block {
    Title(String),
    Section(String),
    Line(String),
    Hint(String),
    More { rest: usize, command: String },
    Marks(Vec<Mark>),
    Row(Row),
    Table(Table),
}

impl Block {
    pub(super) fn is_row(&self) -> bool {
        matches!(self, Self::Row(_))
    }
}

/// A labelled line of cells. Consecutive rows form an alignment group.
pub struct Row {
    pub(super) label: String,
    pub(super) mark: Option<Status>,
    pub(super) cells: Vec<Cell>,
    pub(super) hint: Option<String>,
}

impl Row {
    pub(super) fn new(label: String, mark: Option<Status>) -> Self {
        Self {
            label,
            mark,
            cells: Vec::new(),
            hint: None,
        }
    }

    /// Adds a plain text cell.
    pub fn text(&mut self, text: impl Into<String>) -> &mut Self {
        self.push(text.into(), Tone::Plain)
    }

    /// Adds a text cell in `tone`.
    pub fn tone(&mut self, text: impl Into<String>, tone: Tone) -> &mut Self {
        self.push(text.into(), tone)
    }

    /// Adds a dimmed text cell.
    pub fn dim(&mut self, text: impl Into<String>) -> &mut Self {
        self.push(text.into(), Tone::Dim)
    }

    /// Adds a bar for `ratio`, from 0 to 1, filled in the tone of the ratio.
    pub fn bar(&mut self, ratio: f64) -> &mut Self {
        self.cells.push(Cell::Bar(ratio));
        self
    }

    fn push(&mut self, text: String, tone: Tone) -> &mut Self {
        self.cells.push(Cell::Text(text, tone));
        self
    }
}

/// One cell of a row: text in a tone, or a bar.
pub enum Cell {
    Text(String, Tone),
    Bar(f64),
}

/// A status mark with its text, such as `✓ hook registered`.
pub struct Mark {
    pub status: Status,
    pub text: String,
}

/// Column headings, alignment and rows of a table.
pub struct Table {
    pub head: Vec<String>,
    pub align: Vec<Align>,
    pub rows: Vec<Vec<String>>,
}

/// How a table column aligns its cells.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Align {
    Left,
    Right,
}

impl Doc {
    /// Adds the title, at column 0.
    pub fn title(&mut self, title: impl Into<String>) {
        self.blocks.push(Block::Title(title.into()));
    }

    /// Adds a section header, at column 0.
    pub fn section(&mut self, name: impl Into<String>) {
        self.blocks.push(Block::Section(name.into()));
    }

    /// Adds an indented line of plain text.
    pub fn line(&mut self, text: impl Into<String>) {
        self.blocks.push(Block::Line(text.into()));
    }

    /// Adds a hint, which tells the user how to fix something.
    pub fn hint(&mut self, text: impl Into<String>) {
        self.blocks.push(Block::Hint(text.into()));
    }

    /// Adds the note that `rest` more items follow, and the command that shows them.
    pub fn more(&mut self, rest: usize, command: &str) {
        self.blocks.push(Block::More {
            rest,
            command: command.to_owned(),
        });
    }

    /// Adds a row with `label` and returns it so cells can be added to it.
    pub fn row(&mut self, label: impl Into<String>) -> &mut Row {
        self.push_row(Row::new(label.into(), None))
    }

    /// Adds one line of status marks.
    pub fn marks(&mut self, marks: impl IntoIterator<Item = Mark>) {
        self.blocks.push(Block::Marks(marks.into_iter().collect()));
    }

    /// Adds a doctor check as a marked row, with its hint under it when it has one.
    pub fn check(&mut self, check: &Check) {
        let mut row = Row::new(check.name.to_string(), Some(check.status));
        if !check.detail.is_empty() {
            row.text(check.detail.clone());
        }
        row.hint = check.hint.clone();
        self.push_row(row);
    }

    /// Adds a table.
    pub fn table(&mut self, table: Table) {
        self.blocks.push(Block::Table(table));
    }

    /// Writes the document in `profile`.
    pub fn write(&self, out: &mut dyn Write, profile: Profile) -> io::Result<()> {
        text::write(&self.blocks, out, profile)
    }

    fn push_row(&mut self, row: Row) -> &mut Row {
        self.blocks.push(Block::Row(row));
        match self.blocks.last_mut() {
            Some(Block::Row(row)) => row,
            _ => unreachable!("a row was pushed last"),
        }
    }
}
