// SPDX-License-Identifier: MPL-2.0
//! Turns agent-supplied text into a single visible line that is safe to show and to log.

/// The longest agent-supplied field (command, URL, one argument) the broker accepts. A longer
/// one is refused, never shortened, so the dialog always shows the whole text.
pub const MAX_FIELD: usize = 2000;

/// The marker that stands for a line break, so a multi-line command cannot pass for one line.
const NEWLINE: char = '\u{21b5}';

/// Returns whether `c` changes how text is laid out or hides it: format characters (Cf),
/// the line and paragraph separators, and the default-ignorable code points that render as
/// nothing (fillers, variation selectors, tags).
fn is_invisible(c: char) -> bool {
    matches!(
        c,
        '\u{00ad}'
            | '\u{034f}'
            | '\u{0600}'..='\u{0605}'
            | '\u{061c}'
            | '\u{06dd}'
            | '\u{070f}'
            | '\u{08e2}'
            | '\u{115f}'
            | '\u{1160}'
            | '\u{17b4}'
            | '\u{17b5}'
            | '\u{180b}'..='\u{180f}'
            | '\u{200b}'..='\u{200f}'
            | '\u{2028}'..='\u{202e}'
            | '\u{2060}'..='\u{206f}'
            | '\u{3164}'
            | '\u{fe00}'..='\u{fe0f}'
            | '\u{feff}'
            | '\u{ffa0}'
            | '\u{fff0}'..='\u{fffb}'
            | '\u{110bd}'
            | '\u{110cd}'
            | '\u{13430}'..='\u{1343f}'
            | '\u{1bca0}'..='\u{1bca3}'
            | '\u{1d173}'..='\u{1d17a}'
            | '\u{e0000}'..='\u{e0fff}'
    )
}

/// Returns the first character of `text` that a request may not contain: a control
/// character (line breaks and tabs included) or an invisible one. Other Unicode is fine.
pub fn forbidden(text: &str) -> Option<char> {
    text.chars().find(|c| c.is_control() || is_invisible(*c))
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
    fn forbidden_finds_each_class_of_hidden_character_and_allows_other_unicode() {
        for c in [
            '\u{0}',
            '\n',
            '\r',
            '\t',
            '\u{1b}',
            '\u{7f}',
            '\u{85}',
            '\u{9f}',
            '\u{ad}',
            '\u{34f}',
            '\u{61c}',
            '\u{115f}',
            '\u{1160}',
            '\u{180b}',
            '\u{180f}',
            '\u{200b}',
            '\u{200f}',
            '\u{2028}',
            '\u{2029}',
            '\u{202e}',
            '\u{2066}',
            '\u{2069}',
            '\u{3164}',
            '\u{fe00}',
            '\u{fe0f}',
            '\u{feff}',
            '\u{ffa0}',
            '\u{e0001}',
            '\u{e0041}',
            '\u{e0fff}',
        ] {
            assert_eq!(
                forbidden(&format!("a{c}b")),
                Some(c),
                "{:04x}",
                u32::from(c)
            );
        }
        assert_eq!(forbidden("Grüße, 世界 café ñ — ok"), None);
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
