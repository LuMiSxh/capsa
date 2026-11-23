// SPDX-License-Identifier: MPL-2.0
use crate::common::*;
use std::path::PathBuf;

/// A git repository with its own home, Claude config and acus config, and one MCP server `fake`
/// defined in the user scope.
struct Sandbox {
    dir: tempfile::TempDir,
    pid_file: PathBuf,
}

impl Sandbox {
    fn new(config: &str) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path();
        std::fs::create_dir_all(path.join("repo/.git")).unwrap();
        std::fs::create_dir_all(path.join("claude")).unwrap();
        let fake = std::path::Path::new(env!("CARGO_BIN_EXE_acus"))
            .parent()
            .unwrap()
            .join(format!("examples/fake_mcp{}", std::env::consts::EXE_SUFFIX));
        let pid_file = path.join("fake.pid");
        let claude = serde_json::json!({"mcpServers": {"fake": {
            "command": fake.display().to_string(),
            "env": {"FAKE_PID_FILE": pid_file.display().to_string()},
        }}});
        std::fs::write(path.join("claude/.claude.json"), claude.to_string()).unwrap();
        std::fs::write(path.join("acus.toml"), config).unwrap();
        Sandbox { dir, pid_file }
    }

    fn repo(&self) -> PathBuf {
        self.dir.path().join("repo")
    }

    fn script(&self, code: &str) -> (i32, String, String) {
        self.acus(&[], code)
    }

    fn acus(&self, args: &[&str], code: &str) -> (i32, String, String) {
        let path = |p: &str| self.dir.path().join(p).display().to_string();
        let env = [
            ("HOME", path("home")),
            ("CLAUDE_CONFIG_DIR", path("claude")),
            ("ACUS_CONFIG", path("acus.toml")),
            ("ACUS_MANAGED_SETTINGS", path("managed.json")),
            ("XDG_STATE_HOME", path("state")),
            ("LOCALAPPDATA", path("state")),
        ];
        let env: Vec<(&str, &str)> = env.iter().map(|(k, v)| (*k, v.as_str())).collect();
        acus_in_env(&self.repo(), &[&["script"], args].concat(), code, &env)
    }

    fn settings(&self, json: &str) {
        std::fs::create_dir_all(self.repo().join(".claude")).unwrap();
        std::fs::write(self.repo().join(".claude/settings.json"), json).unwrap();
    }
}

const ENABLED: &str = "[script]\nmcp = [\"fake\"]\ntimeout = 3\n";

#[test]
fn imported_server_tools_return_their_results() {
    let sandbox = Sandbox::new(ENABLED);
    let code = r#"
import fake from "mcp:fake";
const [text, structured, raw] = await Promise.all([
  fake.echo({text: "hi"}), fake.structured(), fake.image(),
]);
return {text, structured, raw};
"#;
    let expected = concat!(
        r#"{"text":"echo: hi","structured":{"n":1},"#,
        r#""raw":[{"type":"image","data":"AA==","mimeType":"image/png"}]}"#,
        "\n"
    );
    assert_eq!(
        sandbox.script(code),
        (0, expected.to_owned(), String::new())
    );
}

#[test]
fn tools_export_lists_the_server_tools() {
    let sandbox = Sandbox::new(ENABLED);
    let code =
        "import {tools} from \"mcp:fake\"; return tools.map(t => t.name + \": \" + t.description)";
    let (code, out, err) = sandbox.script(code);
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("echo: Echoes text"), "{out}");
    assert!(out.contains("hang: Never answers"), "{out}");
}

#[test]
fn a_tool_error_throws_its_text() {
    let sandbox = Sandbox::new(ENABLED);
    let caught = "import fake from \"mcp:fake\"\ntry { await fake.fail({}); } catch (e) { return e.message; }";
    assert_eq!(
        sandbox.script(caught),
        (0, "boom\n".to_owned(), String::new())
    );
    let (code, _, err) = sandbox.script("import fake from \"mcp:fake\"; await fake.fail({});");
    assert_eq!(code, 2);
    assert!(err.contains("boom"), "{err}");
}

#[test]
fn a_server_that_is_not_enabled_is_refused_with_the_config_key() {
    let sandbox = Sandbox::new("");
    let (code, _, err) = sandbox.script("import fake from \"mcp:fake\"; return 1");
    assert_eq!(code, 2);
    assert!(err.contains("\"fake\" is not enabled for scripts"), "{err}");
    assert!(err.contains("`[script]` `mcp = [\"fake\"]`"), "{err}");
    assert!(!sandbox.pid_file.exists(), "the server must not start");
}

#[test]
fn permission_rules_refuse_denied_and_ask_tools() {
    let sandbox = Sandbox::new(ENABLED);
    sandbox
        .settings(r#"{"permissions": {"deny": ["mcp__fake__fail"], "ask": ["mcp__fake__image"]}}"#);
    let ok = "import fake from \"mcp:fake\"; return await fake.echo({text: \"ok\"});";
    assert_eq!(
        sandbox.script(ok),
        (0, "echo: ok\n".to_owned(), String::new())
    );
    let (code, _, err) = sandbox.script("import fake from \"mcp:fake\"; await fake.fail({});");
    assert_eq!(code, 2);
    assert!(
        err.contains("mcp__fake__fail is denied by `mcp__fake__fail` in permissions.deny"),
        "{err}"
    );
    let (code, _, err) = sandbox.script("import fake from \"mcp:fake\"; await fake.image({});");
    assert_eq!(code, 2);
    assert!(err.contains("needs approval"), "{err}");

    let sandbox = Sandbox::new(ENABLED);
    sandbox.settings(r#"{"permissions": {"deny": ["mcp__fake"]}}"#);
    let (code, _, err) = sandbox.script("import fake from \"mcp:fake\"; return 1");
    assert_eq!(code, 2);
    assert!(err.contains("the MCP server fake is denied"), "{err}");
    assert!(!sandbox.pid_file.exists(), "a denied server must not start");
}

#[test]
fn managed_settings_deny_a_tool_and_save_starts_no_server() {
    let sandbox = Sandbox::new(ENABLED);
    std::fs::write(
        sandbox.dir.path().join("managed.json"),
        r#"{"permissions": {"deny": ["mcp__fake__echo"]}}"#,
    )
    .unwrap();
    let (code, _, err) =
        sandbox.script("import fake from \"mcp:fake\"; await fake.echo({text: \"x\"});");
    assert_eq!(code, 2);
    assert!(err.contains("mcp__fake__echo is denied"), "{err}");

    let source = "import fake from \"mcp:fake\";\nexport const description = \"d\";\nexport const params = {};\nexport default async function () { return await fake.echo({text: \"x\"}); }\n";
    let sandbox = Sandbox::new(ENABLED);
    let (code, _, err) = sandbox.acus(&["save", "uses-fake", "--note", "n"], source);
    assert_eq!(code, 0, "{err}");
    assert!(
        !sandbox.pid_file.exists(),
        "saving must not start the server"
    );

    let sandbox = Sandbox::new("");
    let (code, _, err) = sandbox.acus(&["save", "uses-fake", "--note", "n"], source);
    assert_eq!(code, 2);
    assert!(err.contains("not enabled for scripts"), "{err}");
}

#[cfg(unix)]
#[test]
fn the_deadline_kills_a_hanging_server() {
    use std::time::{Duration, Instant};
    let sandbox = Sandbox::new("[script]\nmcp = [\"fake\"]\ntimeout = 2\n");
    let started = Instant::now();
    let (code, _, err) = sandbox.script("import fake from \"mcp:fake\"; await fake.hang({});");
    assert_eq!(code, 2, "{err}");
    assert!(
        started.elapsed() < Duration::from_secs(8),
        "{:?}",
        started.elapsed()
    );
    let pid = std::fs::read_to_string(&sandbox.pid_file).unwrap();
    let alive = std::process::Command::new("kill")
        .args(["-0", pid.trim()])
        .stderr(std::process::Stdio::null())
        .status()
        .unwrap()
        .success();
    assert!(!alive, "server {pid} is still running");
}
