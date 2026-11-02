// SPDX-License-Identifier: MPL-2.0

use capsa_core::exit::Outcome;
use capsa_core::format::Format;
use capsa_core::render::{Render, emit_to};

use super::*;

#[test]
fn jsonc_comments_and_trailing_commas_are_stripped_outside_strings() {
    let src = r#"{
            // line
            "a": "x // not a comment, } ",  /* block */
            "b": [1, 2,],
            "c": {"d": "q\"/*",},
        }"#;
    let v: Value = serde_json::from_str(&strip_jsonc(src)).unwrap();
    assert_eq!(v["a"], "x // not a comment, } ");
    assert_eq!(v["b"], serde_json::json!([1, 2]));
    assert_eq!(v["c"]["d"], "q\"/*");
}

#[test]
fn detects_servers_that_run_pons() {
    let text = r#"{
            "agent_servers": {
                // mine
                "claude-pons": {"type": "custom", "command": "/Users/me/.cargo/bin/pons", "args": [],},
                "wrapped": {"command": "sh", "args": ["-c", "pons --log /tmp/x -- npx"]},
                "win": {"command": "C:\\bin\\PONS.exe"},
                "other": {"command": "npx", "args": ["-y", "pkg"]},
            },
        }"#;
    let Settings::Servers(found) = find_pons_servers(text) else {
        panic!("should parse");
    };
    let names: Vec<_> = found.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, vec!["claude-pons", "win", "wrapped"]);
    assert_eq!(found[0].command, "/Users/me/.cargo/bin/pons");
}

#[test]
fn no_agent_servers_or_garbage_never_fails() {
    assert_eq!(find_pons_servers("{}"), Settings::Servers(Vec::new()));
    assert_eq!(
        find_pons_servers("{ broken pons\n more"),
        Settings::Unparsed { mentions: 1 }
    );
}

fn env(platform: Platform) -> Env {
    Env {
        platform,
        path: Vec::new(),
        home: Some(PathBuf::from("/h")),
        appdata: Some(PathBuf::from("/appdata")),
        local_appdata: Some(PathBuf::from("/local")),
        pathext: Vec::new(),
    }
}

#[test]
fn settings_and_adapter_locations_per_platform() {
    assert_eq!(
        settings_path(&env(Platform::Macos)),
        Some(PathBuf::from("/h/.config/zed/settings.json"))
    );
    assert_eq!(
        settings_path(&env(Platform::Linux)),
        Some(PathBuf::from("/h/.config/zed/settings.json"))
    );
    assert_eq!(
        settings_path(&env(Platform::Windows)),
        Some(PathBuf::from("/appdata/Zed/settings.json"))
    );
    assert!(
        adapter_dir(&env(Platform::Macos))
            .unwrap()
            .starts_with("/h/Library/Application Support/Zed/external_agents")
    );
    assert!(
        adapter_dir(&env(Platform::Windows))
            .unwrap()
            .starts_with("/local/Zed/external_agents")
    );
}

#[test]
fn adapter_version_comes_from_the_manifest() {
    let read = |p: &Path| {
        p.ends_with("claude-agent-acp/package.json")
            .then(|| r#"{"name":"x","version":"0.85.1"}"#.to_owned())
    };
    assert_eq!(
        adapter_version(Path::new("/d"), &read).as_deref(),
        Some("0.85.1")
    );
    assert_eq!(adapter_version(Path::new("/d"), &|_| None), None);
}

/// One line of the JSON-lines wire log, as `capsa_core::log` writes it.
fn wire_line(dir: &str, text: &str) -> String {
    json!({"ts": 1, "level": "info", "event": "wire", "ms": 0, "dir": dir, "text": text})
        .to_string()
}

#[test]
fn summarizes_the_log() {
    let cancel = r#"{"jsonrpc":"2.0","method":"session/cancel","params":{}}"#;
    let log = [
        wire_line("client->agent", cancel),
        wire_line("pons", "message sent while subagents run: dropped the held session/cancel for s1 (108ms before prompt 2); steered"),
        wire_line("client->agent", cancel),
        wire_line("pons", "stop escape: forwarding repeated session/cancel for s1 (1 subagent(s) in flight)"),
        wire_line("client->agent", cancel),
        wire_line("pons", "background subagent A launched, card kept open"),
        wire_line("pons", "background subagent B launched, card kept open"),
        wire_line("pons", "background subagent A finished (task-notification)"),
        wire_line("pons", "dropped a task-notification user message (10 chars)"),
        wire_line("pons", "fail-safe: rules failed (x), forwarded the line unchanged (1 in a row for s1)"),
        wire_line("agent->client", "session/update session=s1 kind=agent_message_chunk textLen=1"),
        wire_line("pons", "exiting: error: agent exited (boom)"),
        "not a log line".to_owned(),
    ]
    .join("\n");
    let s = summarize_log(&log);
    assert_eq!(s.lines, 12);
    assert!(!s.legacy_text);
    assert_eq!(s.cancels_seen, 3);
    assert_eq!(s.cancels_dropped, 2);
    assert_eq!(s.cancels_forwarded(), 1);
    assert_eq!(s.stop_escapes, 1);
    assert_eq!(s.subagents_tracked, 2);
    assert_eq!(s.subagents_finished, 1);
    assert_eq!(s.fail_safe_events, 1);
    assert_eq!(s.notifications_dropped, 1);
    assert!(!s.stepped_aside);
    assert_eq!(
        s.last_exit.as_deref(),
        Some("exiting: error: agent exited (boom)")
    );
    // Eight notes in the log, the last five kept in order.
    assert_eq!(s.last_notes.len(), 5);
    assert!(s.last_notes[0].contains("subagent B launched"));
    assert!(s.last_notes[4].starts_with("exiting: error"));
}

#[test]
fn meta_level_logs_count_cancels_too() {
    let s = summarize_log(&wire_line("client->agent", "method=session/cancel"));
    assert_eq!(s.cancels_seen, 1);
}

#[test]
fn an_old_text_log_is_flagged_and_not_summarized() {
    let s = summarize_log("1.000 pons fail-safe: rules failed\n");
    assert!(s.legacy_text);
    assert_eq!(s.lines, 0);
    assert!(!summarize_log("").legacy_text);
}

/// Executables the launcher can find, nothing else.
struct Files(Vec<PathBuf>);

impl Fs for Files {
    fn is_executable(&self, path: &Path) -> bool {
        self.0.iter().any(|f| f == path)
    }
    fn is_dir(&self, _: &Path) -> bool {
        false
    }
    fn subdirs(&self, _: &Path) -> Vec<PathBuf> {
        Vec::new()
    }
}

/// Builds the report for auto-detection with `files` executable and `path` as PATH.
fn detect_report(files: &[&str], path: &[&str]) -> Report {
    let mut env = env(Platform::Linux);
    env.path = path.iter().map(PathBuf::from).collect();
    let inputs = Inputs {
        env,
        agent_cmd: Vec::new(),
        log: None,
    };
    let fs = Files(files.iter().map(PathBuf::from).collect());
    gather(&inputs, &fs, &|_| None, &NoSys)
}

#[test]
fn a_missing_adapter_fails_with_the_install_hint_of_the_manager_on_path() {
    let check = |files: &[&str]| detect_report(files, &["/bin"]).adapter;
    let none = check(&[]);
    assert_eq!(none.status, "fail");
    assert_eq!(
        none.hint.as_deref(),
        Some(format!("install Node.js, then run: npm install -g {PACKAGE}").as_str())
    );
    assert_eq!(
        check(&["/bin/npm"]).hint.as_deref(),
        Some(format!("npm install -g {PACKAGE}").as_str())
    );
    assert_eq!(
        check(&["/bin/npm", "/bin/bun"]).hint.as_deref(),
        Some(format!("bun add -g {PACKAGE}").as_str())
    );
    // pnpm on PATH is itself a runner (`pnpm dlx`), so the adapter is ok;
    // the preference is checked on the hint directly.
    let mut on_bin = env(Platform::Linux);
    on_bin.path = vec![PathBuf::from("/bin")];
    let all = Files(
        ["/bin/npm", "/bin/bun", "/bin/pnpm"]
            .into_iter()
            .map(PathBuf::from)
            .collect(),
    );
    assert_eq!(
        install_hint(&on_bin, &all, &NoSys),
        format!("pnpm add -g {PACKAGE}")
    );
    assert_eq!(check(&["/bin/pnpm"]).status, "ok");
}

/// Answers the same global bin directory for every manager.
struct BinSys(PathBuf);

impl Sys for BinSys {
    fn global_bin_dir(&self, _: capsa_core::pkg::Manager) -> Option<PathBuf> {
        Some(self.0.clone())
    }
}

#[test]
fn the_install_hint_names_the_global_bin_dir_only_when_it_is_off_the_path() {
    let fs = Files(["/bin/pnpm"].into_iter().map(PathBuf::from).collect());
    let sys = BinSys(PathBuf::from("/g/bin"));
    let mut off = env(Platform::Linux);
    off.path = vec![PathBuf::from("/bin")];
    assert_eq!(
        install_hint(&off, &fs, &sys),
        format!("pnpm add -g {PACKAGE}, then put /g/bin on PATH")
    );
    let mut on = env(Platform::Linux);
    on.path = vec![PathBuf::from("/bin"), PathBuf::from("/g/bin")];
    assert_eq!(
        install_hint(&on, &fs, &sys),
        format!("pnpm add -g {PACKAGE}")
    );
}

#[test]
fn the_adapter_is_ok_through_any_launcher_and_shows_its_version_when_known() {
    let binary = detect_report(&["/bin/claude-agent-acp"], &["/bin"]).adapter;
    assert_eq!(binary.status, "ok");
    let adapter = Path::new("/bin").join("claude-agent-acp");
    assert!(
        binary
            .detail
            .contains(&format!("{} (version unknown)", adapter.display())),
        "{}",
        binary.detail
    );
    assert_eq!(binary.hint, None);
    let fetched = detect_report(&["/bin/npx"], &["/bin"]).adapter;
    assert_eq!(fetched.status, "ok");
    assert!(fetched.detail.contains("through npx"), "{}", fetched.detail);
}

#[test]
fn explicit_commands_skip_the_check_and_text_and_json_show_it() {
    let explicit = adapter_check(true, None, None, String::new);
    assert_eq!(explicit.status, "ok");
    assert_eq!(explicit.hint, None);
    let view = View::new(detect_report(&[], &["/bin"]));
    let plain = text(&view, false);
    assert!(plain.contains("claude-agent-acp adapter"), "{plain}");
    assert!(
        plain.contains(&format!(
            "install Node.js, then run: npm install -g {PACKAGE}"
        )),
        "{plain}"
    );
    assert!(!plain.contains('\x1b'));
    assert!(text(&view, true).contains('\x1b'));
    assert!(matches!(view.outcome(), Outcome::Empty));
    let v: Value = serde_json::from_str(&json_of(&view)).unwrap();
    assert_eq!(v["ok"], false);
    assert_eq!(v["groups"][0]["name"], "launcher");
    assert_eq!(v["groups"][0]["checks"][1]["status"], "fail");
    assert_eq!(v["launcher"]["adapter"]["status"], "fail");
    assert!(
        v["launcher"]["adapter"]["detail"]
            .as_str()
            .is_some_and(|d| d.starts_with("not found"))
    );
    assert_eq!(
        v["launcher"]["adapter"]["hint"],
        format!("install Node.js, then run: npm install -g {PACKAGE}")
    );
}

#[test]
fn the_report_is_ascii_and_survives_missing_everything() {
    struct NoFs;
    impl Fs for NoFs {
        fn is_executable(&self, _: &Path) -> bool {
            false
        }
        fn is_dir(&self, _: &Path) -> bool {
            false
        }
        fn subdirs(&self, _: &Path) -> Vec<PathBuf> {
            Vec::new()
        }
    }
    let inputs = Inputs {
        env: env(Platform::Macos),
        agent_cmd: Vec::new(),
        log: Some(PathBuf::from("/nope.log")),
    };
    let view = View::new(gather(&inputs, &NoFs, &|_| None, &NoSys));
    let plain = text(&view, false);
    assert!(plain.is_ascii(), "{plain}");
    assert!(plain.contains("nothing would run"));
    assert!(plain.contains("not found or unreadable"));
    assert!(plain.contains(concat!("pons ", env!("CARGO_PKG_VERSION"))));
    assert!(plain.contains("no running pons proxy found"));
    let json = json_of(&view);
    assert!(json.is_ascii());
    let v: Value = serde_json::from_str(&json).unwrap();
    assert_eq!(v["pons"]["version"], env!("CARGO_PKG_VERSION"));
    assert!(v["launcher"]["would_run"].is_null());
}

#[test]
fn etime_and_ps_lines_are_parsed() {
    assert_eq!(parse_etime("05:07"), Some(307));
    assert_eq!(parse_etime("01:02:03"), Some(3723));
    assert_eq!(parse_etime("2-00:00:10"), Some(172_810));
    assert_eq!(parse_etime("x"), None);
    let ps = "\
  101    10:00 /Users/me/.cargo/bin/pons --log /tmp/pons.log
  102       00:05 /Users/me/.cargo/bin/pons self doctor
  103   1-00:00:00 node /x/pons.js
  104   00:09 /usr/bin/pons
  garbage
";
    let procs = parse_ps(ps, 104);
    assert_eq!(
        procs,
        vec![Proc {
            pid: 101,
            age_secs: 600
        }]
    );
    assert!(is_stale(&procs[0], Some(100)));
    assert!(!is_stale(&procs[0], Some(600)));
    assert!(!is_stale(&procs[0], None));
}

#[test]
fn the_configured_adapter_version_is_read_next_to_the_real_binary() {
    struct Sym;
    impl Sys for Sym {
        fn canonicalize(&self, p: &Path) -> Option<PathBuf> {
            (p == Path::new("/bin/claude-agent-acp")).then(|| {
                PathBuf::from(
                    "/lib/node_modules/@agentclientprotocol/claude-agent-acp/dist/index.js",
                )
            })
        }
    }
    let read = |p: &Path| {
        (p == Path::new("/lib/node_modules/@agentclientprotocol/claude-agent-acp/package.json"))
            .then(|| {
                r#"{"name":"@agentclientprotocol/claude-agent-acp","version":"0.9.9"}"#.to_owned()
            })
    };
    let (v, manifest) =
        configured_adapter(Path::new("/bin/claude-agent-acp"), &[], &Sym, &read).unwrap();
    assert_eq!(v, "0.9.9");
    assert!(manifest.ends_with("package.json"));
    assert!(configured_adapter(Path::new("/bin/other"), &[], &Sym, &read).is_none());
}

#[test]
fn exits_and_provisional_finishes_are_summarized() {
    let s = summarize_log(
        &[
            wire_line("pons", "background subagent A finished (idle, provisional)"),
            wire_line("pons", "background subagent A finished (prompt response)"),
            wire_line("pons", "exiting: client closed stdin"),
        ]
        .join("\n"),
    );
    assert_eq!((s.subagents_finished, s.subagents_provisional), (2, 1));
    assert_eq!(s.last_exit.as_deref(), Some("exiting: client closed stdin"));
}

/// The agent view of `view`, or the human view with `colour`.
fn text(view: &View, colour: bool) -> String {
    let format = if colour { Format::Human } else { Format::Agent };
    let mut out = Vec::new();
    emit_to(view, format, colour, &mut out).unwrap();
    String::from_utf8(out).unwrap()
}

/// The `--json` line of `view`.
fn json_of(view: &View) -> String {
    let mut out = Vec::new();
    emit_to(view, Format::Json, false, &mut out).unwrap();
    String::from_utf8(out).unwrap()
}

/// A machine with a launcher-found adapter, a Zed entry, a registry adapter, a wire log and
/// one running proxy.
fn fixture() -> Report {
    Report {
        version: env!("CARGO_PKG_VERSION"),
        platform: "linux",
        arch: "x86_64",
        explicit_command: false,
        would_run: Ok((
            "npx".to_owned(),
            "/bin/npx -y @agentclientprotocol/claude-agent-acp".to_owned(),
        )),
        candidates: vec![(
            "npx -y @agentclientprotocol/claude-agent-acp".to_owned(),
            Some("/bin/npx".to_owned()),
        )],
        searched_dirs: 3,
        configured_adapter: None,
        adapter: AdapterCheck {
            status: "ok",
            detail: "claude-agent-acp at /bin/claude-agent-acp (version 0.85.1)".to_owned(),
            hint: None,
        },
        settings_path: Some("/h/.config/zed/settings.json".to_owned()),
        settings: Some(Settings::Servers(vec![Server {
            name: "claude-pons".to_owned(),
            command: "/bin/pons".to_owned(),
            args: vec!["--log".to_owned(), "/tmp/x".to_owned()],
        }])),
        registry_adapter: Some((
            Some("0.85.1".to_owned()),
            "/h/.local/share/zed/external_agents/registry/npx/claude-acp".to_owned(),
        )),
        log: Some(LogReport {
            path: "/tmp/pons.log".to_owned(),
            summary: Some(summarize_log(&wire_line(
                "pons",
                "background subagent A launched, card kept open",
            ))),
            size: Some(100),
            rotated_size: None,
        }),
        processes: vec![Proc {
            pid: 7,
            age_secs: 60,
        }],
        exe_age_secs: Some(3600),
    }
}

#[test]
fn json_keeps_every_key_of_the_report() {
    let mut v: Value = serde_json::from_str(&json_of(&View::new(fixture()))).unwrap();
    assert_eq!(v["ok"], true);
    assert_eq!(v["groups"].as_array().map(Vec::len), Some(4));
    let object = v.as_object_mut().unwrap();
    object.remove("ok");
    object.remove("groups");
    assert_eq!(
        v,
        json!({
            "pons": {"version": env!("CARGO_PKG_VERSION"), "platform": "linux", "arch": "x86_64"},
            "launcher": {
                "explicit_command": false,
                "would_run": "npx",
                "command": "/bin/npx -y @agentclientprotocol/claude-agent-acp",
                "error": null,
                "candidates": [{"name": "npx -y @agentclientprotocol/claude-agent-acp", "path": "/bin/npx"}],
                "searched_dirs": 3,
                "adapter": {"status": "ok", "detail": "claude-agent-acp at /bin/claude-agent-acp (version 0.85.1)", "hint": null},
                "configured_adapter": null,
            },
            "zed": {
                "settings_path": "/h/.config/zed/settings.json",
                "settings": {"servers": [{"name": "claude-pons", "command": "/bin/pons", "args": ["--log", "/tmp/x"]}]},
                "registry_adapter": {"version": "0.85.1", "dir": "/h/.local/share/zed/external_agents/registry/npx/claude-acp"},
            },
            "processes": [{"pid": 7, "age_secs": 60, "stale": false}],
            "exe_age_secs": 3600,
            "log": {
                "path": "/tmp/pons.log",
                "size": 100,
                "rotated_size": null,
                "summary": {
                    "lines": 1,
                    "cancels_seen": 0,
                    "cancels_dropped": 0,
                    "cancels_forwarded": 0,
                    "stop_escapes": 0,
                    "subagents_tracked": 1,
                    "subagents_finished": 0,
                    "subagents_provisional": 0,
                    "fail_safe_events": 0,
                    "notifications_dropped": 0,
                    "stepped_aside": false,
                    "last_exit": null,
                    "last_notes": ["background subagent A launched, card kept open"],
                },
            },
        })
    );
}
