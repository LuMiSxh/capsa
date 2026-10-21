// SPDX-License-Identifier: MPL-2.0
#![expect(clippy::unwrap_used, reason = "test code")]

use acus_walk::{WalkOpts, display_path, walk};
use std::{fs, path::Path, sync::Mutex};

fn tree() -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    for f in ["a.rs", "b.md", "sub/c.rs", ".hidden/d.rs", "ignored/e.rs"] {
        let p = d.path().join(f);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, "x").unwrap();
    }
    fs::write(d.path().join(".gitignore"), "ignored/\n").unwrap();
    d
}

#[cfg(unix)]
#[test]
fn follow_visits_files_under_symlinked_directories() {
    let d = tree();
    let target = tempfile::tempdir().unwrap();
    fs::write(target.path().join("linked.rs"), "x").unwrap();
    std::os::unix::fs::symlink(target.path(), d.path().join("link")).unwrap();
    let plain = collect(WalkOpts::default(), d.path());
    assert!(!plain.iter().any(|p| p.starts_with("link/")), "{plain:?}");
    let followed = collect(
        WalkOpts {
            follow: true,
            ..Default::default()
        },
        d.path(),
    );
    assert!(
        followed.contains(&"link/linked.rs".to_owned()),
        "{followed:?}"
    );
}

fn collect(opts: WalkOpts, root: &Path) -> Vec<String> {
    let out = Mutex::new(Vec::new());
    let opts = WalkOpts {
        roots: vec![root.to_path_buf()],
        ..opts
    };
    walk(&opts, |p| {
        out.lock()
            .unwrap()
            .push(display_path(p.strip_prefix(root).unwrap()));
    })
    .unwrap();
    let mut v = out.into_inner().unwrap();
    v.sort();
    v
}

#[test]
fn respects_gitignore_and_skips_hidden() {
    let d = tree();
    assert_eq!(
        collect(WalkOpts::default(), d.path()),
        ["a.rs", "b.md", "sub/c.rs"]
    );
}

#[test]
fn glob_without_slash_matches_the_name_at_any_depth() {
    let d = tree();
    let named = |g: &str| {
        collect(
            WalkOpts {
                include: vec![g.into()],
                ..Default::default()
            },
            d.path(),
        )
    };
    assert_eq!(named("c.rs"), ["sub/c.rs"]);
    assert_eq!(named("*.rs"), ["a.rs", "sub/c.rs"]);
    // A glob with a `/` stays anchored at the root.
    assert_eq!(named("sub/c.rs"), ["sub/c.rs"]);
    assert!(named("c/c.rs").is_empty());
}

#[test]
fn hidden_flag_includes_dotfiles() {
    let d = tree();
    let got = collect(
        WalkOpts {
            hidden: true,
            ..Default::default()
        },
        d.path(),
    );
    assert!(got.contains(&".hidden/d.rs".to_string()), "{got:?}");
}

#[test]
fn globs_match_relative_to_root() {
    let d = tree();
    let opts = WalkOpts {
        include: vec!["*.rs".into()],
        exclude: vec!["sub/**".into()],
        ..Default::default()
    };
    assert_eq!(collect(opts, d.path()), ["a.rs"]);
}

#[test]
fn bare_exclude_drops_directories_and_files_by_name() {
    let d = tree();
    let opts = WalkOpts {
        exclude: vec!["sub".into(), "*.md".into()],
        ..Default::default()
    };
    assert_eq!(collect(opts, d.path()), ["a.rs"]);
}

#[test]
fn missing_root_is_an_error() {
    let opts = WalkOpts {
        roots: vec!["/definitely/not/here".into()],
        ..Default::default()
    };
    assert!(
        walk(&opts, |_| {})
            .unwrap_err()
            .to_string()
            .contains("path not found")
    );
}
#[test]
fn missing_root_is_skipped_when_another_exists() {
    let d = tree();
    let opts = WalkOpts {
        roots: vec!["/definitely/not/here".into(), d.path().join("sub")],
        ..Default::default()
    };
    let out = Mutex::new(Vec::new());
    walk(&opts, |p| out.lock().unwrap().push(p.to_path_buf())).unwrap();
    assert_eq!(out.into_inner().unwrap(), [d.path().join("sub/c.rs")]);
}

#[test]
fn display_path_normalises() {
    assert_eq!(display_path(Path::new("./src/a.rs")), "src/a.rs");
    assert_eq!(display_path(Path::new("src\\a.rs")), "src/a.rs");
}
