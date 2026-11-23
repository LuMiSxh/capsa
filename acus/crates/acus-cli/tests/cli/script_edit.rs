// SPDX-License-Identifier: MPL-2.0
use crate::common::*;
use std::path::PathBuf;

/// A git repository with its own home and Claude config, so no real settings file is read.
struct Sandbox {
    dir: tempfile::TempDir,
}

impl Sandbox {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("repo/.git")).unwrap();
        std::fs::create_dir_all(dir.path().join("claude")).unwrap();
        std::fs::write(dir.path().join("repo/a.txt"), "one\ntwo\ntwo\n").unwrap();
        std::fs::write(dir.path().join("repo/b.txt"), "left\nright\n").unwrap();
        std::fs::write(dir.path().join("repo/secret.txt"), "hidden\n").unwrap();
        Sandbox { dir }
    }

    fn repo(&self) -> PathBuf {
        self.dir.path().join("repo")
    }

    fn file(&self, name: &str) -> String {
        std::fs::read_to_string(self.repo().join(name)).unwrap()
    }

    fn deny(&self, rule: &str) {
        std::fs::create_dir_all(self.repo().join(".claude")).unwrap();
        std::fs::write(
            self.repo().join(".claude/settings.json"),
            format!(r#"{{"permissions": {{"deny": ["{rule}"]}}}}"#),
        )
        .unwrap();
    }

    fn script(&self, code: &str) -> (i32, String, String) {
        let path = |p: &str| self.dir.path().join(p).display().to_string();
        let env = [
            ("HOME", path("home")),
            ("USERPROFILE", path("home")),
            ("CLAUDE_CONFIG_DIR", path("claude")),
            ("ACUS_MANAGED_SETTINGS", path("managed.json")),
            ("XDG_STATE_HOME", path("state")),
            ("LOCALAPPDATA", path("state")),
        ];
        let env: Vec<(&str, &str)> = env.iter().map(|(k, v)| (*k, v.as_str())).collect();
        acus_in_env(&self.repo(), &["script"], code, &env)
    }
}

#[test]
fn write_creates_a_file_and_prints_the_written_lines() {
    let sandbox = Sandbox::new();
    let (code, out, err) =
        sandbox.script("return (await write('new.txt', 'alpha\\nbeta\\n')).changes;");
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("A new.txt +2"), "{out}");
    assert_eq!(sandbox.file("new.txt"), "alpha\nbeta\n");
}

#[test]
fn write_replaces_a_file_that_exists() {
    let sandbox = Sandbox::new();
    let (code, out, err) = sandbox.script("return (await write('b.txt', 'only\\n')).changes;");
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("M b.txt +1 -2"), "{out}");
    assert_eq!(sandbox.file("b.txt"), "only\n");
}

#[test]
fn edit_replaces_a_unique_text() {
    let sandbox = Sandbox::new();
    let (code, out, err) = sandbox.script("return (await edit('a.txt', 'one', 'uno')).regions;");
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("uno"), "{out}");
    assert_eq!(sandbox.file("a.txt"), "uno\ntwo\ntwo\n");
}

#[test]
fn edit_refuses_an_ambiguous_text_unless_all_is_set() {
    let sandbox = Sandbox::new();
    let (code, _, err) = sandbox.script("return await edit('a.txt', 'two', 'zwei');");
    assert_eq!(code, 2, "{err}");
    assert_eq!(sandbox.file("a.txt"), "one\ntwo\ntwo\n");
    let (code, _, err) = sandbox.script("return await edit('a.txt', 'two', 'zwei', {all: true});");
    assert_eq!(code, 0, "{err}");
    assert_eq!(sandbox.file("a.txt"), "one\nzwei\nzwei\n");
}

#[test]
fn an_array_patch_applies_every_edit() {
    let sandbox = Sandbox::new();
    let code = "return await patch([{path: 'a.txt', old: 'one', new: '1'}, {path: 'b.txt', old: 'left', new: 'L'}]);";
    let (status, _, err) = sandbox.script(code);
    assert_eq!(status, 0, "{err}");
    assert_eq!(sandbox.file("a.txt"), "1\ntwo\ntwo\n");
    assert_eq!(sandbox.file("b.txt"), "L\nright\n");
}

#[test]
fn an_array_patch_with_a_failing_edit_writes_nothing() {
    let sandbox = Sandbox::new();
    let code = "return await patch([{path: 'a.txt', old: 'one', new: '1'}, {path: 'b.txt', old: 'missing', new: 'x'}]);";
    let (status, _, err) = sandbox.script(code);
    assert_eq!(status, 2, "{err}");
    assert_eq!(sandbox.file("a.txt"), "one\ntwo\ntwo\n");
    assert_eq!(sandbox.file("b.txt"), "left\nright\n");
}

#[test]
fn a_path_outside_the_repository_is_refused() {
    let sandbox = Sandbox::new();
    let outside = sandbox.dir.path().join("outside.txt");
    for code in [
        "return await write('../outside.txt', 'x');".to_owned(),
        "return await edit('../outside.txt', 'a', 'b');".to_owned(),
        format!(
            "return await write({:?}, 'x');",
            outside.display().to_string()
        ),
    ] {
        let (status, _, err) = sandbox.script(&code);
        assert_eq!(status, 2, "{err}");
        assert!(err.contains("outside the repository"), "{err}");
    }
    assert!(!outside.exists());
}

#[test]
fn an_edit_deny_refuses_write_edit_and_patch() {
    let sandbox = Sandbox::new();
    sandbox.deny("Edit(./secret.txt)");
    let patch = "return await patch(`*** Begin Patch\n*** Update File: secret.txt\n<<<<<<< SEARCH\nhidden\n=======\nshown\n>>>>>>> REPLACE\n*** End Patch`);";
    let replace_all = "return await patch(`*** Begin Patch\n*** Replace All: .\n<<<<<<< SEARCH\nhidden\n=======\nshown\n>>>>>>> REPLACE\n*** End Patch`);";
    for code in [
        "return await write('secret.txt', 'x');",
        "return await edit('secret.txt', 'hidden', 'shown');",
        "return await patch([{path: 'secret.txt', old: 'hidden', new: 'shown'}]);",
        patch,
        replace_all,
    ] {
        let (status, _, err) = sandbox.script(code);
        assert_eq!(status, 2, "{code}: {err}");
        assert!(
            err.contains("is denied by `Edit(./secret.txt)` in permissions.deny"),
            "{code}: {err}"
        );
        assert_eq!(sandbox.file("secret.txt"), "hidden\n");
    }
    let (status, _, err) = sandbox.script("return await edit('a.txt', 'one', 'uno');");
    assert_eq!(status, 0, "{err}");
}

#[test]
fn two_edits_then_a_find_keep_the_program_order() {
    let sandbox = Sandbox::new();
    let code = "const first = edit('a.txt', 'one', 'uno');
const second = edit('b.txt', 'left', 'uno');
const hits = find('uno');
await first;
await second;
return (await hits).length;";
    let (status, out, err) = sandbox.script(code);
    assert_eq!(status, 0, "{err}");
    assert_eq!(out.trim(), "2", "{out}");
}

#[test]
fn an_edit_clears_the_results_remembered_before_it() {
    let sandbox = Sandbox::new();
    let code = "const before = (await find('one')).length;
const again = (await find('one')).length;
await edit('a.txt', 'one', 'uno');
const after = (await find('one')).length;
return [before, again, after];";
    let (status, out, err) = sandbox.script(code);
    assert_eq!(status, 0, "{err}");
    let compact: String = out.split_whitespace().collect();
    assert!(
        compact.contains("1,1,0") || compact.contains("[1,1,0]"),
        "{out}"
    );
}

#[test]
fn write_keeps_the_text_byte_exact() {
    let sandbox = Sandbox::new();
    for (name, text) in [
        ("plain.txt", "abc"),
        ("crlf.txt", "a\r\nb\r\n"),
        ("blank.txt", "a\n\n"),
        ("replaced.txt", "xyz"),
    ] {
        std::fs::write(sandbox.repo().join("replaced.txt"), "old\n").unwrap();
        let code = format!("return (await write('{name}', {text:?})).changes;");
        let (status, _, err) = sandbox.script(&code);
        assert_eq!(status, 0, "{err}");
        assert_eq!(sandbox.file(name), text, "{name}");
    }
}

#[test]
fn edit_replaces_text_with_a_newline_as_a_plain_substring() {
    let sandbox = Sandbox::new();
    std::fs::write(sandbox.repo().join("c.txt"), "ab\ncd\nab\n").unwrap();
    let (status, _, err) = sandbox.script("return await edit('c.txt', 'ab\\ncd', 'X');");
    assert_eq!(status, 0, "{err}");
    assert_eq!(sandbox.file("c.txt"), "X\nab\n");
    let (status, _, err) = sandbox.script("return await edit('a.txt', 'one\\n', 'uno ');");
    assert_eq!(status, 0, "{err}");
    assert_eq!(sandbox.file("a.txt"), "uno two\ntwo\n");
}

#[test]
fn a_string_patch_outside_the_repository_is_refused() {
    let sandbox = Sandbox::new();
    let outside = sandbox.dir.path().join("outside.txt");
    let patch =
        "return await patch(`*** Begin Patch\n*** Add File: ../outside.txt\n+x\n*** End Patch`);";
    let (status, _, err) = sandbox.script(patch);
    assert_eq!(status, 2, "{err}");
    assert!(err.contains("outside the repository"), "{err}");
    assert!(!outside.exists());
}

#[test]
fn a_dotdot_after_a_missing_directory_does_not_escape() {
    let sandbox = Sandbox::new();
    let (status, _, err) = sandbox.script("return await write('missing/../../outside.txt', 'x');");
    assert_eq!(status, 2, "{err}");
    assert!(err.contains("outside the repository"), "{err}");
    assert!(!sandbox.dir.path().join("outside.txt").exists());
}

#[cfg(unix)]
#[test]
fn a_dotdot_after_a_symlink_leaves_through_the_link_and_is_refused() {
    let sandbox = Sandbox::new();
    let far = sandbox.dir.path().join("far/sub");
    std::fs::create_dir_all(&far).unwrap();
    std::os::unix::fs::symlink(&far, sandbox.repo().join("link")).unwrap();
    let patch =
        "return await patch(`*** Begin Patch\n*** Add File: link/../x.txt\n+x\n*** End Patch`);";
    for code in [
        "return await write('link/../x.txt', 'x');",
        "return await edit('link/../x.txt', 'a', 'b');",
        patch,
    ] {
        let (status, _, err) = sandbox.script(code);
        assert_eq!(status, 2, "{code}: {err}");
        assert!(err.contains("outside the repository"), "{code}: {err}");
    }
    assert!(!sandbox.dir.path().join("far/x.txt").exists());
}

#[test]
fn a_run_clears_the_results_remembered_before_it() {
    let sandbox = Sandbox::new();
    let code = "const before = (await find('one')).length;
await run('echo uno > a.txt');
const after = (await find('one')).length;
return [before, after];";
    let (status, out, err) = sandbox.script(code);
    assert_eq!(status, 0, "{err}");
    let compact: String = out.split_whitespace().collect();
    assert!(compact.contains("1,0"), "{out}");
}

#[test]
fn read_rules_apply_to_the_files_show_outline_blame_follow_and_find_name() {
    let sandbox = Sandbox::new();
    sandbox.deny("Read(./secret.txt)");
    for code in [
        "return await show('secret.txt');",
        "return await show(['a.txt', 'secret.txt:1-1']);",
        "return await show('secret.txt#x');",
        "return await outline('secret.txt');",
        "return await blame('secret.txt:1-1');",
        "return await follow('secret.txt#x');",
        "return await find('hidden', {paths: ['secret.txt']});",
    ] {
        let (status, _, err) = sandbox.script(code);
        assert_eq!(status, 2, "{code}: {err}");
        assert!(
            err.contains("is denied by `Read(./secret.txt)` in permissions.deny"),
            "{code}: {err}"
        );
    }
    let (status, _, err) = sandbox.script("return await show('a.txt');");
    assert_eq!(status, 0, "{err}");
}
