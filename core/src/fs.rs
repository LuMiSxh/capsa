// SPDX-License-Identifier: MPL-2.0
//! Replaces whole files so that a reader sees the old bytes or the new ones, never a part.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

/// Replaces the file at `path` with `bytes`. The bytes go to a new temporary file in the same
/// directory, are synced, then renamed over `path`, so readers see the old file or the new one.
/// An existing file keeps its permissions, so a private file stays private. On Unix the
/// directory is synced after the rename, so the new file survives a crash; a filesystem that
/// cannot sync a directory still gets the new file.
///
/// # Errors
/// Fails when the temporary file cannot be created, written or synced, or when the rename fails.
/// The temporary file is removed on failure, and `path` is left as it was.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let (tmp, mut file) = create_temp(path)?;
    let written = keep_permissions(path, &file)
        .and_then(|()| file.write_all(bytes))
        .and_then(|()| file.sync_all());
    drop(file);
    let result = written.and_then(|()| fs::rename(&tmp, path));
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    if result.is_ok() {
        // The new bytes are already in place; some FUSE and network mounts refuse a directory
        // sync, and reporting that as a failed write would be wrong.
        let _ = sync_parent(path);
    }
    result
}

#[cfg(unix)]
fn sync_parent(path: &Path) -> io::Result<()> {
    match path.parent().filter(|dir| !dir.as_os_str().is_empty()) {
        Some(dir) => File::open(dir)?.sync_all(),
        None => File::open(".")?.sync_all(),
    }
}

// Windows cannot open a directory as a `File` without extra flags, and NTFS journals the rename.
#[cfg(not(unix))]
fn sync_parent(_path: &Path) -> io::Result<()> {
    Ok(())
}

fn keep_permissions(path: &Path, file: &File) -> io::Result<()> {
    match fs::metadata(path) {
        Ok(meta) => file.set_permissions(meta.permissions()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

/// Creates a new file beside `path`, named with this process id and a counter, so that calls
/// never share a temporary file. A name that already exists is skipped, never reused.
fn create_temp(path: &Path) -> io::Result<(PathBuf, File)> {
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    loop {
        let n = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        let tmp = path.with_file_name(format!(".{name}.{}.{n}.tmp", std::process::id()));
        match OpenOptions::new().write(true).create_new(true).open(&tmp) {
            Ok(file) => return Ok((tmp, file)),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::write_atomic;
    use std::path::Path;

    fn names(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn writes_a_missing_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        write_atomic(&path, b"{}\n").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"{}\n");
    }

    #[test]
    fn replaces_an_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(&path, "old").unwrap();
        write_atomic(&path, b"new").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "new");
    }

    #[cfg(unix)]
    #[test]
    fn keeps_the_permissions_of_the_replaced_file() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(&path, "old").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        write_atomic(&path, b"new").unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[test]
    fn leaves_no_temporary_file_behind() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        write_atomic(&path, b"x").unwrap();
        assert_eq!(names(dir.path()), ["settings.json"]);
    }

    #[test]
    fn back_to_back_writes_do_not_collide() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        for i in 0..50 {
            write_atomic(&path, i.to_string().as_bytes()).unwrap();
        }
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "49");
        assert_eq!(names(dir.path()), ["settings.json"]);
    }

    #[test]
    fn failed_rename_removes_the_temporary_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("occupied");
        std::fs::create_dir(&path).unwrap();
        assert!(write_atomic(&path, b"x").is_err());
        assert_eq!(names(dir.path()), ["occupied"]);
        assert!(path.is_dir());
    }
}
