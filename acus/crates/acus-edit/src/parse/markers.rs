// SPDX-License-Identifier: MPL-2.0
//! Finds git conflict markers in file text, apart from a patch's own `SEARCH`/`REPLACE` markers.

/// Whether `line` opens a conflict section, such as `<<<<<<< HEAD`. The patch's own bare
/// `<<<<<<<` and `<<<<<<< SEARCH` are block markers, not ones.
pub(crate) fn is_open(line: &str) -> bool {
    line.strip_prefix("<<<<<<<").is_some_and(|label| {
        let label = label.trim();
        !label.is_empty() && label != "SEARCH"
    })
}

/// Whether `line` closes a conflict section, such as `>>>>>>> branch`. The patch's own bare
/// `>>>>>>>` and `>>>>>>> REPLACE` are block markers, not ones.
pub(crate) fn is_close(line: &str) -> bool {
    line.strip_prefix(">>>>>>>").is_some_and(|label| {
        let label = label.trim();
        !label.is_empty() && label != "REPLACE"
    })
}

/// Whether `line` separates a conflict section's two halves.
pub(crate) fn is_separator(line: &str) -> bool {
    line.trim_end() == "======="
}

/// Whether `line` starts a conflict section's diff3 base half.
pub(crate) fn is_base(line: &str) -> bool {
    line.starts_with("|||||||")
}

/// Whether each line lies inside a balanced conflict section, its markers included. A lone
/// opener or closer is not a section.
pub(crate) fn in_conflict_lines(text: &str) -> Vec<bool> {
    let lines: Vec<&str> = text.lines().collect();
    let mut inside = vec![false; lines.len()];
    let mut i = 0;
    while i < lines.len() {
        if is_open(lines[i])
            && let Some(end) = section_end(&lines, i)
        {
            for slot in &mut inside[i..=end] {
                *slot = true;
            }
            i = end + 1;
            continue;
        }
        i += 1;
    }
    inside
}

/// The index of the closer that ends the section opening at `start`, or `None` when it never
/// closes. The section may carry a diff3 `|||||||` base half.
fn section_end(lines: &[&str], start: usize) -> Option<usize> {
    let mut j = start + 1;
    while j < lines.len() && !is_separator(lines[j]) && !is_close(lines[j]) && !is_base(lines[j]) {
        j += 1;
    }
    if j == lines.len() {
        return None;
    }
    if is_base(lines[j]) {
        j += 1;
        while j < lines.len() && !is_separator(lines[j]) && !is_close(lines[j]) {
            j += 1;
        }
        if j == lines.len() || !is_separator(lines[j]) {
            return None;
        }
    }
    if !is_separator(lines[j]) {
        return None;
    }
    j += 1;
    while j < lines.len() && !is_close(lines[j]) {
        j += 1;
    }
    (j < lines.len()).then_some(j)
}

/// Whether `text` holds a complete conflict section.
pub(crate) fn has_markers(text: &str) -> bool {
    in_conflict_lines(text).contains(&true)
}

/// The 1-based line of the first unmatched conflict marker, or `None` when every opener has its
/// closer. A lone `=======` is not reported: a Markdown heading underlines with it.
pub(crate) fn unbalanced(text: &str) -> Option<usize> {
    let mut open = None;
    for (i, line) in text.lines().enumerate() {
        if is_open(line) {
            open.get_or_insert(i + 1);
        } else if is_close(line) && open.take().is_none() {
            return Some(i + 1);
        }
    }
    open
}

/// The 1-based line of the first conflict opener in `text`.
pub(crate) fn first_marker(text: &str) -> Option<usize> {
    text.lines().position(is_open).map(|i| i + 1)
}
