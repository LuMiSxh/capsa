// SPDX-License-Identifier: MPL-2.0
//! Scratch directories for census's tests, under the system temp directory.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static NEXT: AtomicU64 = AtomicU64::new(0);

/// A new directory that is removed when the value is dropped.
pub struct Scratch(PathBuf);

impl Scratch {
    #[expect(
        clippy::expect_used,
        reason = "a scratch directory that cannot be made must fail the test"
    )]
    pub fn new(name: &str) -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let unique = NEXT.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "census-test-{name}-{}-{nanos}-{unique}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).expect("create the scratch directory");
        Scratch(dir)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }

    /// Writes `text` to `rel` under the directory, creating parents, and returns the path.
    pub fn write(&self, rel: &str, text: &str) -> PathBuf {
        self.write_bytes(rel, text.as_bytes())
    }

    /// Writes `bytes` to `rel` under the directory, creating parents, and returns the path.
    #[expect(
        clippy::expect_used,
        reason = "a fixture that cannot be written must fail the test"
    )]
    pub fn write_bytes(&self, rel: &str, bytes: &[u8]) -> PathBuf {
        let path = self.0.join(rel);
        std::fs::create_dir_all(path.parent().unwrap_or(&self.0)).expect("create parents");
        std::fs::write(&path, bytes).expect("write the fixture");
        path
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
