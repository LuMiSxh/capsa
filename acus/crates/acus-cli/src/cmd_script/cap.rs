// SPDX-License-Identifier: MPL-2.0
//! Caps what a script shows: the printed lines first, then the return value, which keeps half
//! the budget however much was printed.

/// The most lines and bytes a script shows.
#[derive(Clone, Copy)]
pub(super) struct Cap {
    pub(super) lines: usize,
    pub(super) bytes: usize,
}

struct Fit {
    shown: Vec<String>,
    bytes: usize,
    cut: usize,
}

/// Keeps the leading lines that fit `cap`; the line that crosses the byte limit is shortened.
fn fit(lines: &[String], cap: Cap) -> Fit {
    let mut shown = Vec::new();
    let mut bytes = 0;
    let mut cut = 0;
    for line in lines {
        let left = cap.bytes.saturating_sub(bytes);
        if shown.len() >= cap.lines || left == 0 {
            break;
        }
        if line.len() < left {
            bytes += line.len() + 1;
            shown.push(line.clone());
            continue;
        }
        let end = (0..left).rev().find(|&i| line.is_char_boundary(i));
        shown.push(format!("{}…", &line[..end.unwrap_or(0)]));
        bytes = cap.bytes;
        cut += 1;
        break;
    }
    cut += lines.len().saturating_sub(shown.len());
    Fit { shown, bytes, cut }
}

/// Returns `before` then `last` within `cap`, with a footer when lines were cut. `last` keeps
/// half the budget; `before` fills the rest, and `last` gets what `before` leaves.
pub(super) fn apply(before: &[String], last: &[String], cap: Cap) -> Vec<String> {
    let reserve = fit(
        last,
        Cap {
            lines: cap.lines / 2,
            bytes: cap.bytes / 2,
        },
    );
    let head = fit(
        before,
        Cap {
            lines: cap.lines - reserve.shown.len(),
            bytes: cap.bytes.saturating_sub(reserve.bytes),
        },
    );
    let tail = fit(
        last,
        Cap {
            lines: cap.lines - head.shown.len(),
            bytes: cap.bytes.saturating_sub(head.bytes),
        },
    );
    let cut = head.cut + tail.cut;
    let mut out = head.shown;
    out.extend(tail.shown);
    if cut > 0 {
        out.push(format!(
            "… cut {cut} lines; return less, use .head()/.grep(), or rerun with --max-output N"
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn many(prefix: &str, n: usize) -> Vec<String> {
        (0..n).map(|i| format!("{prefix}{i}")).collect()
    }

    #[test]
    fn return_keeps_half_the_lines_when_prints_fill_the_rest() {
        let cap = Cap {
            lines: 10,
            bytes: 1000,
        };
        let out = apply(&many("p", 50), &many("r", 50), cap);
        assert_eq!(out.iter().filter(|l| l.starts_with('p')).count(), 5);
        assert_eq!(out.iter().filter(|l| l.starts_with('r')).count(), 5);
        assert!(out.last().unwrap().starts_with("… cut 90 lines"));
    }

    #[test]
    fn return_takes_what_prints_leave() {
        let cap = Cap {
            lines: 10,
            bytes: 1000,
        };
        let out = apply(&many("p", 2), &many("r", 50), cap);
        assert_eq!(out.iter().filter(|l| l.starts_with('r')).count(), 8);
    }

    #[test]
    fn a_long_line_is_shortened_to_the_bytes() {
        let cap = Cap {
            lines: 10,
            bytes: 20,
        };
        let out = apply(&[], &["é".repeat(100)], cap);
        assert!(out[0].len() <= 19 + '…'.len_utf8());
        assert!(out[0].ends_with('…'));
    }
}
