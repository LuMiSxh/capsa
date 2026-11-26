// SPDX-License-Identifier: MPL-2.0
use crate::common::*;
use std::path::{Path, PathBuf};

const GREET: &str = "export const description = \"Greets\";\nexport const params = {who: \"name\"};\nexport default async function (args) { return \"hello \" + args.who; }\n";

/// A git repository with its own config, state and home directories, so nothing reaches the
/// user's real scripts or logs.
struct Sandbox {
    dir: tempfile::TempDir,
    env: Vec<(&'static str, String)>,
}

impl Sandbox {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("repo/.git")).unwrap();
        std::fs::create_dir_all(dir.path().join("home")).unwrap();
        let path = |p: &str| dir.path().join(p).display().to_string();
        let env = vec![
            ("HOME", path("home")),
            ("ACUS_CONFIG", path("home/acus.toml")),
            ("XDG_STATE_HOME", path("state")),
            ("LOCALAPPDATA", path("state")),
        ];
        Sandbox { dir, env }
    }

    fn repo(&self) -> PathBuf {
        self.dir.path().join("repo")
    }

    fn acus(&self, args: &[&str], stdin: &str) -> (i32, String, String) {
        let env: Vec<(&str, &str)> = self.env.iter().map(|(k, v)| (*k, v.as_str())).collect();
        acus_in_env(&self.repo(), &[&["script"], args].concat(), stdin, &env)
    }

    fn save(&self, name: &str, source: &str) -> (i32, String, String) {
        self.acus(&["save", name, "--note", "a note"], source)
    }

    fn session_start(&self) -> String {
        let env: Vec<(&str, &str)> = self.env.iter().map(|(k, v)| (*k, v.as_str())).collect();
        let input = serde_json::json!({
            "hook_event_name": "SessionStart",
            "session_id": "s",
            "cwd": self.repo().display().to_string(),
        });
        let (code, out, err) = acus_in_env(&self.repo(), &["hook"], &input.to_string(), &env);
        assert_eq!(code, 0, "{err}");
        out
    }

    fn script_log(&self) -> PathBuf {
        let state = self.dir.path().join("state/acus");
        if cfg!(windows) {
            state.join("state/script.jsonl")
        } else {
            state.join("script.jsonl")
        }
    }

    fn meta(&self, name: &str) -> PathBuf {
        self.repo()
            .join(".acus/scripts")
            .join(name)
            .join("meta.json")
    }
}

#[test]
fn a_saved_script_runs_with_its_arguments_and_a_pin_keeps_the_old_code() {
    let sb = Sandbox::new();
    let (code, out, err) = sb.save("greet", GREET);
    assert_eq!((code, out.as_str()), (0, "saved greet@1 (repo)\n"), "{err}");
    assert_eq!(
        sb.acus(&["greet", "who=ada"], ""),
        (0, "hello ada\n".to_owned(), String::new())
    );
    let (code, out, _) = sb.save("greet", &GREET.replace("hello", "hi"));
    assert_eq!((code, out.as_str()), (0, "saved greet@2 (repo)\n"));
    assert_eq!(sb.acus(&["greet", "who=ada"], "").1, "hi ada\n");
    assert_eq!(sb.acus(&["greet@1", "who=ada"], "").1, "hello ada\n");
    let (code, out, _) = sb.acus(&["log", "greet"], "");
    assert_eq!(code, 0);
    assert_eq!(out.lines().count(), 2, "{out}");
}

#[test]
fn saving_unchanged_content_creates_no_version() {
    let sb = Sandbox::new();
    sb.save("greet", GREET);
    let (code, out, _) = sb.save("greet", GREET);
    assert_eq!(code, 0);
    assert!(out.starts_with("unchanged: greet@1"), "{out}");
    assert!(!sb.repo().join(".acus/scripts/greet/v2.js").exists());

    let orphan = Sandbox::new();
    let dir = orphan.repo().join(".acus/scripts/greet");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("v1.js"), "orphan").unwrap();
    let (code, out, err) = orphan.save("greet", GREET);
    assert_eq!((code, out.as_str()), (0, "saved greet@2 (repo)\n"), "{err}");
    assert_eq!(orphan.acus(&["greet", "who=o"], "").1, "hello o\n");
}

#[test]
fn save_refuses_a_missing_export_a_missing_note_and_a_reserved_name() {
    let sb = Sandbox::new();
    let no_params = GREET.replace("export const params = {who: \"name\"};\n", "");
    let no_default = GREET.replace(
        "export default async function",
        "export const run = async function",
    );
    for (source, lacks) in [
        (&no_params, "`export const params`"),
        (&no_default, "`export default"),
    ] {
        let (code, _, err) = sb.save("greet", source);
        assert_eq!(code, 2, "{err}");
        assert!(err.contains(lacks), "{err}");
    }
    let (code, _, _) = sb.acus(&["save", "greet"], GREET);
    assert_eq!(code, 2);
    let (code, _, err) = sb.save("list", GREET);
    assert_eq!(code, 2);
    assert!(err.contains("reserved"), "{err}");
    assert_eq!(sb.acus(&["list"], "").0, 1);
}

#[test]
fn a_repo_script_overrides_a_global_one_of_the_same_name() {
    let sb = Sandbox::new();
    let (code, out, err) = sb.acus(&["save", "greet", "--note", "n", "--global"], GREET);
    assert_eq!((code, out.as_str()), (0, "saved greet@1 (user)\n"), "{err}");
    assert_eq!(sb.acus(&["greet", "who=g"], "").1, "hello g\n");
    sb.save("greet", &GREET.replace("hello", "repo"));
    assert_eq!(sb.acus(&["greet", "who=g"], "").1, "repo g\n");
    let (_, out, _) = sb.acus(&["list", "--json"], "");
    assert!(
        out.contains("\"scope\":\"repo\"") && out.contains("\"scope\":\"user\""),
        "{out}"
    );
}

#[test]
fn a_script_imports_a_pinned_repo_script_and_refuses_relative_imports() {
    let sb = Sandbox::new();
    let base = "export const description = \"base\";\nexport const params = {};\nexport default async function () { return \"base1\"; }\n";
    sb.save("base", base);
    sb.save("base", &base.replace("base1", "base2"));
    let user = "import base from \"repo:base@1\";\nexport const description = \"uses base\";\nexport const params = {};\nexport default async function () { return await base(); }\n";
    let (code, _, err) = sb.save("user", user);
    assert_eq!(code, 0, "{err}");
    assert_eq!(sb.acus(&["user"], "").1, "base1\n");
    let relative = user.replace("repo:base@1", "./base.js");
    let (code, _, err) = sb.save("relative", &relative);
    assert_eq!(code, 2);
    assert!(err.contains("is refused"), "{err}");
}

#[test]
fn names_versions_and_import_specifiers_that_could_escape_are_refused() {
    let sb = Sandbox::new();
    sb.save("greet", GREET);
    let import = |spec: &str| {
        format!(
            "import x from \"{spec}\";\nexport const description = \"d\";\nexport const params = {{}};\nexport default async function () {{}}\n"
        )
    };
    let saves = [
        ("Greet", GREET.to_owned()),
        ("list", GREET.to_owned()),
        ("evil", import("repo:../x")),
        ("evil", import("user:/abs")),
        ("evil", import("repo:greet@0")),
    ];
    for (name, source) in saves {
        let (code, out, err) = sb.save(name, &source);
        assert_eq!(code, 2, "{name}: {out}{err}");
    }
    for words in [["greet@../"], ["greet@0"], ["Greet"], ["show"]] {
        let (code, _, err) = sb.acus(&words, GREET);
        assert_eq!(code, 2, "{words:?}: {err}");
    }
}

#[test]
fn circular_imports_finish() {
    let sb = Sandbox::new();
    let module = |import: &str, value: &str| {
        format!(
            "{import}export const description = \"d\";\nexport const params = {{}};\nexport default async function () {{ return \"{value}\"; }}\n"
        )
    };
    sb.save("a", &module("", "a1"));
    sb.save("b", &module("import a from \"repo:a\";\n", "b"));
    let (code, _, err) = sb.save("a", &module("import b from \"repo:b\";\n", "a2"));
    assert_eq!(code, 0, "{err}");
    assert_eq!(sb.acus(&["a"], ""), (0, "a2\n".to_owned(), String::new()));
}

#[test]
fn a_failing_saved_script_is_logged_as_not_ok() {
    let sb = Sandbox::new();
    let failing = GREET.replace(
        "return \"hello \" + args.who;",
        "throw new Error(\"boom\");",
    );
    sb.save("greet", &failing);
    let (code, _, err) = sb.acus(&["greet"], "");
    assert_eq!(code, 2, "{err}");
    let log = std::fs::read_to_string(sb.script_log()).unwrap();
    assert!(
        log.contains("\"name\":\"greet\"") && log.contains("\"ok\":false"),
        "{log}"
    );
}

/// The last line of the script log, parsed: the run the call just made.
fn last_run(sb: &Sandbox) -> serde_json::Value {
    let log = std::fs::read_to_string(sb.script_log()).unwrap();
    serde_json::from_str(log.lines().last().unwrap()).unwrap()
}

#[test]
fn an_inline_program_is_logged_without_a_name() {
    let sb = Sandbox::new();
    let code = "await find('needle'); await find('other'); return 1";
    assert_eq!(
        sb.acus(&["-e", code], ""),
        (0, "1\n".to_owned(), String::new())
    );
    assert_eq!(sb.acus(&["list"], "").0, 1);
    let run = last_run(&sb);
    assert_eq!(run["event"], "run");
    assert_eq!(run["inline"], true);
    assert_eq!(
        (run["ok"].as_bool(), run["calls"].as_u64()),
        (Some(true), Some(2))
    );
    assert!(run.get("name").is_none(), "{run}");
    assert!(run["cwd"].is_string() && run["ms"].is_u64(), "{run}");

    let (code, _, err) = sb.acus(&[], "throw new Error(\"boom\");");
    assert_eq!(code, 2, "{err}");
    let run = last_run(&sb);
    assert_eq!(
        (run["ok"].as_bool(), run["calls"].as_u64()),
        (Some(false), Some(0))
    );
    assert!(run.get("name").is_none(), "{run}");
}

#[test]
fn the_first_save_makes_acus_ignore_itself_without_touching_the_repo_gitignore() {
    let sb = Sandbox::new();
    std::fs::write(sb.repo().join(".gitignore"), "target\n").unwrap();
    sb.save("greet", GREET);
    let read = |p: &Path| std::fs::read_to_string(p).unwrap();
    assert_eq!(read(&sb.repo().join(".acus/.gitignore")), "*\n");
    assert_eq!(read(&sb.repo().join(".gitignore")), "target\n");
}

#[test]
fn session_start_names_used_scripts_and_leaves_out_stale_ones() {
    let sb = Sandbox::new();
    assert!(!sb.session_start().contains("scripts here"));
    sb.save("alpha", GREET);
    sb.save("beta", GREET);
    sb.acus(&["beta", "who=x"], "");
    assert!(
        sb.session_start()
            .ends_with("acus scripts here: beta, alpha\n")
    );
    for name in ["alpha", "beta"] {
        let text = std::fs::read_to_string(sb.meta(name)).unwrap();
        let mut meta: serde_json::Value = serde_json::from_str(&text).unwrap();
        meta["versions"][0]["at"] = 1.into();
        std::fs::write(sb.meta(name), meta.to_string()).unwrap();
    }
    let stale_log = sb.script_log();
    std::fs::write(stale_log, "").unwrap();
    assert!(!sb.session_start().contains("scripts here"));
}
