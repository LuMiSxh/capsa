// SPDX-License-Identifier: MPL-2.0
//! Chooses what `acus find` searches: walked paths, addressed ranges and standard input.

use acus_search::{FileHits, FindOpts, find, find_range, find_stdin};
use acus_syntax::Address;
use anyhow::{Context, Result};
use std::io::{IsTerminal, Read};
use std::path::Path;

/// Files with hits: the walked paths, the addressed line ranges, then standard input.
/// Sorted by path.
pub(super) fn search_all(
    opts: &FindOpts,
    walk_files: bool,
    stdin: Option<&[u8]>,
    spans: &[(Address, (usize, usize))],
) -> Result<Vec<FileHits>> {
    let mut files = if walk_files { find(opts)? } else { Vec::new() };
    for (addr, lines) in spans {
        files.extend(find_range(opts, Path::new(&addr.path), *lines)?);
    }
    if let Some(input) = stdin {
        files.extend(find_stdin(opts, input)?);
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(files)
}

/// Reads standard input to the end.
pub(super) fn read_stdin() -> Result<Vec<u8>> {
    let mut input = Vec::new();
    std::io::stdin()
        .read_to_end(&mut input)
        .context("cannot read stdin")?;
    Ok(input)
}

/// The piped standard input for a search with no path: as with rg, `cmd | acus find PAT`
/// filters the command's output. A terminal, an empty stream (`/dev/null`, an empty file) or a
/// closed descriptor gives `None`, so those keep the walk of `.`.
pub(super) fn piped_stdin() -> Option<Vec<u8>> {
    if std::io::stdin().is_terminal() {
        return None;
    }
    read_stdin().ok().filter(|input| !input.is_empty())
}
