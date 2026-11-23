// SPDX-License-Identifier: MPL-2.0
//! Splits a `batch` line into acus arguments and quotes them back for its header and footers.

use anyhow::Result;

/// The line's command with shell quoting, so it can be pasted back.
pub(super) fn shown_args(args: &[String]) -> String {
    args.iter().map(|a| quote(a)).collect::<Vec<_>>().join(" ")
}

/// Quotes `a` in single quotes unless every character is one a shell leaves alone.
fn quote(a: &str) -> String {
    let safe = !a.is_empty()
        && a.chars()
            .all(|c| c.is_ascii_alphanumeric() || "_./:#=@%+,-".contains(c));
    if safe {
        a.to_owned()
    } else {
        format!("\x27{}\x27", a.replace('\x27', "\x27\\\x27\x27"))
    }
}

/// Splits a command line like a shell would for plain words and '…'/"…" quotes; drops a
/// leading `acus` and `#` comments. Backslashes are literal outside double quotes (Windows
/// paths).
pub(super) fn split(line: &str) -> Result<Vec<String>> {
    let (mut args, mut cur, mut word) = (Vec::new(), String::new(), false);
    let mut chars = line.trim().chars();
    while let Some(c) = chars.next() {
        match c {
            '#' if !word => break,
            c if c.is_whitespace() => {
                if word {
                    args.push(std::mem::take(&mut cur));
                    word = false;
                }
            }
            '\x27' | '"' => {
                word = true;
                loop {
                    match chars.next() {
                        Some(q) if q == c => break,
                        Some('\x5c') if c == '"' => cur.extend(chars.next()),
                        Some(x) => cur.push(x),
                        None => anyhow::bail!("unclosed {c} in `{line}`"),
                    }
                }
            }
            c => {
                word = true;
                cur.push(c);
            }
        }
    }
    if word {
        args.push(cur);
    }
    if args.first().is_some_and(|a| a == "acus") {
        args.remove(0);
    }
    Ok(args)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_command_lines() {
        let s = |l| split(l).unwrap();
        assert_eq!(
            s("acus find 'fn (a|b)' --block # why"),
            ["find", "fn (a|b)", "--block"]
        );
        assert_eq!(
            s(r#"show "a b.rs#f" src\x.rs"#),
            ["show", "a b.rs#f", r"src\x.rs"]
        );
        assert_eq!(s("show a.rs#f"), ["show", "a.rs#f"]);
        assert!(s("  # only a comment").is_empty());
        assert!(split("find 'x").is_err());
    }

    #[test]
    fn quotes_arguments_a_shell_would_split() {
        let args = ["find", "foo|bar", "src", "-g", "*.rs", "$x", "a b", "x\"y"].map(str::to_owned);
        assert_eq!(
            shown_args(&args),
            r#"find 'foo|bar' src -g '*.rs' '$x' 'a b' 'x"y'"#
        );
    }
}
