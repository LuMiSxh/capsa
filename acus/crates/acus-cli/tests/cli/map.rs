// SPDX-License-Identifier: MPL-2.0
use crate::common::*;

#[test]
fn map_takes_several_paths_and_ignores_a_repeated_root() {
    let (code, out, err) = acus(&["map", "src", "web"]);
    assert_eq!((code, err.as_str()), (0, ""));
    assert!(out.starts_with("== .  "), "{out}");
    assert!(out.contains("src/lib.rs"), "{out}");
    assert!(out.contains("web/App.svelte"), "{out}");
    assert!(out.contains("2 files"), "{out}");
    // The same root twice is one tree, not twice the counts.
    let (_, once, _) = acus(&["map", "src"]);
    let (_, twice, _) = acus(&["map", "src", "src"]);
    assert_eq!(twice, once);
}

#[test]
fn map_takes_head_and_cut() {
    let (code, out, _) = acus(&["map", "src", "--head", "1"]);
    assert_eq!(code, 0);
    assert_eq!(out.lines().count(), 2, "{out}");
    assert!(
        out.ends_with("… output cut at --head 1; 1 more lines (rerun with --head 2)\n"),
        "{out}"
    );
    let (_, out, _) = acus(&["map", "src", "--cut", "3"]);
    assert!(out.lines().next().unwrap().contains("… (+"), "{out}");
}
