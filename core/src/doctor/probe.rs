// SPDX-License-Identifier: MPL-2.0
//! Probes the file system for what a doctor reports: whether a directory takes a file, and
//! whether two paths name the same file.

use std::borrow::Cow;
use std::io;
use std::path::Path;

use super::check;
use crate::render::{Check, Status};

/// Reports whether `dir` takes a file. A missing directory is created on first use, so it is not
/// an error; a directory that cannot take a probe file fails with `hint` as its fix.
pub fn dir_check(name: impl Into<Cow<'static, str>>, dir: &Path, hint: &str) -> Check {
    let name = name.into();
    if !dir.is_dir() {
        return check(
            name,
            Status::Ok,
            format!("{} not created yet", dir.display()),
            None,
        );
    }
    match writable(dir) {
        Ok(()) => check(
            name,
            Status::Ok,
            format!("{} writable", dir.display()),
            None,
        ),
        Err(e) => check(
            name,
            Status::Fail,
            format!("{} not writable: {e}", dir.display()),
            Some(hint),
        ),
    }
}

/// Reports whether `dir` takes a file, by creating and then removing a probe file in it.
///
/// # Errors
/// Fails with the error of the probe when the file cannot be created or removed.
pub fn writable(dir: &Path) -> io::Result<()> {
    let probe = dir.join(format!(".capsa-doctor-{}", std::process::id()));
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&probe)?;
    std::fs::remove_file(&probe)
}

/// Reports whether `a` and `b` name the same file, resolving symlinks where they can be.
///
/// A path that cannot be canonicalized, such as one that does not exist, compares as written.
pub fn same_file(a: &Path, b: &Path) -> bool {
    let canon = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_owned());
    canon(a) == canon(b)
}
