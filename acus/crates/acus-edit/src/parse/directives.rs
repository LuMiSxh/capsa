// SPDX-License-Identifier: MPL-2.0
//! Holds the directive table that `parse` matches `*** Name:` lines against.

/// A directive `parse` accepts. `name` is what follows `*** ` and is matched exactly, `kind`
/// picks the op it builds, `form` is the usage that `did you mean` suggests and `note` is what
/// the accepted list adds after it.
pub(super) struct Directive {
    pub(super) name: &'static str,
    pub(super) kind: Kind,
    form: &'static str,
    note: &'static str,
}

/// What a directive does. Most open a new op; `MoveTo` sets the destination of the update
/// before it, and `Insert` covers both `*** Insert Before` and `*** Insert After`.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Kind {
    Add,
    Update,
    Delete,
    MoveTo,
    Copy,
    ReplaceSymbol,
    Insert { after: bool },
    DeleteSymbol,
    MoveSymbol,
    ReplaceAll,
}

/// Every directive `parse` accepts. The unknown-directive error and `did you mean` are built
/// from this table. A new directive needs a row here and a `Kind` arm in `start_op`, which the
/// compiler checks for.
const DIRECTIVES: &[Directive] = &[
    Directive {
        name: "Add File",
        kind: Kind::Add,
        form: "*** Add File: PATH",
        note: "",
    },
    Directive {
        name: "Update File",
        kind: Kind::Update,
        form: "*** Update File: PATH",
        note: "",
    },
    Directive {
        name: "Delete File",
        kind: Kind::Delete,
        form: "*** Delete File: PATH",
        note: "",
    },
    Directive {
        name: "Move to",
        kind: Kind::MoveTo,
        form: "*** Move to: PATH",
        note: " (after Update File)",
    },
    Directive {
        name: "Copy File",
        kind: Kind::Copy,
        form: "*** Copy File: PATH",
        note: " (then *** To: PATH)",
    },
    Directive {
        name: "Replace Symbol",
        kind: Kind::ReplaceSymbol,
        form: "*** Replace Symbol: PATH#Symbol",
        note: "",
    },
    Directive {
        name: "Insert Before",
        kind: Kind::Insert { after: false },
        form: "*** Insert Before: PATH#Symbol",
        note: " or PATH:LINE (then `+` lines)",
    },
    Directive {
        name: "Insert After",
        kind: Kind::Insert { after: true },
        form: "*** Insert After: PATH#Symbol",
        note: " or PATH:LINE (then `+` lines)",
    },
    Directive {
        name: "Delete Symbol",
        kind: Kind::DeleteSymbol,
        form: "*** Delete Symbol: PATH#Symbol",
        note: "",
    },
    Directive {
        name: "Move Symbol",
        kind: Kind::MoveSymbol,
        form: "*** Move Symbol: PATH#Symbol",
        note: " (then *** Before: PATH#Sym, *** After: PATH#Sym or *** To: PATH)",
    },
    Directive {
        name: "Replace All",
        kind: Kind::ReplaceAll,
        form: "*** Replace All: PATH [PATH...]",
        note: " (files, directories or globs, then `<<<<<<< SEARCH` or `<<<<<<< REGEX`, `=======`, `>>>>>>> REPLACE`; see `acus patch --help`)",
    },
];

/// The accepted forms, as the unknown-directive error lists them.
pub(super) fn accepted() -> String {
    let forms: Vec<String> = DIRECTIVES
        .iter()
        .map(|d| format!("{}{}", d.form, d.note))
        .collect();
    forms.join(" | ")
}

fn normalized(name: &str) -> String {
    name.chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

/// `did you mean …` for a directive with the right name but the wrong case or spacing, such as
/// `*** Replace all:` or `*** Replace All:src`; empty otherwise.
pub(super) fn did_you_mean(line: &str) -> String {
    let name = line
        .trim_start_matches('*')
        .split(':')
        .next()
        .unwrap_or_default();
    let norm = normalized(name);
    match DIRECTIVES.iter().find(|d| normalized(d.name) == norm) {
        Some(d) => format!(
            "did you mean `{}` (exact case, one space after the colon, a path after it)? ",
            d.form
        ),
        None => String::new(),
    }
}

/// Matches `*** Name: rest` against `DIRECTIVES`; `rest` is the text after the colon and space.
pub(super) fn split_directive(line: &str) -> Option<(&Directive, &str)> {
    let rest = line.strip_prefix("*** ")?;
    DIRECTIVES.iter().find_map(|d| {
        let after = rest.strip_prefix(d.name)?;
        let value = match after.strip_prefix(": ") {
            Some(value) => value,
            // `*** Replace Symbol path#Symbol`, without the colon, is accepted too.
            None if d.kind == Kind::ReplaceSymbol => after.strip_prefix(' ')?,
            None => return None,
        };
        Some((d, value))
    })
}
