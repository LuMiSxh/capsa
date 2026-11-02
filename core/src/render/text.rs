// SPDX-License-Identifier: MPL-2.0
//! Lays the blocks of a Doc out as text: indentation, alignment groups, bars and tables.
//! Widths count characters; there is no terminal width lookup and no wrapping.

use std::io::{self, Write};

use super::check::Status;
use super::doc::{Align, Block, Cell, Mark, Profile, Row, Table};
use super::paint::{Tone, bold, paint};

const BAR_WIDTH: usize = 16;
const INDENT: &str = "  ";
const GAP: &str = "  ";
/// Columns between the widest label and the first cell.
const LABEL_GAP: usize = 5;

/// Writes the blocks in `profile`. Consecutive rows form one alignment group.
pub(super) fn write(blocks: &[Block], out: &mut dyn Write, profile: Profile) -> io::Result<()> {
    let mut last: Option<&Block> = None;
    for group in blocks.chunk_by(|a, b| a.is_row() && b.is_row()) {
        if let Some(prev) = last
            && gap_before(prev, &group[0])
        {
            writeln!(out)?;
        }
        if group[0].is_row() {
            write_rows(group, out, profile)?;
        } else {
            write_block(&group[0], out, profile)?;
        }
        last = group.last();
    }
    Ok(())
}

/// A blank line separates a title from what follows, a section from the block before it,
/// and a row group from marks or a table.
fn gap_before(prev: &Block, next: &Block) -> bool {
    matches!(prev, Block::Title(_))
        || matches!(next, Block::Section(_))
        || (prev.is_row() && matches!(next, Block::Marks(_) | Block::Table(_)))
}

fn write_rows(group: &[Block], out: &mut dyn Write, profile: Profile) -> io::Result<()> {
    let rows: Vec<&Row> = group
        .iter()
        .filter_map(|block| match block {
            Block::Row(row) => Some(row),
            _ => None,
        })
        .collect();
    if profile.rich {
        rich_rows(&rows, out, profile.color)
    } else {
        plain_rows(&rows, out)
    }
}

/// Lays out one alignment group: labels padded to the widest, then each cell column.
fn rich_rows(rows: &[&Row], out: &mut dyn Write, color: bool) -> io::Result<()> {
    let label_width = rows.iter().map(|row| chars(&row.label)).max().unwrap_or(0);
    let marked = rows.iter().any(|row| row.mark.is_some());
    let prefix = if marked { 2 } else { 0 };
    let widths = column_widths(rows);
    for row in rows {
        let mut line = String::from(INDENT);
        match row.mark {
            Some(status) => {
                line.push_str(&glyph(status, color));
                line.push(' ');
            }
            None if marked => line.push_str("  "),
            None => {}
        }
        line.push_str(&row.label);
        let used = prefix + chars(&row.label);
        pad(
            &mut line,
            (prefix + label_width + LABEL_GAP).saturating_sub(used),
        );
        push_cells(&mut line, &row.cells, &widths, color);
        writeln!(out, "{}", line.trim_end())?;
        if let Some(hint) = &row.hint {
            writeln!(
                out,
                "    {}",
                paint(&format!("hint: {hint}"), Tone::Accent, color)
            )?;
        }
    }
    Ok(())
}

/// Width of each cell column: a bar column is at least `BAR_WIDTH`, a text column is one wider
/// than its widest text.
fn column_widths(rows: &[&Row]) -> Vec<usize> {
    let mut columns: Vec<(usize, bool)> = Vec::new();
    for row in rows {
        for (index, cell) in row.cells.iter().enumerate() {
            if columns.len() <= index {
                columns.resize(index + 1, (0, false));
            }
            let column = &mut columns[index];
            match cell {
                Cell::Text(text, _) => column.0 = column.0.max(chars(text)),
                Cell::Bar(_) => column.1 = true,
            }
        }
    }
    columns
        .into_iter()
        .map(|(widest, bar)| {
            if bar {
                widest.max(BAR_WIDTH)
            } else {
                widest + 1
            }
        })
        .collect()
}

fn push_cells(line: &mut String, cells: &[Cell], widths: &[usize], color: bool) {
    let last = cells.len().saturating_sub(1);
    for (index, cell) in cells.iter().enumerate() {
        let (painted, width) = match cell {
            Cell::Text(text, tone) => (paint(text, *tone, color).into_owned(), chars(text)),
            Cell::Bar(ratio) => (bar(*ratio, color), BAR_WIDTH),
        };
        line.push_str(&painted);
        if index < last {
            pad(line, widths[index].saturating_sub(width));
            line.push_str(GAP);
        }
    }
}

/// Renders a bar: the filled part in the tone of the ratio, the rest plain.
fn bar(ratio: f64, color: bool) -> String {
    let ratio = if ratio.is_nan() {
        0.0
    } else {
        ratio.clamp(0.0, 1.0)
    };
    let filled = (ratio * BAR_WIDTH as f64).round() as usize;
    format!(
        "{}{}",
        paint(&"█".repeat(filled), Tone::for_ratio(ratio), color),
        "░".repeat(BAR_WIDTH - filled)
    )
}

/// Agent text: `label: cell, cell`, with the mark word in front when the row has a mark.
fn plain_rows(rows: &[&Row], out: &mut dyn Write) -> io::Result<()> {
    for row in rows {
        let mut line = match row.mark {
            Some(status) => format!("{}: {}", word(status), row.label),
            None => row.label.clone(),
        };
        let cells: Vec<&str> = row
            .cells
            .iter()
            .filter_map(|cell| match cell {
                Cell::Text(text, _) => Some(text.as_str()),
                Cell::Bar(_) => None,
            })
            .collect();
        if !cells.is_empty() {
            line.push_str(": ");
            line.push_str(&cells.join(", "));
        }
        writeln!(out, "{line}")?;
        if let Some(hint) = &row.hint {
            writeln!(out, "hint: {hint}")?;
        }
    }
    Ok(())
}

fn write_block(block: &Block, out: &mut dyn Write, profile: Profile) -> io::Result<()> {
    let color = profile.color;
    let indent = if profile.rich { INDENT } else { "" };
    match block {
        Block::Title(title) => writeln!(out, "{}", bold(title, color)),
        Block::Section(name) => writeln!(out, "{}", bold(name, color)),
        Block::Line(text) => writeln!(out, "{indent}{text}"),
        Block::Hint(text) => {
            writeln!(
                out,
                "{indent}{}",
                paint(&format!("hint: {text}"), Tone::Accent, color)
            )
        }
        Block::More { rest, command } => writeln!(
            out,
            "{indent}{}",
            paint(&format!("… {rest} more; run {command}"), Tone::Dim, color)
        ),
        Block::Marks(marks) => write_marks(marks, out, profile),
        Block::Table(table) => write_table(table, out, profile),
        Block::Row(_) => Ok(()),
    }
}

fn write_marks(marks: &[Mark], out: &mut dyn Write, profile: Profile) -> io::Result<()> {
    if profile.rich {
        let parts: Vec<String> = marks
            .iter()
            .map(|mark| format!("{} {}", glyph(mark.status, profile.color), mark.text))
            .collect();
        writeln!(out, "{INDENT}{}", parts.join("   "))
    } else {
        for mark in marks {
            writeln!(out, "{}: {}", word(mark.status), mark.text)?;
        }
        Ok(())
    }
}

fn write_table(table: &Table, out: &mut dyn Write, profile: Profile) -> io::Result<()> {
    if !profile.rich {
        writeln!(out, "{}", table.head.join(", "))?;
        for row in &table.rows {
            writeln!(out, "{}", row.join(", "))?;
        }
        return Ok(());
    }
    let widths: Vec<usize> = table
        .head
        .iter()
        .enumerate()
        .map(|(index, head)| {
            table
                .rows
                .iter()
                .filter_map(|row| row.get(index))
                .map(|cell| chars(cell))
                .fold(chars(head), usize::max)
        })
        .collect();
    let head = table_line(&table.head, &widths, &table.align);
    writeln!(out, "{}", paint(&head, Tone::Dim, profile.color))?;
    for row in &table.rows {
        writeln!(out, "{}", table_line(row, &widths, &table.align))?;
    }
    Ok(())
}

/// One table line: every column padded to its width, right-aligned where `align` says so.
fn table_line(cells: &[String], widths: &[usize], align: &[Align]) -> String {
    let mut line = String::from(INDENT);
    for (index, width) in widths.iter().enumerate() {
        let cell = cells.get(index).map_or("", String::as_str);
        let spacing = " ".repeat(width.saturating_sub(chars(cell)));
        if align.get(index) == Some(&Align::Right) {
            line.push_str(&spacing);
            line.push_str(cell);
        } else {
            line.push_str(cell);
            line.push_str(&spacing);
        }
        if index + 1 < widths.len() {
            line.push_str(GAP);
        }
    }
    line.trim_end().to_owned()
}

fn glyph(status: Status, color: bool) -> String {
    let (symbol, tone) = match status {
        Status::Ok => ("✓", Tone::Ok),
        Status::Warn => ("!", Tone::Warn),
        Status::Fail => ("✗", Tone::Fail),
    };
    paint(symbol, tone, color).into_owned()
}

fn word(status: Status) -> &'static str {
    match status {
        Status::Ok => "ok",
        Status::Warn => "warn",
        Status::Fail => "fail",
    }
}

fn pad(line: &mut String, count: usize) {
    line.extend(std::iter::repeat_n(' ', count));
}

fn chars(text: &str) -> usize {
    text.chars().count()
}
