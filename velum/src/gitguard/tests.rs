// SPDX-License-Identifier: MPL-2.0
use super::config::{config_hits, neutralise};
use super::*;
use crate::scratch::Scratch;
use std::os::unix::fs::symlink;

fn git_dir(root: &Path, rel: &str) -> PathBuf {
    let dir = root.join(rel).join(".git");
    fs::create_dir_all(dir.join("hooks")).unwrap();
    fs::write(dir.join("hooks/pre-commit.sample"), "#!/bin/sh\n").unwrap();
    fs::write(dir.join("config"), "[core]\n\tbare = false\n").unwrap();
    fs::write(dir.join("HEAD"), "ref: refs/heads/main\n").unwrap();
    dir
}

fn text(path: &Path) -> String {
    path.display().to_string()
}

#[test]
fn existing_repos_get_concrete_entries_and_skipped_dirs_are_not_walked() {
    let scratch = Scratch::new("git-entries");
    let root = scratch.path().join("work");
    let main = git_dir(&root, "a");
    let module = main.join("modules/lib/sub");
    fs::create_dir_all(module.join("hooks")).unwrap();
    fs::write(module.join("HEAD"), "x").unwrap();
    git_dir(&root, "target/skipped");
    git_dir(&root, "node_modules/skipped");
    let outside = scratch.path().join("outside");
    git_dir(&outside, "x");
    std::os::unix::fs::symlink(&outside, root.join("link")).unwrap();
    fs::write(root.join("wt.git"), "").unwrap();
    let start = Start::capture(&root);
    let entries = start.deny_entries();
    let skipped = ["target", "node_modules", ".build"]
        .map(|skip| format!("{}/**/{skip}/**/.git/config", root.display()));
    let concrete = [
        text(&main.join("hooks")),
        text(&main.join("config")),
        text(&module.join("hooks")),
        text(&module.join("config")),
    ];
    assert_eq!(entries, [concrete.as_slice(), skipped.as_slice()].concat());
    assert!(start.warning().is_none());
}

fn bare_shape(dir: &Path) {
    fs::create_dir_all(dir.join("objects")).unwrap();
    fs::create_dir_all(dir.join("refs")).unwrap();
    fs::write(dir.join("HEAD"), "ref: refs/heads/main\n").unwrap();
}

#[test]
fn a_directory_that_becomes_bare_shaped_is_reported_once() {
    let scratch = Scratch::new("git-bare");
    let root = scratch.path().join("work");
    bare_shape(&root.join("old"));
    git_dir(&root, "repo");
    fs::create_dir_all(root.join("later/inner")).unwrap();
    let home = scratch.path().join("home");
    let start = Start::capture(&root);
    assert!(start.check(home.to_str().unwrap(), "run").is_empty());
    bare_shape(&root.join("later/inner"));
    let lines = start.check(home.to_str().unwrap(), "run");
    let text = lines.join("\n");
    assert!(
        text.contains(&root.join("later/inner").display().to_string()),
        "{text}"
    );
    assert!(!text.contains("work/old"), "{text}");
    assert!(!text.contains("work/repo"), "{text}");
    assert!(text.contains("safe.bareRepository explicit"), "{text}");
}

#[test]
fn a_git_file_is_denied_itself() {
    let scratch = Scratch::new("git-file");
    let root = scratch.path().join("work");
    fs::create_dir_all(root.join("sub")).unwrap();
    fs::write(root.join("sub/.git"), "gitdir: ../elsewhere\n").unwrap();
    let entries = Start::capture(&root).deny_entries();
    assert_eq!(entries[0], text(&root.join("sub/.git")));
}

#[test]
fn a_capped_walk_falls_back_to_globs() {
    let scratch = Scratch::new("git-cap");
    let root = scratch.path().join("work");
    for name in ["a", "b", "c"] {
        git_dir(&root, name);
    }
    let start = Start::capture_with(&root, 2, 100);
    assert!(start.warning().is_some());
    let globs = start.deny_entries();
    assert!(globs.contains(&format!("{}/**/.git/hooks", root.display())));
    assert!(globs.contains(&format!("{}/**/.git/modules/*/config", root.display())));
    let start = Start::capture_with(&root, 100, 2);
    assert!(start.warning().is_some());
    assert!(start.deny_entries().iter().any(|e| e.contains("/**/.git/")));
}

#[test]
fn a_path_with_glob_characters_adds_globs() {
    let scratch = Scratch::new("git-glob-name");
    let root = scratch.path().join("work");
    git_dir(&root, "we[ird");
    let entries = Start::capture(&root).deny_entries();
    assert!(entries.iter().any(|e| e.contains("/**/.git/hooks")));
    assert!(!entries.iter().any(|e| e.contains("we[ird")));
}

#[test]
fn new_repo_hooks_are_quarantined_and_config_is_reported() {
    let scratch = Scratch::new("git-new");
    let root = scratch.path().join("work");
    let home = scratch.path().join("home");
    fs::create_dir_all(&home).unwrap();
    let old = git_dir(&root, "old");
    let start = Start::capture(&root);
    // The agent plants a hook in the old repo (the sandbox would refuse; the scan ignores it).
    fs::write(old.join("hooks/post-checkout"), "x").unwrap();
    let fresh = git_dir(&root, "fresh");
    fs::write(fresh.join("hooks/pre-commit"), "#!/bin/sh\nevil\n").unwrap();
    fs::write(
        fresh.join("config"),
        "[core]\n\thooksPath = /x\n\tbare = false\n[filter \"lfs\"]\n\tsmudge = evil\n[diff \"p\"]\n\ttextconv = evil\n[includeIf \"gitdir:~/\"]\n\tpath = /y\n",
    )
    .unwrap();
    let lines = start.check(home.to_str().unwrap(), "run-1");
    let quarantine = home.join("Library/Application Support/velum/quarantine/run-1");
    assert!(!fresh.join("hooks/pre-commit").exists());
    assert!(fresh.join("hooks/pre-commit.sample").exists());
    assert!(old.join("hooks/post-checkout").exists());
    let kept: Vec<String> = fs::read_dir(&quarantine)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(kept, ["0-pre-commit"]);
    let joined = lines.join("\n");
    assert!(joined.contains(&text(&quarantine)), "{joined}");
    for key in [
        "core.hookspath",
        "filter.smudge",
        "diff.textconv",
        "includeif.path",
    ] {
        assert!(joined.contains(key), "{key} in {joined}");
    }
    assert!(!joined.contains("core.bare"));
    let kept = fs::read_to_string(fresh.join("config")).unwrap();
    assert!(!kept.contains("hooksPath"), "{kept}");
    assert!(
        kept.contains("bare = false") && kept.contains("smudge = evil"),
        "{kept}"
    );
    assert!(joined.contains("removed git config entries"), "{joined}");
}

#[test]
fn neutralise_removes_hooks_path_and_fsmonitor_only() {
    let scratch = Scratch::new("git-neutralise");
    let dir = scratch.path().join("g");
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("config"),
        "[core]\n\tbare = false\n\thooksPath = /x\n\tfsmonitor = \\\n\t  /evil\n\teditor = vi\n[core] FsMonitor = y\n[alias]\n\tx = !sh\n",
    )
    .unwrap();
    let removed = neutralise(&dir).unwrap();
    assert_eq!(removed.len(), 3, "{removed:?}");
    assert_eq!(
        fs::read_to_string(dir.join("config")).unwrap(),
        "[core]\n\tbare = false\n\teditor = vi\n[core]\n[alias]\n\tx = !sh\n"
    );
    assert!(neutralise(&dir).unwrap().is_empty());
}

#[test]
fn neutralise_never_follows_a_config_link() {
    let scratch = Scratch::new("git-neutralise-link");
    let dir = scratch.path().join("g");
    fs::create_dir_all(&dir).unwrap();
    let victim = scratch.path().join("victim");
    fs::write(&victim, "[core]\n\thooksPath = /x\n").unwrap();
    symlink(&victim, dir.join("config")).unwrap();
    assert!(neutralise(&dir).unwrap().is_empty());
    assert!(fs::read_to_string(&victim).unwrap().contains("hooksPath"));
}

#[test]
fn existing_bare_directories_get_hooks_and_config_entries() {
    let scratch = Scratch::new("git-bare-deny");
    let root = scratch.path().join("work");
    let bare = root.join("vendor/fixture.git");
    bare_shape(&bare);
    fs::create_dir_all(bare.join("modules/m/hooks")).unwrap();
    fs::write(bare.join("modules/m/HEAD"), "x").unwrap();
    let entries = Start::capture(&root).deny_entries();
    for tail in ["hooks", "config", "modules/m/hooks", "modules/m/config"] {
        assert!(
            entries.contains(&text(&bare.join(tail))),
            "{tail} in {entries:?}"
        );
    }
}

#[test]
fn a_git_file_naming_a_dir_outside_cwd_or_through_a_link_is_left_alone() {
    let scratch = Scratch::new("git-link-outside");
    let root = scratch.path().join("work");
    let outside = scratch.path().join("other/.git");
    fs::create_dir_all(outside.join("hooks")).unwrap();
    fs::write(outside.join("hooks/pre-commit"), "x").unwrap();
    fs::create_dir_all(root.join("a")).unwrap();
    fs::create_dir_all(root.join("b")).unwrap();
    let start = Start::capture(&root);
    fs::write(
        root.join("a/.git"),
        format!("gitdir: {}\n", outside.display()),
    )
    .unwrap();
    symlink(&outside, root.join("viaLink")).unwrap();
    fs::write(root.join("b/.git"), "gitdir: ../viaLink\n").unwrap();
    let lines = start.check(scratch.path().to_str().unwrap(), "run-5");
    assert!(outside.join("hooks/pre-commit").exists());
    let joined = lines.join("\n");
    assert_eq!(joined.matches("which is outside").count(), 2, "{joined}");
}

#[test]
fn a_git_file_naming_a_dir_that_existed_before_is_left_alone() {
    let scratch = Scratch::new("git-link-old");
    let root = scratch.path().join("work");
    let old = git_dir(&root, "old");
    fs::write(old.join("hooks/post-merge"), "x").unwrap();
    fs::create_dir_all(root.join("sub")).unwrap();
    let start = Start::capture(&root);
    fs::write(root.join("sub/.git"), "gitdir: ../old/.git\n").unwrap();
    start.check(scratch.path().to_str().unwrap(), "run-6");
    assert!(old.join("hooks/post-merge").exists());
}

#[test]
fn move_aside_copies_a_file_but_keeps_a_directory_or_link_after_a_failed_rename() {
    let scratch = Scratch::new("git-move");
    let root = scratch.path();
    let failing = |_: &Path, _: &Path| Err(io::Error::from_raw_os_error(18));
    let file = root.join("hook");
    fs::write(&file, "body").unwrap();
    move_aside_with(&file, &root.join("copy"), failing).unwrap();
    assert!(!file.exists());
    assert_eq!(fs::read_to_string(root.join("copy")).unwrap(), "body");
    let dir = root.join("hooks");
    fs::create_dir_all(dir.join("inner")).unwrap();
    assert!(move_aside_with(&dir, &root.join("d"), failing).is_err());
    assert!(dir.join("inner").exists());
    let link = root.join("link");
    symlink(&dir, &link).unwrap();
    assert!(move_aside_with(&link, &root.join("l"), failing).is_err());
    assert!(link.symlink_metadata().is_ok());
    assert!(!root.join("l").exists());
}

#[test]
fn a_replaced_git_dir_and_a_new_submodule_dir_count_as_new() {
    let scratch = Scratch::new("git-replaced");
    let root = scratch.path().join("work");
    let home = scratch.path().join("home");
    let old = git_dir(&root, "r");
    let start = Start::capture(&root);
    fs::rename(&old, root.join("r/old-git")).unwrap();
    let fresh = git_dir(&root, "r");
    fs::write(fresh.join("hooks/post-merge"), "x").unwrap();
    let module = fresh.join("modules/m");
    fs::create_dir_all(module.join("hooks")).unwrap();
    fs::write(module.join("HEAD"), "x").unwrap();
    fs::write(module.join("hooks/pre-push"), "x").unwrap();
    let lines = start.check(home.to_str().unwrap(), "run-2");
    assert!(!fresh.join("hooks/post-merge").exists());
    assert!(!module.join("hooks/pre-push").exists());
    assert!(lines.join("\n").contains("post-merge"));
}

#[test]
fn a_clean_run_is_silent() {
    let scratch = Scratch::new("git-clean");
    let root = scratch.path().join("work");
    git_dir(&root, "a");
    let start = Start::capture(&root);
    git_dir(&root, "b");
    assert!(
        start
            .check(scratch.path().to_str().unwrap(), "run-3")
            .is_empty()
    );
}

#[test]
fn a_new_git_file_pointing_at_a_new_dir_is_scanned() {
    let scratch = Scratch::new("git-link");
    let root = scratch.path().join("work");
    fs::create_dir_all(&root).unwrap();
    let start = Start::capture(&root);
    let target = root.join("store");
    fs::create_dir_all(target.join("hooks")).unwrap();
    fs::write(target.join("hooks/pre-commit"), "x").unwrap();
    fs::create_dir_all(root.join("sub")).unwrap();
    fs::write(root.join("sub/.git"), "gitdir: ../store\n").unwrap();
    let lines = start.check(scratch.path().to_str().unwrap(), "run-4");
    assert!(lines.iter().any(|l| l.contains("new .git file")));
    assert!(!target.join("hooks/pre-commit").exists());
}

#[test]
fn config_parser_reads_sections_keys_and_one_line_headers() {
    let hits = config_hits(
        "# c\n[core]\n  editor = vi\n  FsMonitor=x\n  autocrlf = true\n[remote \"o\"]\n  url = u\n[include]\n  path = p\n[alias]\n  x = !sh\n  y = status\n[core] pager = less\n",
    );
    assert_eq!(
        hits,
        [
            "core.editor",
            "core.fsmonitor",
            "include.path",
            "alias.x",
            "core.pager"
        ]
    );
}

#[test]
fn config_parser_flags_every_key_that_runs_a_program() {
    let hits = config_hits(
        "[credential \"https://x\"]\n helper = h\n[core]\n gitProxy = p\n askPass = a\n[gpg]\n program = g\n[gpg \"ssh\"]\n program = s\n[diff]\n external = e\n[diff \"d\"]\n command = c\n[merge \"m\"]\n driver = d\n[remote \"o\"]\n uploadpack = u\n receivepack = r\n url = u\n[uploadpack]\n packObjectsHook = h\n",
    );
    assert_eq!(
        hits,
        [
            "credential.helper",
            "core.gitproxy",
            "core.askpass",
            "gpg.program",
            "gpg.program",
            "diff.external",
            "diff.command",
            "merge.driver",
            "remote.uploadpack",
            "remote.receivepack",
            "uploadpack.packobjectshook"
        ]
    );
}
