// SPDX-License-Identifier: MPL-2.0
//! Chooses what `acus find` searches: walked paths, addressed ranges and standard input.

use crate::source::warn_missing_roots;
use acus_search::{FileHits, FindOpts, find, find_ranges, find_stdin};
use acus_syntax::Address;
use anyhow::{Context, Result};
use std::io::Read;
use std::path::Path;
use std::sync::mpsc;
use std::time::Duration;

#[cfg(not(unix))]
use std::io::IsTerminal;

/// How long a silent pipe is read before `piped_stdin` says it is waiting on stderr.
const STUCK_WAIT: Duration = Duration::from_secs(2);

/// One line, once, when a pipe has sent nothing after `STUCK_WAIT`.
const STUCK_NOTE: &str = "acus: waiting for piped stdin; pass a PATH to search files instead";

/// Files with hits: the walked paths, the addressed line ranges, then standard input.
/// Sorted by path.
pub(super) fn search_all(
    opts: &FindOpts,
    walk_files: bool,
    stdin: Option<&[u8]>,
    spans: &[(Address, (usize, usize))],
) -> Result<Vec<FileHits>> {
    let mut files = if walk_files {
        warn_missing_roots(&opts.walk);
        find(opts)?
    } else {
        Vec::new()
    };
    files.extend(find_ranges(
        opts,
        spans
            .iter()
            .map(|(addr, lines)| (Path::new(&addr.path), *lines)),
    )?);
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
/// filters the command's output. Only a pipe or a regular file is read; a terminal, a socket, a
/// character device (`/dev/null`), a closed descriptor or an empty file gives `None`, so those
/// keep the walk of `.`.
#[cfg(unix)]
pub(super) fn piped_stdin() -> Option<Vec<u8>> {
    use std::os::fd::AsFd;
    use std::os::unix::fs::FileTypeExt;
    let owned = std::io::stdin().as_fd().try_clone_to_owned().ok()?;
    let kind = std::fs::File::from(owned).metadata().ok()?.file_type();
    if kind.is_file() {
        return read_stdin().ok().filter(|input| !input.is_empty());
    }
    if !kind.is_fifo() {
        return None;
    }
    read_pipe()
}

/// The piped standard input for a search with no path. Without the unix file-type check, any
/// non-terminal stdin is read, as before.
#[cfg(not(unix))]
pub(super) fn piped_stdin() -> Option<Vec<u8>> {
    if std::io::stdin().is_terminal() {
        return None;
    }
    read_pipe()
}

/// Reads a pipe to the end, naming the wait on stderr when it stays silent for `STUCK_WAIT`.
fn read_pipe() -> Option<Vec<u8>> {
    let (tx, rx) = mpsc::channel();
    // A reader thread, not a blocking read on this one, so the wait can name itself; a stuck
    // pipe is visible without killing the read and losing its data.
    std::thread::spawn(move || {
        let mut buf = [0u8; 8192];
        loop {
            match std::io::stdin().read(&mut buf) {
                Ok(0) | Err(_) => return,
                Ok(n) => {
                    if tx.send(buf[..n].to_vec()).is_err() {
                        return;
                    }
                }
            }
        }
    });
    // EOF with no bytes gives `None`; after the note the wait stays open until data or EOF.
    let mut input = match rx.recv_timeout(STUCK_WAIT) {
        Ok(first) => first,
        Err(mpsc::RecvTimeoutError::Timeout) => {
            eprintln!("{STUCK_NOTE}");
            rx.recv().ok()?
        }
        Err(mpsc::RecvTimeoutError::Disconnected) => return None,
    };
    while let Ok(chunk) = rx.recv() {
        input.extend(chunk);
    }
    Some(input)
}
