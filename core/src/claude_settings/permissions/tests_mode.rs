// SPDX-License-Identifier: MPL-2.0
use super::{Mode, Permissions, Refusal};
use std::path::{Path, PathBuf};

struct Sandbox {
    dir: tempfile::TempDir,
}

impl Sandbox {
    fn new(settings: &str) -> Self {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("repo/.git")).unwrap();
        std::fs::create_dir_all(dir.path().join("repo/.claude")).unwrap();
        std::fs::write(dir.path().join("repo/.claude/settings.json"), settings).unwrap();
        Sandbox { dir }
    }

    fn root(&self) -> PathBuf {
        self.dir.path().join("repo").canonicalize().unwrap()
    }

    fn perms(&self, mode: Option<Mode>, strict: bool) -> Permissions {
        let files = vec![
            self.root().join(".claude/settings.json"),
            self.root().join(".claude/settings.local.json"),
        ];
        Permissions::with_files(files, self.root(), self.root(), None)
            .with_mode(mode)
            .with_strict(strict)
    }
}

/// Makes `link` another name for `target`: a symlink, or a junction on Windows.
fn alias(target: &Path, link: &Path) -> bool {
    #[cfg(unix)]
    return std::os::unix::fs::symlink(target, link).is_ok();
    #[cfg(windows)]
    return std::process::Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(link)
        .arg(target)
        .output()
        .is_ok_and(|o| o.status.success());
}

#[test]
fn a_working_directory_under_another_name_still_counts_as_local() {
    let sandbox = Sandbox::new("{}");
    let link = sandbox.dir.path().join("alias");
    if !alias(&sandbox.root(), &link) {
        return;
    }
    let p = Permissions::with_files(Vec::new(), link.clone(), link.clone(), None)
        .with_mode(Some(Mode::AcceptEdits))
        .with_strict(true);
    for file in [link.join("new.txt"), link.join("src/deep/new.txt")] {
        assert_eq!(message(p.check_edit(&file)), None, "{}", file.display());
    }
    let outside = sandbox.dir.path().join("outside.txt");
    assert!(p.check_edit(&outside).is_err());
    assert!(p.check_edit(&link.join("../outside.txt")).is_err());
}

fn message(result: Result<(), Refusal>) -> Option<String> {
    result.err().map(|e| e.message)
}

/// The five kinds of call, each as `Ok` or the refusal text.
fn calls(p: &Permissions) -> [Option<String>; 5] {
    [
        message(p.check_bash("git status")),
        message(p.check_edit(Path::new("src/a.rs"))),
        message(p.check_fetch("https://example.com/x")),
        message(p.check_mcp("srv", Some("tool"))),
        message(p.check_read(Path::new("a.txt"))),
    ]
}

const ALLOWS: &str = r#"{"permissions": {"allow": [
    "Bash(git status:*)", "Edit(src/**)", "WebFetch(domain:example.com)", "mcp__srv__tool"
]}}"#;

fn refused_in(text: &Option<String>, what: &str) -> bool {
    text.as_deref().is_some_and(|t| t.contains(what))
}

#[test]
fn no_mode_and_no_strict_keeps_every_call_that_no_rule_names() {
    let s = Sandbox::new("{}");
    assert_eq!(calls(&s.perms(None, false)), [None, None, None, None, None]);
}

#[test]
fn plan_refuses_bash_and_edits_and_lets_fetch_mcp_and_reads_go() {
    let s = Sandbox::new("{}");
    let [bash, edit, fetch, mcp, read] = calls(&s.perms(Some(Mode::Plan), false));
    assert!(refused_in(&bash, "refused in plan mode"), "{bash:?}");
    assert!(refused_in(&edit, "refused in plan mode"), "{edit:?}");
    assert_eq!((fetch, mcp, read), (None, None, None));
}

#[test]
fn plan_names_a_deny_rule_before_the_mode() {
    let s = Sandbox::new(r#"{"permissions": {"deny": ["Bash(git status:*)"]}}"#);
    let bash = message(s.perms(Some(Mode::Plan), false).check_bash("git status"));
    assert!(refused_in(&bash, "is denied by"), "{bash:?}");
}

#[test]
fn dont_ask_refuses_calls_without_an_allow_rule_but_not_reads() {
    let s = Sandbox::new("{}");
    let [bash, edit, fetch, mcp, read] = calls(&s.perms(Some(Mode::DontAsk), false));
    for text in [&bash, &edit, &fetch, &mcp] {
        assert!(refused_in(text, "refused in dontAsk mode"), "{text:?}");
    }
    assert_eq!(read, None);
}

#[test]
fn dont_ask_lets_calls_go_that_an_allow_rule_covers() {
    let s = Sandbox::new(ALLOWS);
    assert_eq!(
        calls(&s.perms(Some(Mode::DontAsk), false)),
        [None, None, None, None, None]
    );
}

#[test]
fn an_allow_rule_covers_only_what_it_names() {
    let s = Sandbox::new(ALLOWS);
    let p = s.perms(Some(Mode::DontAsk), false);
    assert!(p.check_bash("git status -s").is_ok());
    assert!(p.check_bash("git push").is_err());
    assert!(p.check_bash("git status && rm -rf x").is_err());
    assert!(p.check_bash("git status; git status -s").is_ok());
    assert!(p.check_bash("bash -c 'git status && rm x'").is_err());
    assert!(p.check_edit(Path::new("other/a.rs")).is_err());
    assert!(p.check_edit(Path::new("src/../other/a.rs")).is_err());
    assert!(p.check_fetch("https://evil.com/").is_err());
    assert!(p.check_mcp("srv", Some("other")).is_err());
    assert!(p.check_mcp("other", None).is_err());
}

#[test]
fn a_bare_allow_rule_covers_every_call_of_its_tool() {
    let s = Sandbox::new(r#"{"permissions": {"allow": ["Bash", "mcp__srv"]}}"#);
    let p = s.perms(Some(Mode::DontAsk), false);
    assert!(p.check_bash("anything && at all").is_ok());
    assert!(p.check_mcp("srv", Some("tool")).is_ok());
    assert!(p.check_edit(Path::new("src/a.rs")).is_err());
}

#[test]
fn deny_and_ask_rules_beat_an_allow_rule_in_every_mode() {
    let s = Sandbox::new(
        r#"{"permissions": {"allow": ["Bash", "Edit"], "deny": ["Bash(rm:*)"],
            "ask": ["Edit(src/**)"]}}"#,
    );
    for mode in [
        Mode::Default,
        Mode::AcceptEdits,
        Mode::Auto,
        Mode::DontAsk,
        Mode::BypassPermissions,
    ] {
        let p = s.perms(Some(mode), true);
        let deny = message(p.check_bash("rm x"));
        assert!(refused_in(&deny, "is denied by"), "{mode:?} {deny:?}");
        let ask = message(p.check_edit(Path::new("src/a.rs")));
        assert!(refused_in(&ask, "needs approval"), "{mode:?} {ask:?}");
    }
}

#[test]
fn default_auto_and_accept_edits_are_lenient_without_strict() {
    let s = Sandbox::new("{}");
    for mode in [Mode::Default, Mode::Auto, Mode::AcceptEdits] {
        assert_eq!(
            calls(&s.perms(Some(mode), false)),
            [None, None, None, None, None]
        );
    }
}

#[test]
fn strict_refuses_unlisted_calls_in_default_and_auto_with_the_hint() {
    let s = Sandbox::new("{}");
    for mode in [Mode::Default, Mode::Auto] {
        let p = s.perms(Some(mode), true);
        let [bash, edit, fetch, mcp, read] = calls(&p);
        for text in [&bash, &edit, &fetch, &mcp] {
            assert!(
                refused_in(text, &format!("refused in {} mode", mode.as_str())),
                "{text:?}"
            );
        }
        assert_eq!(read, None);
        let hint = p.check_bash("git status").unwrap_err().hint.unwrap();
        assert_eq!(
            hint,
            "no allow rule; add one or run it as its own tool call"
        );
    }
}

#[test]
fn strict_lets_listed_calls_go_in_every_prompting_mode() {
    let s = Sandbox::new(ALLOWS);
    for mode in [Mode::Default, Mode::Auto, Mode::AcceptEdits, Mode::DontAsk] {
        assert_eq!(
            calls(&s.perms(Some(mode), true)),
            [None, None, None, None, None]
        );
    }
}

#[test]
fn strict_accept_edits_allows_edits_inside_the_working_directory_only() {
    let s = Sandbox::new("{}");
    let p = s.perms(Some(Mode::AcceptEdits), true);
    assert!(p.check_edit(Path::new("src/a.rs")).is_ok());
    assert!(p.check_edit(Path::new("../outside.txt")).is_err());
    assert!(p.check_bash("touch x").is_err());
}

const PROTECTED: [&str; 4] = [
    ".git/hooks/pre-commit",
    ".git/config",
    ".claude/settings.json",
    ".claude/settings.local.json",
];

#[test]
fn protected_paths_need_an_allow_rule_in_every_mode_but_bypass() {
    let s = Sandbox::new("{}");
    for (mode, strict) in [
        (Mode::Default, false),
        (Mode::Auto, false),
        (Mode::AcceptEdits, false),
        (Mode::AcceptEdits, true),
        (Mode::Default, true),
        (Mode::DontAsk, false),
    ] {
        let p = s.perms(Some(mode), strict);
        assert!(p.check_edit(Path::new("src/a.rs")).is_ok() || strict || mode == Mode::DontAsk);
        for path in PROTECTED {
            let hint = p.check_edit(Path::new(path)).err().and_then(|e| e.hint);
            if mode == Mode::DontAsk {
                assert!(hint.is_some(), "{mode:?} {path}");
            } else {
                assert_eq!(
                    hint.as_deref(),
                    Some("protected path; run the edit as its own tool call"),
                    "{mode:?} strict={strict} {path}"
                );
            }
        }
    }
    let p = s.perms(Some(Mode::AcceptEdits), true);
    assert!(p.check_edit(Path::new("src/a.rs")).is_ok());
    assert!(p.check_edit(Path::new("src/../.git/hooks/x")).is_err());
    assert!(p.check_edit(Path::new("sub/.GIT/config")).is_err());
}

#[test]
fn protected_paths_go_with_an_allow_rule_or_in_bypass_or_with_no_mode() {
    let s = Sandbox::new(r#"{"permissions": {"allow": ["Edit(.git/**)"]}}"#);
    let p = s.perms(Some(Mode::AcceptEdits), true);
    assert!(p.check_edit(Path::new(".git/hooks/pre-commit")).is_ok());
    assert!(
        p.check_edit(Path::new(".claude/settings.local.json"))
            .is_err()
    );
    let s = Sandbox::new("{}");
    for p in [
        s.perms(Some(Mode::BypassPermissions), true),
        s.perms(None, false),
    ] {
        for path in PROTECTED {
            assert!(p.check_edit(Path::new(path)).is_ok(), "{path}");
        }
    }
}

#[test]
fn bypass_permissions_ignores_strict_and_allow_rules() {
    let s = Sandbox::new("{}");
    assert_eq!(
        calls(&s.perms(Some(Mode::BypassPermissions), true)),
        [None, None, None, None, None]
    );
}

#[test]
fn strict_with_an_unknown_mode_counts_as_default() {
    let s = Sandbox::new("{}");
    let bash = message(s.perms(None, true).check_bash("git status"));
    assert!(refused_in(&bash, "refused in an unknown mode"), "{bash:?}");
}

#[test]
fn plan_still_refuses_with_an_allow_rule() {
    let s = Sandbox::new(ALLOWS);
    let [bash, edit, ..] = calls(&s.perms(Some(Mode::Plan), true));
    assert!(refused_in(&bash, "plan mode") && refused_in(&edit, "plan mode"));
}
