// SPDX-License-Identifier: MPL-2.0
//! Owns reading a package's version from its `package.json`.

use std::io::Read;
use std::path::{Path, PathBuf};

use serde_json::Value;

/// Returns the version and manifest path of `package` when the `package.json` in `dir`
/// names it. `read` reads a text file, so callers can inject the file system.
pub fn manifest_in(
    dir: &Path,
    package: &str,
    read: &dyn Fn(&Path) -> Option<String>,
) -> Option<(String, PathBuf)> {
    let path = dir.join("package.json");
    let json: Value = serde_json::from_str(&read(&path)?).ok()?;
    if json.get("name")?.as_str()? != package {
        return None;
    }
    let version = json.get("version")?.as_str()?.to_owned();
    Some((version, path))
}

/// Returns the version and manifest path of `package` behind a resolved global
/// `binary`: the `package.json` above the real path of the binary, or above the
/// target of a `cmd-shim` script (npm and pnpm write one, whose
/// `# cmd-shim-target=` line names the entry point). `canonicalize` resolves
/// symlinks (`None` keeps a path as given) and `read` reads text files.
pub fn manifest_for(
    binary: &Path,
    package: &str,
    canonicalize: &dyn Fn(&Path) -> Option<PathBuf>,
    read: &dyn Fn(&Path) -> Option<String>,
) -> Option<(String, PathBuf)> {
    let targets = std::iter::once(binary.to_path_buf()).chain(shim_target(binary));
    targets
        .map(|target| canonicalize(&target).unwrap_or(target))
        .find_map(|real| {
            real.ancestors()
                .skip(1)
                .find_map(|dir| manifest_in(dir, package, read))
        })
}

/// Returns the version of `package` behind `binary`, read from the real file system.
pub fn package_version(binary: &Path, package: &str) -> Option<String> {
    let canonicalize = |p: &Path| std::fs::canonicalize(p).ok();
    let read = |p: &Path| std::fs::read_to_string(p).ok();
    manifest_for(binary, package, &canonicalize, &read).map(|(version, _)| version)
}

/// Returns the entry point a `cmd-shim` script runs, from its `# cmd-shim-target=`
/// line. Only the first 4 KiB are read, so a native binary is cheap to ask.
fn shim_target(binary: &Path) -> Option<PathBuf> {
    let mut head = Vec::new();
    std::fs::File::open(binary)
        .ok()?
        .take(4096)
        .read_to_end(&mut head)
        .ok()?;
    String::from_utf8_lossy(&head)
        .lines()
        .find_map(|line| line.strip_prefix("# cmd-shim-target="))
        .map(|target| PathBuf::from(target.trim()))
}
