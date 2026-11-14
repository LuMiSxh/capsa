// SPDX-License-Identifier: MPL-2.0
//! Finds the formatter for a written file and pipes the file's text through it.

use anyhow::Result;
use std::cell::RefCell;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

/// The formatter binaries already looked up on PATH in this patch run, so each is searched once.
#[derive(Default)]
pub(super) struct Tools {
    found: RefCell<Vec<(&'static str, bool)>>,
}

impl Tools {
    /// Reports whether name is on PATH, searching only the first time it is asked.
    fn on_path(&self, name: &'static str) -> bool {
        if let Some(&(_, hit)) = self.found.borrow().iter().find(|(n, _)| *n == name) {
            return hit;
        }
        let path = std::env::var_os("PATH").unwrap_or_default();
        let hit = capsa_core::which::find_on_path(name, &path, None).is_some();
        self.found.borrow_mut().push((name, hit));
        hit
    }
}

/// Formatter command reading the file on stdin and printing it formatted, if one is installed.
pub(super) fn command_for(path: &Path, tools: &Tools) -> Option<Vec<String>> {
    let p = path.to_string_lossy().into_owned();
    let v = |a: &[&str]| Some(a.iter().map(|s| s.to_string()).collect());
    match path.extension()?.to_str()? {
        "rs" if tools.on_path("rustfmt") => v(&["rustfmt", "--edition", &rust_edition(path)]),
        "py" if tools.on_path("ruff") => v(&["ruff", "format", "--stdin-filename", &p, "-"]),
        "py" if tools.on_path("black") => v(&["black", "-q", "--stdin-filename", &p, "-"]),
        "go" if tools.on_path("gofmt") => v(&["gofmt"]),
        "swift" if tools.on_path("swift-format") => {
            v(&["swift-format", "format", "--assume-filename", &p])
        }
        "swift" if cfg!(target_os = "macos") && tools.on_path("xcrun") => {
            v(&["xcrun", "swift-format", "format", "--assume-filename", &p])
        }
        "ts" | "tsx" | "js" | "jsx" | "mjs" | "cjs" | "mts" | "cts" | "svelte" | "vue" | "css"
        | "scss" => {
            let bin = path
                .canonicalize()
                .ok()?
                .ancestors()
                .map(|d| d.join("node_modules/.bin/prettier"))
                .find(|b| b.is_file())
                .map(|b| b.to_string_lossy().into_owned())
                .or_else(|| tools.on_path("prettier").then(|| "prettier".into()))?;
            Some(vec![bin, "--stdin-filepath".into(), p])
        }
        _ => None,
    }
}

/// `edition` from the nearest Cargo.toml that sets one (following `edition.workspace`).
fn rust_edition(path: &Path) -> String {
    let abs = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    for d in abs.ancestors().skip(1) {
        let Ok(t) = std::fs::read_to_string(d.join("Cargo.toml")) else {
            continue;
        };
        let ed = t.lines().map(str::trim).find_map(|l| {
            let v = l.strip_prefix("edition")?.trim_start().strip_prefix('=')?;
            Some(v.trim().trim_matches('"').to_owned())
        });
        if let Some(ed) = ed.filter(|e| e.chars().all(|c| c.is_ascii_digit())) {
            return ed;
        }
    }
    "2021".into()
}

pub(super) fn pipe(argv: &[String], input: &str) -> Result<String> {
    let mut child = Command::new(&argv[0])
        .args(&argv[1..])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| anyhow::anyhow!("child stdin is not piped"))?;
    let input = input.to_owned();
    let writer = std::thread::spawn(move || stdin.write_all(input.as_bytes()));
    let out = child.wait_with_output()?;
    writer.join().ok();
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        anyhow::bail!(
            "{}",
            err.lines()
                .find(|l| !l.trim().is_empty())
                .unwrap_or("failed")
                .trim()
        );
    }
    Ok(String::from_utf8(out.stdout)?)
}
