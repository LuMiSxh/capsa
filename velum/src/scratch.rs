// SPDX-License-Identifier: MPL-2.0
//! A scratch directory for tests, removed when it is dropped.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

/// Returns a fresh directory under the system temp directory, named after the test.
pub struct Scratch(PathBuf);

impl Scratch {
    pub fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("velum-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("creating a scratch directory");
        Self(dir)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Writes an executable `#!/bin/sh` program named `name` into `dir`, with `body` as its script.
pub fn write_program(dir: &Path, name: &str, body: &str) {
    std::fs::create_dir_all(dir).expect("creating a program directory");
    let file = dir.join(name);
    std::fs::write(&file, format!("#!/bin/sh\n{body}\n")).expect("writing a program");
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o755)).expect("chmod");
}
