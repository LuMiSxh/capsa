// SPDX-License-Identifier: MPL-2.0
//! Prints the planned tree of `acus map`: a row per directory, then its file rows.

use super::{Dir, ENTRY, FILE_NAMES, File, SYMBOLS};

pub(super) fn render(d: &Dir, level: usize, out: &mut Vec<String>) {
    out.extend(file_rows(d, level, d.detail));
    let pad = "  ".repeat(level - 1);
    for (name, c) in &d.dirs {
        out.push(dir_row(&pad, name, c));
        if c.open {
            render(c, level + 1, out);
        }
    }
}

pub(super) fn dir_row(pad: &str, name: &str, d: &Dir) -> String {
    format!("{pad}{name}/{}  {}", tags(d), summary(d))
}

/// One line of file names, or with `detail` one line per file with its top-level symbols;
/// entry points first, then the largest.
pub(super) fn file_rows(d: &Dir, level: usize, detail: bool) -> Vec<String> {
    let pad = "  ".repeat(level - 1);
    let mut files: Vec<&File> = d.files.iter().collect();
    files.sort_by_key(|f| (!is_entry(&f.name), std::cmp::Reverse(f.lines)));
    if detail {
        files
            .iter()
            .map(|f| {
                let mut row = format!("{pad}{} {}", f.name, num(f.lines));
                if !f.symbols.is_empty() {
                    row += &format!(": {}", capped(&f.symbols, SYMBOLS));
                }
                row
            })
            .collect()
    } else if files.is_empty() {
        vec![]
    } else {
        let names: Vec<String> = files
            .iter()
            .map(|f| format!("{} {}", f.name, num(f.lines)))
            .collect();
        vec![format!("{pad}{}", capped(&names, FILE_NAMES))]
    }
}

pub(super) fn has_closed(d: &Dir) -> bool {
    d.dirs.values().any(|c| !c.open || has_closed(c))
}

fn is_entry(name: &str) -> bool {
    let base = name.rsplit('/').next().unwrap_or(name);
    let stem = base.split('.').next().unwrap_or(base);
    ENTRY.iter().any(|e| e.eq_ignore_ascii_case(stem))
}

pub(super) fn tags(d: &Dir) -> String {
    if d.tags.is_empty() {
        String::new()
    } else {
        format!(
            " [{}]",
            d.tags.iter().copied().collect::<Vec<_>>().join(", ")
        )
    }
}

/// `14 files, 5.2k lines (rs, md)`: the three extensions with the most lines.
pub(super) fn summary(d: &Dir) -> String {
    let mut exts: Vec<_> = d.exts.iter().collect();
    exts.sort_by_key(|(_, l)| std::cmp::Reverse(**l));
    let exts: Vec<_> = exts.iter().take(3).map(|(e, _)| e.as_str()).collect();
    let files = if d.n == 1 { "file" } else { "files" };
    let mut s = format!("{} {files}, {} lines", d.n, num(d.lines));
    if !exts.is_empty() {
        s += &format!(" ({})", exts.join(", "));
    }
    s
}

fn capped(items: &[String], cap: usize) -> String {
    let mut s = items[..items.len().min(cap)].join(", ");
    if items.len() > cap {
        s += &format!(" +{}", items.len() - cap);
    }
    s
}

fn num(n: usize) -> String {
    if n >= 1000 {
        format!("{:.1}k", n as f64 / 1000.0)
    } else {
        n.to_string()
    }
}
