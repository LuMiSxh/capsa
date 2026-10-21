// SPDX-License-Identifier: MPL-2.0

//! Owns the `acus show` call that replaces a `cat`, `head`, `sed -n` or `Get-Content` read.

use super::*;

/// Returns the `acus show` call for a `cat`, `head`, `sed -n 'A,Bp'` or `Get-Content` read. Paths
/// are written as the user wrote them when the shell expands them, so wildcards and variables keep
/// working.
pub(super) fn show_cmd(head: &str, args: &[&str]) -> Option<String> {
    // Byte counts are not lines: no line range says the same.
    if counts_bytes(head, args) {
        return None;
    }
    // `sed -n 'N,$p' FILE` and `tail -n +N FILE`: from line N to the end of the file.
    if head == "sed"
        && let ["-n", script, f] = args
        && let Some(n) = sed_from(script)
        && operand(f)
    {
        return Some(format!("`acus show {}`", path_addr(f, &format!(":{n}-"))));
    }
    if head == "tail"
        && let Some((n, f)) = tail_from(args)
        && operand(f)
    {
        return Some(format!("`acus show {}`", path_addr(f, &format!(":{n}-"))));
    }
    // The first or last lines of one file, as the exact range.
    if matches!(head, "head" | "tail")
        && let Some((n, f)) = line_count_args(args)
        && operand(f)
    {
        let at = if head == "tail" {
            format!(":-{n}")
        } else {
            format!(":1-{n}")
        };
        return Some(format!("`acus show {}`", path_addr(f, &at)));
    }
    let (mut files, mut first, mut lines): (Vec<&str>, Option<usize>, Vec<(usize, usize)>) =
        (vec![], None, vec![]);
    let mut words = args.iter().copied();
    while let Some(a) = words.next() {
        let lc = a.to_lowercase();
        let count = |v: Option<&str>| v.and_then(|v| v.parse::<usize>().ok());
        match lc.as_str() {
            "-n" if !matches!(head, "sed" | "cat") => {
                let n = count(words.next());
                if head != "tail" {
                    first = first.or(n);
                }
            }
            "-totalcount" | "-head" | "-first" => first = first.or(count(words.next())),
            "-tail" | "-last" | "-encoding" | "-delimiter" => {
                words.next();
            }
            "-path" | "-literalpath" => files.extend(words.next()),
            l if l.starts_with('-') => {
                // `head -20` and `head -n20`.
                if head == "head" {
                    first = first.or(l[1..].trim_start_matches('n').parse().ok());
                }
            }
            // The sed script comes first: its line addresses, or none for a regex.
            _ if head == "sed" && lines.is_empty() => lines = sed_lines(unquote(a))?,
            _ if operand(a) => files.push(a),
            _ => {}
        }
    }
    files.dedup();
    if head == "head" && first.is_none() {
        first = Some(10);
    }
    if head == "sed" && lines.is_empty() {
        return None;
    }
    let [file] = files.as_slice() else {
        // Several files are shown whole, as before; a sed script names the lines of one file.
        return (!files.is_empty() && head != "sed").then(|| {
            let quoted: Vec<String> = files.iter().map(|f| path_addr(f, "")).collect();
            format!("`acus show {}`", quoted.join(" "))
        });
    };
    let suffixes: Vec<String> = if head == "sed" {
        lines
            .iter()
            .map(|&(a, b)| {
                if a == b {
                    format!(":{a}")
                } else {
                    format!(":{a}-{b}")
                }
            })
            .collect()
    } else {
        first.map(|n| format!(":1-{n}")).into_iter().collect()
    };
    if suffixes.is_empty() {
        return Some(format!("`acus show {}`", path_addr(file, "")));
    }
    // A wildcard expands to several files, and a line range cannot follow it.
    if has_glob(file) {
        return None;
    }
    let addrs: Vec<String> = suffixes.iter().map(|s| path_addr(file, s)).collect();
    Some(format!("`acus show {}`", addrs.join(" ")))
}
