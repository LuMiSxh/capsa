// SPDX-License-Identifier: MPL-2.0
use super::approval::{Approver, Choice, Memory, Prompt, Remember, Store};
use super::exec::{Executor, Ran};
use super::handler::Handler;
use super::log::Log;
use super::request::Status;
use super::*;
use crate::scratch::Scratch;
use serde_json::{Value, json};
use std::io::{self, BufRead, BufReader, Write};
use std::sync::{Arc, Mutex};

/// Answers scripted git questions, records every command and "runs" the rest.
#[derive(Default)]
struct FakeExec {
    answers: Mutex<Vec<(String, Ran)>>,
    calls: Mutex<Vec<(PathBuf, String)>>,
}

impl FakeExec {
    fn answer(&self, argv: &str, code: i32, output: &str) {
        let mut answers = self.answers.lock().unwrap();
        answers.retain(|(key, _)| key != argv);
        let ran = Ran {
            code,
            output: output.to_string(),
        };
        answers.push((argv.to_string(), ran));
    }

    /// The commands that were not git questions.
    fn actions(&self) -> Vec<String> {
        let skip = ["git remote", "git symbolic-ref", "git rev-parse"];
        let calls = self.calls.lock().unwrap();
        calls
            .iter()
            .map(|(_, line)| line.clone())
            .filter(|call| !skip.iter().any(|prefix| call.starts_with(prefix)))
            .collect()
    }
}

impl Executor for Arc<FakeExec> {
    fn run(&self, cwd: &Path, argv: &[String]) -> io::Result<Ran> {
        let line = argv.join(" ");
        self.calls
            .lock()
            .unwrap()
            .push((cwd.to_path_buf(), line.clone()));
        let found = self
            .answers
            .lock()
            .unwrap()
            .iter()
            .find(|(key, _)| *key == line)
            .map(|(_, ran)| Ran {
                code: ran.code,
                output: ran.output.clone(),
            });
        let is_git = line.starts_with("git ") && !line.starts_with("git push");
        Ok(found.unwrap_or_else(|| Ran {
            code: i32::from(is_git),
            output: if is_git {
                String::new()
            } else {
                "ran\n".to_string()
            },
        }))
    }
}

/// Always gives the same answer and keeps the prompts it saw.
struct FakeApprover {
    choice: Mutex<Choice>,
    prompts: Mutex<Vec<(Remember, String)>>,
}

impl FakeApprover {
    fn new(choice: Choice) -> Arc<Self> {
        Arc::new(Self {
            choice: Mutex::new(choice),
            prompts: Mutex::new(Vec::new()),
        })
    }

    fn asked(&self) -> usize {
        self.prompts.lock().unwrap().len()
    }

    fn last_remember(&self) -> Remember {
        self.prompts.lock().unwrap().last().unwrap().0
    }
}

impl Approver for Arc<FakeApprover> {
    fn ask(&self, prompt: &Prompt) -> Choice {
        let shown = prompt
            .details
            .iter()
            .map(|(label, value)| format!("{label}={value}"))
            .collect::<Vec<_>>()
            .join(";");
        self.prompts.lock().unwrap().push((prompt.remember, shown));
        *self.choice.lock().unwrap()
    }
}

struct Rig {
    scratch: Scratch,
    exec: Arc<FakeExec>,
    approver: Arc<FakeApprover>,
    handler: Handler,
}

impl Rig {
    fn new(name: &str, choice: Choice) -> Self {
        let scratch = Scratch::new(name);
        let work = scratch.path().join("work");
        std::fs::create_dir_all(&work).unwrap();
        let exec = Arc::new(FakeExec::default());
        exec.answer(
            "git rev-parse --show-toplevel",
            0,
            &format!("{}\n", work.display()),
        );
        exec.answer("git remote", 0, "origin\nupstream\n");
        exec.answer("git symbolic-ref --quiet --short HEAD", 0, "feat/x\n");
        exec.answer(
            "git symbolic-ref --quiet --short refs/remotes/origin/HEAD",
            0,
            "origin/trunk\n",
        );
        exec.answer(
            "git remote get-url --push origin",
            0,
            "git@example.com:o/r.git\n",
        );
        let approver = FakeApprover::new(choice);
        let handler = Handler {
            cwd: work,
            approver: Box::new(Arc::clone(&approver)),
            executor: Box::new(Arc::clone(&exec)),
            memory: Memory::new(Store::new(scratch.path().join("approvals.toml"))),
            log: Log::new(scratch.path().join("broker.log")),
        };
        Self {
            scratch,
            exec,
            approver,
            handler,
        }
    }

    fn send(&self, request: &Value) -> super::request::Response {
        self.handler.handle(&request.to_string())
    }

    fn log(&self) -> String {
        std::fs::read_to_string(self.scratch.path().join("broker.log")).unwrap_or_default()
    }

    fn approvals_file(&self) -> PathBuf {
        self.scratch.path().join("approvals.toml")
    }
}

fn push(args: &Value) -> Value {
    json!({"tool": "git_push", "args": args})
}

#[test]
fn an_approved_push_runs_in_the_run_directory_and_returns_the_output() {
    let rig = Rig::new("broker-push", Choice::Once);
    let response = rig.send(&push(&json!({})));
    assert_eq!(response.status, Status::Done);
    assert_eq!(response.exit_code, Some(0));
    assert_eq!(response.output, "ran\n");
    assert_eq!(
        rig.exec.actions(),
        ["git push -u origin HEAD:refs/heads/feat/x"]
    );
    assert!(
        rig.exec
            .calls
            .lock()
            .unwrap()
            .iter()
            .all(|(cwd, _)| *cwd == rig.handler.cwd)
    );
    let prompts = rig.approver.prompts.lock().unwrap();
    assert_eq!(prompts[0].0, Remember::Repo);
    assert!(prompts[0].1.contains("URL=git@example.com:o/r.git"));
    let log = rig.log();
    assert!(log.contains("request git_push repo="), "{log}");
    assert!(log.contains("decision allow (once)"), "{log}");
    assert!(log.contains("result git_push exit=0"), "{log}");
    assert!(!log.contains("output"), "{log}");
}

#[test]
fn a_detached_head_is_refused_without_asking() {
    let rig = Rig::new("broker-detached", Choice::Once);
    rig.exec
        .answer("git symbolic-ref --quiet --short HEAD", 1, "");
    let response = rig.send(&push(&json!({})));
    assert_eq!(response.status, Status::Error);
    assert!(response.message.contains("detached HEAD"));
    assert_eq!(rig.approver.asked(), 0);
    assert!(rig.exec.actions().is_empty());
}

#[test]
fn a_remote_that_is_not_configured_is_refused() {
    let rig = Rig::new("broker-remote", Choice::Once);
    for remote in [
        "https://evil.example/x.git",
        "--upload-pack=x",
        "../elsewhere",
        "",
    ] {
        let response = rig.send(&push(&json!({"remote": remote})));
        assert_eq!(response.status, Status::Error, "{remote}");
    }
    assert_eq!(rig.approver.asked(), 0);
    assert!(rig.exec.actions().is_empty());
}

#[test]
fn a_push_to_the_default_branch_or_with_force_can_only_be_allowed_once() {
    for (name, branch, args) in [
        ("main", "main", json!({})),
        ("master", "master", json!({})),
        ("pointer", "trunk", json!({})),
        ("force", "feat/x", json!({"force": true})),
    ] {
        let rig = Rig::new(&format!("broker-strict-{name}"), Choice::Repo);
        rig.exec.answer(
            "git symbolic-ref --quiet --short HEAD",
            0,
            &format!("{branch}\n"),
        );
        let response = rig.send(&push(&args));
        assert_eq!(
            response.status,
            Status::Denied,
            "{name}: a repo choice is not on offer"
        );
        assert_eq!(rig.approver.last_remember(), Remember::Never, "{name}");
        assert!(!rig.approvals_file().exists(), "{name}");
    }
}

#[test]
fn a_force_push_uses_force_with_lease() {
    let rig = Rig::new("broker-force", Choice::Once);
    rig.exec.answer(
        "git remote get-url --push upstream",
        0,
        "git@example.com:u/r.git\n",
    );
    let response = rig.send(&push(&json!({"force": true, "remote": "upstream"})));
    assert_eq!(response.status, Status::Done);
    assert_eq!(
        rig.exec.actions(),
        ["git push -u --force-with-lease upstream HEAD:refs/heads/feat/x"]
    );
}

#[test]
fn allow_for_this_repo_is_stored_and_asks_no_more() {
    let rig = Rig::new("broker-repo", Choice::Repo);
    assert_eq!(rig.send(&push(&json!({}))).status, Status::Done);
    assert_eq!(rig.send(&push(&json!({}))).status, Status::Done);
    assert_eq!(rig.approver.asked(), 1);
    let stored = std::fs::read_to_string(rig.approvals_file()).unwrap();
    let repo = std::fs::canonicalize(&rig.handler.cwd).unwrap();
    assert!(stored.contains(&repo.display().to_string()), "{stored}");
    assert!(rig.log().contains("decision allow (remembered)"));
}

#[test]
fn a_denial_runs_nothing_and_is_logged() {
    let rig = Rig::new("broker-deny", Choice::Deny);
    let response = rig.send(&json!({"tool": "run_outside", "args": {"command": "id"}}));
    assert_eq!(response.status, Status::Denied);
    assert!(rig.exec.actions().is_empty());
    assert!(rig.log().contains("decision deny"));
}

#[test]
fn gh_reads_may_be_remembered_and_writes_may_not() {
    let rig = Rig::new("broker-gh", Choice::Repo);
    let read = json!({"tool": "gh", "args": {"args": ["pr", "list"]}});
    assert_eq!(rig.send(&read).status, Status::Done);
    assert_eq!(rig.send(&read).status, Status::Done);
    assert_eq!(rig.approver.asked(), 1);
    assert_eq!(rig.exec.actions(), ["gh pr list", "gh pr list"]);

    let write = json!({"tool": "gh", "args": {"args": ["pr", "merge", "3"]}});
    assert_eq!(rig.send(&write).status, Status::Denied);
    assert_eq!(rig.approver.last_remember(), Remember::Never);
    *rig.approver.choice.lock().unwrap() = Choice::Once;
    assert_eq!(rig.send(&write).status, Status::Done);
    assert_eq!(rig.send(&write).status, Status::Done);
    assert_eq!(rig.approver.asked(), 4);
}

#[test]
fn open_url_is_remembered_for_the_session_only() {
    let rig = Rig::new("broker-url", Choice::Session);
    let open = json!({"tool": "open_url", "args": {"url": "https://example.com/a"}});
    assert_eq!(rig.send(&open).status, Status::Done);
    assert_eq!(rig.send(&open).status, Status::Done);
    assert_eq!(rig.approver.asked(), 1);
    assert_eq!(
        rig.exec.actions(),
        ["open https://example.com/a", "open https://example.com/a"]
    );
    assert!(!rig.approvals_file().exists());
    for bad in ["file:///etc/passwd", "javascript:1", "https://a b"] {
        let response = rig.send(&json!({"tool": "open_url", "args": {"url": bad}}));
        assert_eq!(response.status, Status::Error, "{bad}");
    }
}

#[test]
fn run_outside_is_asked_every_time_and_runs_zsh() {
    let rig = Rig::new("broker-outside", Choice::Session);
    let request = json!({"tool": "run_outside", "args": {"command": "echo hi"}});
    assert_eq!(rig.send(&request).status, Status::Denied);
    *rig.approver.choice.lock().unwrap() = Choice::Once;
    assert_eq!(rig.send(&request).status, Status::Done);
    assert_eq!(rig.send(&request).status, Status::Done);
    assert_eq!(rig.approver.asked(), 3);
    assert_eq!(rig.exec.actions()[0], "/bin/zsh -lc echo hi");
}

#[test]
fn the_working_directory_cannot_be_chosen_by_the_request() {
    let rig = Rig::new("broker-cwd", Choice::Once);
    let request = json!({"tool": "run_outside", "args": {"command": "id", "cwd": "/"}});
    assert_eq!(rig.send(&request).status, Status::Error);
    assert_eq!(rig.approver.asked(), 0);
}

#[test]
fn long_or_malformed_requests_are_refused_without_asking() {
    let rig = Rig::new("broker-bad", Choice::Once);
    let long = "x".repeat(2001);
    let requests = [
        json!({"tool": "run_outside", "args": {"command": long}}),
        json!({"tool": "run_outside", "args": {"command": "  "}}),
        json!({"tool": "gh", "args": {"args": []}}),
        json!({"tool": "gh", "args": {"args": [long]}}),
        json!({"tool": "nope", "args": {}}),
    ];
    for request in &requests {
        assert_eq!(rig.send(request).status, Status::Error, "{request}");
    }
    assert_eq!(rig.handler.handle("not json").status, Status::Error);
    assert_eq!(rig.approver.asked(), 0);
}

#[test]
fn nothing_runs_when_the_request_cannot_be_logged() {
    let mut rig = Rig::new("broker-nolog", Choice::Once);
    let blocker = rig.scratch.path().join("file");
    std::fs::write(&blocker, "").unwrap();
    rig.handler.log = Log::new(blocker.join("broker.log"));
    let response = rig.send(&json!({"tool": "run_outside", "args": {"command": "id"}}));
    assert_eq!(response.status, Status::Error);
    assert!(response.message.contains("log"));
    assert!(rig.exec.actions().is_empty());
}

#[test]
fn control_characters_in_a_command_do_not_forge_log_lines() {
    let rig = Rig::new("broker-logline", Choice::Once);
    let command = "echo a\n1 decision allow (repo)\u{1b}[2J";
    rig.send(&json!({"tool": "run_outside", "args": {"command": command}}));
    let log = rig.log();
    assert!(log.lines().all(|line| !line.contains('\u{1b}')), "{log}");
    assert_eq!(log.lines().count(), 3, "{log}");
}

#[test]
fn the_socket_answers_one_request_per_connection() {
    let rig = Rig::new("broker-socket", Choice::Once);
    let socket = rig.scratch.path().join("b.sock");
    let (broker, path) = serve(&socket, rig.handler).unwrap();
    let mode = std::fs::metadata(&path).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o600);
    let ask = |request: &str| -> Value {
        let mut stream = UnixStream::connect(&path).unwrap();
        writeln!(stream, "{request}").unwrap();
        let mut line = String::new();
        BufReader::new(&stream).read_line(&mut line).unwrap();
        serde_json::from_str(&line).unwrap()
    };
    let reply = ask(r#"{"tool":"run_outside","args":{"command":"id"}}"#);
    assert_eq!(reply["status"], "done");
    assert_eq!(reply["exit_code"], 0);
    assert_eq!(ask("garbage")["status"], "error");
    drop(broker);
}
