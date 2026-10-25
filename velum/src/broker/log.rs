// SPDX-License-Identifier: MPL-2.0
//! Owns the broker log: one line per request and per decision, never an output.

use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

/// Appends lines to `~/Library/Application Support/velum/broker.log`, which the sandbox
/// cannot write (it lies under the protected `~/Library/Application Support/velum`).
pub struct Log {
    path: PathBuf,
}

impl Log {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    /// Appends `<unix seconds> <line>`. The caller passes text that went through `text::clean`.
    pub fn write(&self, line: &str) -> std::io::Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .create(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&self.path)?;
        writeln!(file, "{secs} {line}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scratch::Scratch;

    #[test]
    fn appends_one_timestamped_line_per_write() {
        let scratch = Scratch::new("broker-log");
        let log = Log::new(scratch.path().join("sub/broker.log"));
        log.write("request git_push").unwrap();
        log.write("decision deny").unwrap();
        let text = std::fs::read_to_string(scratch.path().join("sub/broker.log")).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].ends_with(" request git_push"));
        assert!(lines[1].ends_with(" decision deny"));
    }

    #[test]
    fn refuses_to_follow_a_symlink() {
        let scratch = Scratch::new("broker-log-link");
        let target = scratch.path().join("target");
        std::fs::write(&target, "").unwrap();
        let link = scratch.path().join("broker.log");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        assert!(Log::new(link).write("x").is_err());
    }
}
