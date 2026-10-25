// SPDX-License-Identifier: MPL-2.0
//! Turns agent-supplied text into a single visible line that is safe to show and to log.

/// The longest agent-supplied field (command, URL, one argument) the broker accepts. A longer
/// one is refused, never shortened, so the dialog always shows the whole text.
pub const MAX_FIELD: usize = 2000;

/// The marker that stands for a line break, so a multi-line command cannot pass for one line.
const NEWLINE: char = '\u{21b5}';

/// Returns whether `c` changes how text is laid out or hides it: zero-width characters and
/// the bidirectional overrides.
fn is_invisible(c: char) -> bool {
    matches!(
        c,
        '\u{200b}'..='\u{200f}'
            | '\u{202a}'..='\u{202e}'
            | '\u{2060}'..='\u{2064}'
            | '\u{2066}'..='\u{2069}'
            | '\u{feff}'
    )
}

/// Returns `text` as one line: line breaks become `↵`, tabs become spaces, and other control
/// and invisible characters are removed.
pub fn clean(text: &str) -> String {
    text.chars()
        .filter_map(|c| match c {
            '\n' | '\r' => Some(NEWLINE),
            '\t' => Some(' '),
            c if c.is_control() || is_invisible(c) => None,
            c => Some(c),
        })
        .collect()
}

/// Quotes `arg` as one shell word when it holds anything but plain characters.
pub fn quote_arg(arg: &str) -> String {
    let plain = !arg.is_empty()
        && arg
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_./:=@%+,".contains(c));
    if plain {
        arg.to_string()
    } else {
        format!("'{}'", arg.replace('\'', r"'\''"))
    }
}

/// Joins `args` into one command line, each quoted where needed.
pub fn join_args(args: &[String]) -> String {
    args.iter()
        .map(|arg| quote_arg(arg))
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_marks_line_breaks_and_drops_control_and_invisible_characters() {
        assert_eq!(clean("a\nb\r\nc\td"), "a\u{21b5}b\u{21b5}\u{21b5}c d");
        assert_eq!(
            clean("rm\u{0}\u{1b}[2J -rf\u{202e}x\u{200b}y"),
            "rm[2J -rfxy"
        );
        assert_eq!(clean("plain ünïcode"), "plain ünïcode");
    }

    #[test]
    fn quote_arg_quotes_only_what_needs_it() {
        assert_eq!(quote_arg("view"), "view");
        assert_eq!(quote_arg("a b"), "'a b'");
        assert_eq!(quote_arg("it's"), r"'it'\''s'");
        assert_eq!(quote_arg(""), "''");
        assert_eq!(
            join_args(&["pr".into(), "title; rm".into()]),
            "pr 'title; rm'"
        );
    }
}
