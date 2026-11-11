// SPDX-License-Identifier: MPL-2.0
//! Takes advisory OS file locks that the kernel releases when the holder exits or crashes.

use std::fs::{File, OpenOptions, TryLockError};
use std::io;
use std::path::Path;
use std::time::{Duration, Instant};

const POLL: Duration = Duration::from_millis(1);

/// Holds a file lock until dropped.
#[derive(Debug)]
pub struct Lock(#[expect(dead_code, reason = "the open file keeps the lock")] File);

/// Takes an exclusive lock on the file at `path`, creating it when missing, and retries for
/// `wait`. `Duration::ZERO` tries once. Returns `None` when another holder kept the lock for all
/// of `wait`.
///
/// The file is never truncated, and its parent directory must exist.
///
/// # Errors
/// Fails when the file cannot be opened or the lock call fails for a reason other than a holder.
pub fn exclusive(path: &Path, wait: Duration) -> io::Result<Option<Lock>> {
    acquire(path, wait, File::try_lock)
}

/// Takes a shared lock on the file at `path`; see [`exclusive`]. Shared locks coexist and block
/// exclusive ones.
///
/// # Errors
/// Same as [`exclusive`].
pub fn shared(path: &Path, wait: Duration) -> io::Result<Option<Lock>> {
    acquire(path, wait, File::try_lock_shared)
}

fn acquire(
    path: &Path,
    wait: Duration,
    try_lock: fn(&File) -> Result<(), TryLockError>,
) -> io::Result<Option<Lock>> {
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(path)?;
    let start = Instant::now();
    loop {
        match try_lock(&file) {
            Ok(()) => return Ok(Some(Lock(file))),
            Err(TryLockError::WouldBlock) => {
                if start.elapsed() >= wait {
                    return Ok(None);
                }
                std::thread::sleep(POLL);
            }
            Err(TryLockError::Error(e)) => return Err(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHORT: Duration = Duration::from_millis(20);

    #[test]
    fn second_exclusive_waits_then_gets_none() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("l");
        let _held = exclusive(&path, SHORT).unwrap().unwrap();
        let start = Instant::now();
        assert!(exclusive(&path, SHORT).unwrap().is_none());
        assert!(start.elapsed() >= SHORT);
    }

    #[test]
    fn zero_wait_tries_once() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("l");
        let _held = exclusive(&path, Duration::ZERO).unwrap().unwrap();
        assert!(exclusive(&path, Duration::ZERO).unwrap().is_none());
    }

    #[test]
    fn shared_locks_coexist_and_block_exclusive() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("l");
        let a = shared(&path, SHORT).unwrap().unwrap();
        let b = shared(&path, SHORT).unwrap().unwrap();
        assert!(exclusive(&path, SHORT).unwrap().is_none());
        drop((a, b));
        assert!(exclusive(&path, SHORT).unwrap().is_some());
    }

    #[test]
    fn exclusive_blocks_shared() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("l");
        let _held = exclusive(&path, SHORT).unwrap().unwrap();
        assert!(shared(&path, SHORT).unwrap().is_none());
    }

    #[test]
    fn drop_releases_the_lock() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("l");
        drop(exclusive(&path, SHORT).unwrap());
        assert!(exclusive(&path, SHORT).unwrap().is_some());
    }

    #[test]
    fn existing_content_is_kept() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("l");
        std::fs::write(&path, "old nonce").unwrap();
        let _held = exclusive(&path, SHORT).unwrap().unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "old nonce");
    }

    #[test]
    fn missing_parent_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        assert!(exclusive(&dir.path().join("no/l"), SHORT).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn a_killed_process_releases_its_lock() {
        use std::io::Read;
        use std::process::{Command, Stdio};
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("l");
        let script = "use Fcntl qw(:flock); open(F, '>>', $ARGV[0]); flock(F, LOCK_EX); \
                      $| = 1; print qq(x\\n); sleep 30";
        let mut child = Command::new("perl")
            .args(["-e", script])
            .arg(&path)
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut ready = [0_u8; 2];
        child
            .stdout
            .as_mut()
            .unwrap()
            .read_exact(&mut ready)
            .unwrap();
        assert!(exclusive(&path, SHORT).unwrap().is_none());
        child.kill().unwrap();
        child.wait().unwrap();
        assert!(
            exclusive(&path, Duration::from_millis(500))
                .unwrap()
                .is_some()
        );
    }
}
