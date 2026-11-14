// SPDX-License-Identifier: MPL-2.0
#![expect(clippy::unwrap_used, reason = "test code")]

mod conflicts;
mod files;
mod line_ranges;
mod replace_all;
mod search_replace;
mod symbols;

use acus_edit::{ApplyOptions, Change, EditError, apply, parse};
use capsa_core::hint::Hint;
use std::fs;
use std::path::Path;

fn dir(files: &[(&str, &str)]) -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    for (p, c) in files {
        let p = d.path().join(p);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, c).unwrap();
    }
    d
}

/// The error as the CLI prints it: the message, then the hint on its own line.
trait Shown {
    fn shown(&self) -> String;
}

impl Shown for EditError {
    fn shown(&self) -> String {
        match self.hint() {
            Some(hint) => format!("{self}\nhint: {hint}"),
            None => self.to_string(),
        }
    }
}

/// Options that check a patch without writing it.
fn check_only() -> ApplyOptions {
    ApplyOptions {
        check: true,
        ..ApplyOptions::default()
    }
}

fn run(root: &Path, patch: &str) -> Result<Vec<Change>, EditError> {
    Ok(apply(&parse(patch)?, root, &ApplyOptions::default())?.changes)
}

fn read(root: &Path, p: &str) -> String {
    fs::read_to_string(root.join(p)).unwrap()
}

fn no_temp_files(root: &Path) {
    for e in fs::read_dir(root).unwrap() {
        let n = e.unwrap().file_name();
        assert!(!n.to_string_lossy().starts_with(".tmp"), "leftover {n:?}");
    }
}
