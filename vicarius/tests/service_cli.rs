// SPDX-License-Identifier: MPL-2.0

use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::Path;
use std::process::{Command, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};

/// Where vicarius reads its config under a scratch home: macOS ignores `XDG_CONFIG_HOME`.
const CONFIG_FILE: &str = if cfg!(target_os = "macos") {
    "Library/Application Support/vicarius/config.json"
} else {
    "config/vicarius/config.json"
};

const SENIOR: &str = r#"[{"from":"senior-worker","model":"deepseek-flash"}]"#;

const OPEN: &str = r#"{"started_at":1,"gate":{"open":true,"reason":null,"reason_text":null,"balance":18.4,"currency":"USD","checked_at":1793723161},"counters":{"anthropic":{"requests":3,"refused":0,"input_tokens":10,"cache_creation_tokens":1,"cache_read_tokens":2,"output_tokens":4},"deepseek":{"requests":2,"refused":1,"input_tokens":100,"cache_creation_tokens":0,"cache_read_tokens":50,"output_tokens":20}},"agents":{"entries":[{"agent":"senior-worker","file":"senior-worker-ds.md","outcome":"written"}]},"refresh":{"last_ok_at":1793723161,"last_error":null,"next_at":1793723761}}"#;

const ONE_AGENT_LINE: &str = r#"{"hookSpecificOutput":{"hookEventName":"SessionStart","additionalContext":"vicarius: DeepSeek agents available (balance $18.40): senior-worker-ds (deepseek-flash, variant of senior-worker). Use a healthy mix with their Claude source agents: a -ds agent for well-scoped tasks with a clear check, the Claude agent for work that needs judgement or after a -ds result needed rework; report the DeepSeek cost afterwards (vicarius status) and do not pass a model parameter when spawning one."}}"#;

const SESSION_START: &str = r#"{"hook_event_name":"SessionStart","session_id":"s1"}"#;

/// Builds a vicarius command with `home` as the user profile, so that its config and settings go
/// under `home`.
fn base(home: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_vicarius"));
    command
        .env("HOME", home)
        .env("USERPROFILE", home)
        .env("XDG_CONFIG_HOME", home.join("config"))
        .env("APPDATA", home.join("config"))
        .env_remove("CLAUDE_CONFIG_DIR")
        .env("VICARIUS_LAUNCHCTL", "/usr/bin/false");
    command
}

#[expect(
    clippy::expect_used,
    reason = "a vicarius binary that cannot run must fail the test"
)]
fn vicarius(home: &Path, args: &[&str]) -> Output {
    base(home).args(args).output().expect("start vicarius")
}

#[expect(
    clippy::expect_used,
    reason = "a vicarius binary that cannot run must fail the test"
)]
fn vicarius_with_input(home: &Path, args: &[&str], input: &[u8]) -> Output {
    let mut child = base(home)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start vicarius");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(input)
        .expect("write the hook input");
    child.wait_with_output().expect("wait for vicarius")
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// Writes a config with `port` and the `agents` JSON list.
#[expect(
    clippy::unwrap_used,
    reason = "a scratch directory that cannot be written must fail the test"
)]
fn write_config(home: &Path, port: u16, agents: &str) {
    let path = home.join(CONFIG_FILE);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, format!(r#"{{"port": {port}, "agents": {agents}}}"#)).unwrap();
}

/// Serves `routes` on 127.0.0.1 and returns the port. A route is `METHOD /path`, a status and a
/// body; any other request gets a 404. The thread lives until the test process exits.
#[expect(
    clippy::unwrap_used,
    reason = "a socket that cannot be bound must fail the test"
)]
fn serve(routes: Vec<(&'static str, u16, String)>) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    thread::spawn(move || {
        for mut stream in listener.incoming().flatten() {
            let mut head = Vec::new();
            let mut chunk = [0u8; 1024];
            while !head.windows(4).any(|w| w == b"\r\n\r\n") {
                match stream.read(&mut chunk) {
                    Ok(0) | Err(_) => break,
                    Ok(read) => head.extend_from_slice(&chunk[..read]),
                }
            }
            let line = String::from_utf8_lossy(&head)
                .lines()
                .next()
                .unwrap_or("")
                .to_owned();
            let key: Vec<&str> = line.split_whitespace().take(2).collect();
            let key = key.join(" ");
            let (status, body) = routes
                .iter()
                .find(|(route, _, _)| *route == key)
                .map_or((404, String::new()), |(_, status, body)| {
                    (*status, body.clone())
                });
            let reply = format!(
                "HTTP/1.1 {status} reply\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(reply.as_bytes());
        }
    });
    port
}

/// Returns a port that nothing listens on.
#[expect(
    clippy::unwrap_used,
    reason = "a socket that cannot be bound must fail the test"
)]
fn free_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.local_addr().unwrap().port()
}

fn status_route(body: &str) -> Vec<(&'static str, u16, String)> {
    vec![("GET /vicarius/status", 200, body.to_owned())]
}

#[test]
fn hook_prints_the_exact_context_line_for_one_available_agent() {
    let home = tempfile::tempdir().unwrap();
    let port = serve(status_route(OPEN));
    write_config(home.path(), port, SENIOR);
    let out = vicarius_with_input(home.path(), &["hook"], SESSION_START.as_bytes());
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    assert_eq!(text(&out.stdout), format!("{ONE_AGENT_LINE}\n"));
}

#[test]
fn hook_lists_two_agents_comma_separated() {
    let home = tempfile::tempdir().unwrap();
    let body = OPEN.replace(
        r#""entries":[{"agent":"senior-worker","file":"senior-worker-ds.md","outcome":"written"}]"#,
        r#""entries":[{"agent":"senior-worker","file":"senior-worker-ds.md","outcome":"written"},{"agent":"consultant","file":"consultant-ds.md","outcome":"unchanged"}]"#,
    );
    let port = serve(status_route(&body));
    let agents = r#"[{"from":"senior-worker","model":"deepseek-flash"},{"from":"consultant","model":"deepseek-v4-pro"}]"#;
    write_config(home.path(), port, agents);
    let out = vicarius_with_input(home.path(), &["hook"], SESSION_START.as_bytes());
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    let line = text(&out.stdout);
    assert!(
        line.contains("senior-worker-ds (deepseek-flash, variant of senior-worker), consultant-ds (deepseek-v4-pro, variant of consultant)."),
        "{line}"
    );
}

#[test]
fn hook_prints_nothing_when_the_gate_is_closed_or_no_agent_is_ready() {
    let closed = r#"{"gate":{"open":false,"reason":"no_key","reason_text":"no API key configured"},"counters":{},"agents":{"entries":[{"agent":"senior-worker","file":"senior-worker-ds.md","outcome":"written"}]}}"#;
    let no_agents = r#"{"gate":{"open":true,"balance":18.4,"currency":"USD"},"agents":null}"#;
    let conflict = r#"{"gate":{"open":true},"agents":{"entries":[{"agent":"senior-worker","file":"senior-worker-ds.md","outcome":"conflict"}]}}"#;
    let malformed = "not json";
    for body in [closed, no_agents, conflict, malformed] {
        let home = tempfile::tempdir().unwrap();
        let port = serve(status_route(body));
        write_config(home.path(), port, SENIOR);
        let out = vicarius_with_input(home.path(), &["hook"], SESSION_START.as_bytes());
        assert_eq!(out.status.code(), Some(0), "{body}: {}", text(&out.stderr));
        assert!(out.stdout.is_empty(), "{body}: {}", text(&out.stdout));
    }
}

#[test]
fn hook_prints_nothing_and_exits_0_when_the_service_is_down() {
    let home = tempfile::tempdir().unwrap();
    write_config(home.path(), free_port(), SENIOR);
    let started = Instant::now();
    let out = vicarius_with_input(home.path(), &["hook"], SESSION_START.as_bytes());
    let elapsed = started.elapsed();
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    assert!(out.stdout.is_empty());
    assert!(
        elapsed < Duration::from_millis(400),
        "hook took {elapsed:?}"
    );
}

#[test]
fn hook_answers_within_400_ms_when_the_service_is_up() {
    let home = tempfile::tempdir().unwrap();
    let port = serve(status_route(OPEN));
    write_config(home.path(), port, SENIOR);
    let started = Instant::now();
    let out = vicarius_with_input(home.path(), &["hook"], SESSION_START.as_bytes());
    let elapsed = started.elapsed();
    assert_eq!(text(&out.stdout), format!("{ONE_AGENT_LINE}\n"));
    assert!(
        elapsed < Duration::from_millis(400),
        "hook took {elapsed:?}"
    );
}

#[test]
fn hook_ignores_an_event_other_than_session_start() {
    let home = tempfile::tempdir().unwrap();
    let port = serve(status_route(OPEN));
    write_config(home.path(), port, SENIOR);
    for input in [
        r#"{"hook_event_name":"UserPromptSubmit","session_id":"s1"}"#,
        r#"{"hook_event_name":"PostToolUse","session_id":"s1"}"#,
        r#"{"session_id":"s1"}"#,
    ] {
        let out = vicarius_with_input(home.path(), &["hook"], input.as_bytes());
        assert_eq!(out.status.code(), Some(0), "{input}: {}", text(&out.stderr));
        assert!(out.stdout.is_empty(), "{input}: {}", text(&out.stdout));
    }
}

#[test]
fn hook_ignores_invalid_input() {
    let home = tempfile::tempdir().unwrap();
    let port = serve(status_route(OPEN));
    write_config(home.path(), port, SENIOR);
    let inputs: [&[u8]; 3] = [b"not json", b"", &[0xff, 0xfe]];
    for (index, input) in inputs.iter().enumerate() {
        let out = vicarius_with_input(home.path(), &["hook"], input);
        assert_eq!(out.status.code(), Some(0), "{index}: {}", text(&out.stderr));
        assert!(out.stdout.is_empty(), "{index}: {}", text(&out.stdout));
    }
}

#[test]
fn status_renders_the_agent_view_the_human_view_and_the_json_bytes() {
    let home = tempfile::tempdir().unwrap();
    let port = serve(status_route(OPEN));
    write_config(home.path(), port, SENIOR);
    let agent = vicarius(home.path(), &["status"]);
    assert_eq!(agent.status.code(), Some(0), "{}", text(&agent.stderr));
    let view = text(&agent.stdout);
    assert!(view.contains("balance: $18.40"), "{view}");
    assert!(view.contains("senior-worker-ds"), "{view}");
    assert!(view.contains("in 100, out 20"), "{view}");
    let human = vicarius(home.path(), &["--human", "status"]);
    assert_eq!(human.status.code(), Some(0), "{}", text(&human.stderr));
    assert!(text(&human.stdout).contains("$18.40"));
    let json = vicarius(home.path(), &["--json", "status"]);
    assert_eq!(text(&json.stdout), format!("{OPEN}\n"));
}

#[test]
fn status_exits_2_with_a_hint_when_the_service_is_down() {
    let home = tempfile::tempdir().unwrap();
    write_config(home.path(), free_port(), SENIOR);
    let out = vicarius(home.path(), &["status"]);
    assert_eq!(out.status.code(), Some(2));
    let stderr = text(&out.stderr);
    assert!(stderr.contains("error:"), "{stderr}");
    assert!(
        stderr.contains("hint: start the service with `vicarius self start`"),
        "{stderr}"
    );
}

#[test]
fn doctor_warns_about_a_missing_install_and_exits_0() {
    let home = tempfile::tempdir().unwrap();
    write_config(home.path(), free_port(), SENIOR);
    let out = vicarius(home.path(), &["self", "doctor"]);
    let report = text(&out.stdout);
    assert_eq!(out.status.code(), Some(0), "{report}{}", text(&out.stderr));
    assert!(
        report.contains("the launchd agent is not installed"),
        "{report}"
    );
    assert!(report.contains("not installed"), "{report}");
    assert!(report.contains("does not answer on 127.0.0.1:"), "{report}");
}

#[test]
fn doctor_fails_when_the_settings_point_at_a_service_that_does_not_answer() {
    let home = tempfile::tempdir().unwrap();
    let port = free_port();
    write_config(home.path(), port, SENIOR);
    let settings = home.path().join(".claude/settings.json");
    std::fs::create_dir_all(settings.parent().unwrap()).unwrap();
    let base = format!("http://127.0.0.1:{port}");
    std::fs::write(
        &settings,
        format!(r#"{{"env": {{"ANTHROPIC_BASE_URL": "{base}"}}}}"#),
    )
    .unwrap();
    let out = vicarius(home.path(), &["self", "doctor"]);
    let report = text(&out.stdout);
    assert_eq!(out.status.code(), Some(1), "{report}");
    assert!(report.contains("which does not answer"), "{report}");
}

#[test]
fn doctor_names_an_agent_file_that_conflicts_with_a_hand_written_one() {
    let home = tempfile::tempdir().unwrap();
    let body = r#"{"gate":{"open":true,"balance":18.4,"currency":"USD"},"agents":{"entries":[{"agent":"senior-worker","file":"senior-worker-ds.md","outcome":"conflict"}]}}"#;
    let port = serve(status_route(body));
    write_config(home.path(), port, SENIOR);
    let out = vicarius(home.path(), &["self", "doctor"]);
    let report = text(&out.stdout);
    assert_eq!(out.status.code(), Some(0), "{report}");
    assert!(report.contains("agent senior-worker"), "{report}");
    assert!(
        report.contains("a hand-written file holds the name"),
        "{report}"
    );
}

#[test]
fn doctor_flags_the_old_session_start_hook_as_outdated() {
    let home = tempfile::tempdir().unwrap();
    write_config(home.path(), free_port(), SENIOR);
    let settings = home.path().join(".claude/settings.json");
    std::fs::create_dir_all(settings.parent().unwrap()).unwrap();
    let old = r#"{"hooks": {"SessionStart": [{"hooks": [
        {"type": "command", "command": "vicarius hook session-start"}
    ]}]}}"#;
    std::fs::write(&settings, old).unwrap();
    let out = vicarius(home.path(), &["self", "doctor"]);
    let report = text(&out.stdout);
    assert_eq!(out.status.code(), Some(0), "{report}{}", text(&out.stderr));
    assert!(report.contains("outdated"), "{report}");
    assert!(report.contains("run `vicarius self setup`"), "{report}");
}

#[test]
fn doctor_fails_on_an_invalid_config_and_names_the_rule() {
    let home = tempfile::tempdir().unwrap();
    write_config(home.path(), 0, SENIOR);
    let out = vicarius(home.path(), &["self", "doctor"]);
    let report = text(&out.stdout);
    assert_eq!(out.status.code(), Some(1), "{report}");
    assert!(report.contains("port must not be 0"), "{report}");
}
