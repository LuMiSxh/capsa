// SPDX-License-Identifier: MPL-2.0
use super::approval::{Approver, Choice, Memory, Prompt, Remember};
use super::exec::{Executor, Ran, Registry};
use super::handler::Handler;
use super::log::Log;
use super::repo::Recorded;
use super::request::{Response, Status};
use super::*;
use crate::scratch::Scratch;
use serde_json::{Value, json};
use std::io::{self, BufRead, BufReader, Write};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};

/// A call as the executor saw it.
struct Call {
    cwd: PathBuf,
    line: String,
    /// The root of the repository the command was bound to.
    bound: Option<PathBuf>,
}

/// Answers scripted git questions, records every command and "runs" the rest.
#[derive(Default)]
struct FakeExec {
    answers: Mutex<Vec<(String, Ran)>>,
    calls: Mutex<Vec<Call>>,
}

/// Returns a git command line without the options `Recorded::git` puts in front, so that the
/// scripted answers read like `git remote`.
fn plain(line: &str) -> String {
    match line.split_once(" core.fsmonitor=false ") {
        Some((_, rest)) if line.starts_with("git --git-dir=") => format!("git {rest}"),
        _ => line.to_string(),
    }
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

    /// The commands that were not git questions, git ones without the leading options.
    fn actions(&self) -> Vec<String> {
        let skip = ["git remote", "git symbolic-ref"];
        let calls = self.calls.lock().unwrap();
        calls
            .iter()
            .map(|call| plain(&call.line))
            .filter(|call| !skip.iter().any(|prefix| call.starts_with(prefix)))
            .collect()
    }

    fn full(&self, needle: &str) -> Vec<String> {
        let calls = self.calls.lock().unwrap();
        let all = calls.iter().map(|call| call.line.clone());
        all.filter(|line| line.contains(needle)).collect()
    }
}

impl Executor for Arc<FakeExec> {
    fn run(&self, cwd: &Path, argv: &[String], repo: Option<&Recorded>) -> io::Result<Ran> {
        let line = argv.join(" ");
        self.calls.lock().unwrap().push(Call {
            cwd: cwd.to_path_buf(),
            line: line.clone(),
            bound: repo.map(|repo| repo.root.clone()),
        });
        let key = plain(&line);
        let found = self
            .answers
            .lock()
            .unwrap()
            .iter()
            .find(|(answer, _)| *answer == key)
            .map(|(_, ran)| Ran {
                code: ran.code,
                output: ran.output.clone(),
            });
        let is_git = key.starts_with("git ") && !key.starts_with("git push");
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

/// Gives the same answer every time and keeps the prompts it saw. It can also be told to
/// hold the next question open until released.
struct FakeApprover {
    choice: Mutex<Choice>,
    prompts: Mutex<Vec<(Remember, String)>>,
    hold: Mutex<Option<(Sender<()>, Receiver<()>)>>,
}

impl FakeApprover {
    fn new(choice: Choice) -> Arc<Self> {
        Arc::new(Self {
            choice: Mutex::new(choice),
            prompts: Mutex::new(Vec::new()),
            hold: Mutex::new(None),
        })
    }

    fn asked(&self) -> usize {
        self.prompts.lock().unwrap().len()
    }

    fn last_remember(&self) -> Remember {
        self.prompts.lock().unwrap().last().unwrap().0
    }

    fn set(&self, choice: Choice) {
        *self.choice.lock().unwrap() = choice;
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
        let hold = self.hold.lock().unwrap().take();
        if let Some((entered, release)) = hold {
            entered.send(()).unwrap();
            release.recv().unwrap();
        }
        *self.choice.lock().unwrap()
    }
}

struct Rig {
    scratch: Scratch,
    exec: Arc<FakeExec>,
    approver: Arc<FakeApprover>,
    handler: Handler,
}

/// A repository whose git configuration the sandbox cannot write, as `velum run` records it.
fn make_repo(work: &Path) -> Recorded {
    std::fs::create_dir_all(work.join(".git/hooks")).unwrap();
    std::fs::write(work.join(".git/config"), "").unwrap();
    let deny = [".git", ".git/config", ".git/hooks"].map(|rest| work.join(rest));
    let settings = json!({"filesystem": {"allowWrite": [work], "denyWrite": deny}});
    Recorded::resolve(work, &settings).unwrap()
}

impl Rig {
    fn new(name: &str, choice: Choice) -> Self {
        let scratch = Scratch::new(name);
        let work = scratch.path().join("work");
        let repo = make_repo(&work);
        let exec = Arc::new(FakeExec::default());
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
            "git@github.com:o/r.git\n",
        );
        exec.answer(
            "git remote get-url --push upstream",
            0,
            "git@github.com:u/r.git\n",
        );
        let approver = FakeApprover::new(choice);
        let handler = Handler {
            cwd: std::fs::canonicalize(&work).unwrap(),
            repo: Ok(repo),
            approver: Box::new(Arc::clone(&approver)),
            executor: Box::new(Arc::clone(&exec)),
            memory: Memory::new(scratch.path().join("approvals.toml")),
            log: Log::new(scratch.path().join("broker.log")),
            busy: Mutex::new(()),
        };
        Self {
            scratch,
            exec,
            approver,
            handler,
        }
    }

    fn send(&self, request: &Value) -> Response {
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

fn outside(command: &str) -> Value {
    json!({"tool": "run_outside", "args": {"command": command}})
}

fn gh(args: &[&str]) -> Value {
    json!({"tool": "gh", "args": {"args": args}})
}

#[test]
fn an_approved_push_runs_hardened_in_the_run_directory() {
    let rig = Rig::new("broker-push", Choice::Once);
    let response = rig.send(&push(&json!({})));
    assert_eq!(response.status, Status::Done);
    assert_eq!(response.exit_code, Some(0));
    assert_eq!(response.output, "ran\n");
    assert_eq!(
        rig.exec.actions(),
        ["git push --no-verify -u origin HEAD:refs/heads/feat/x"]
    );
    let pushes = rig.exec.full(" push --no-verify");
    assert_eq!(pushes.len(), 1);
    for part in [
        "--git-dir=",
        "--work-tree=",
        "-c core.hooksPath=/dev/null",
        "-c core.fsmonitor=false",
        "--no-verify",
    ] {
        assert!(pushes[0].contains(part), "{part} in {}", pushes[0]);
    }
    let calls = rig.exec.calls.lock().unwrap();
    assert!(
        calls
            .iter()
            .all(|call| call.cwd == rig.handler.cwd && call.bound.as_ref() == Some(&call.cwd))
    );
    drop(calls);
    let prompts = rig.approver.prompts.lock().unwrap();
    assert_eq!(prompts[0].0, Remember::Repo);
    assert!(prompts[0].1.contains("URL=git@github.com:o/r.git"));
    let log = rig.log();
    assert!(log.contains("request git_push repo="), "{log}");
    assert!(log.contains("decision allow (once)"), "{log}");
    assert!(log.contains("result git_push exit=0"), "{log}");
    assert!(!log.contains("output"), "{log}");
}

#[test]
fn a_repository_that_was_not_recorded_is_refused_for_git_and_gh() {
    let mut rig = Rig::new("broker-hostile", Choice::Once);
    rig.handler.repo = Err("the working directory is not inside a git repository".to_string());
    for request in [
        push(&json!({})),
        gh(&["pr", "list"]),
        gh(&["api", "repos/o/r"]),
    ] {
        let response = rig.send(&request);
        assert_eq!(response.status, Status::Error, "{request}");
        assert!(
            response.message.contains("outside velum"),
            "{}",
            response.message
        );
    }
    assert_eq!(rig.approver.asked(), 0);
    assert!(rig.exec.calls.lock().unwrap().is_empty());
    assert_eq!(rig.send(&outside("id")).status, Status::Done);
}

#[test]
fn a_git_directory_replaced_after_the_start_is_refused() {
    let rig = Rig::new("broker-replaced", Choice::Once);
    let work = rig.handler.cwd.clone();
    std::fs::rename(work.join(".git"), work.join("old")).unwrap();
    std::fs::create_dir_all(work.join(".git/hooks")).unwrap();
    let response = rig.send(&push(&json!({})));
    assert_eq!(response.status, Status::Error);
    assert!(response.message.contains("no longer the one recorded"));
    assert_eq!(rig.approver.asked(), 0);
}

#[test]
fn the_approval_key_is_the_recorded_root_whatever_git_reports() {
    let rig = Rig::new("broker-worktree", Choice::Repo);
    rig.exec
        .answer("git rev-parse --show-toplevel", 0, "/somewhere/else\n");
    assert_eq!(rig.send(&push(&json!({}))).status, Status::Done);
    let stored = std::fs::read_to_string(rig.approvals_file()).unwrap();
    assert!(
        stored.contains(&rig.handler.cwd.display().to_string()),
        "{stored}"
    );
    assert!(!stored.contains("/somewhere/else"), "{stored}");
    assert!(rig.exec.full("rev-parse").is_empty());
    let queries = rig.exec.full("symbolic-ref");
    let root = format!("--work-tree={}", rig.handler.cwd.display());
    assert!(queries.iter().all(|line| line.contains(&root)));
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
fn pushes_to_other_than_a_typed_work_branch_are_asked_every_time() {
    for (name, branch, args) in [
        ("main", "main", json!({})),
        ("master", "master", json!({})),
        ("pointer", "trunk", json!({})),
        ("force", "feat/x", json!({"force": true})),
        ("untyped", "wip/thing", json!({})),
        ("plain", "scratch", json!({})),
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
        rig.approver.set(Choice::Once);
        assert_eq!(rig.send(&push(&args)).status, Status::Done, "{name}");
        assert_eq!(rig.send(&push(&args)).status, Status::Done, "{name}");
        assert_eq!(rig.approver.asked(), 3, "{name}");
    }
}

#[test]
fn a_force_push_uses_force_with_lease() {
    let rig = Rig::new("broker-force", Choice::Once);
    let response = rig.send(&push(&json!({"force": true, "remote": "upstream"})));
    assert_eq!(response.status, Status::Done);
    assert_eq!(
        rig.exec.actions(),
        ["git push --no-verify -u --force-with-lease upstream HEAD:refs/heads/feat/x"]
    );
}

#[test]
fn allow_for_this_repo_is_stored_and_asks_no_more_until_the_url_changes() {
    let rig = Rig::new("broker-repo", Choice::Repo);
    assert_eq!(rig.send(&push(&json!({}))).status, Status::Done);
    assert_eq!(rig.send(&push(&json!({}))).status, Status::Done);
    assert_eq!(rig.approver.asked(), 1);
    assert!(rig.log().contains("decision allow (remembered)"));

    rig.exec.answer(
        "git remote get-url --push origin",
        0,
        "git@github.com:evil/r.git\n",
    );
    assert_eq!(rig.send(&push(&json!({}))).status, Status::Done);
    assert_eq!(rig.approver.asked(), 2);
    let prompts = rig.approver.prompts.lock().unwrap();
    assert!(prompts[1].1.contains("URL=git@github.com:evil/r.git"));
}

#[test]
fn a_denial_runs_nothing_and_is_logged() {
    let rig = Rig::new("broker-deny", Choice::Deny);
    let response = rig.send(&outside("id"));
    assert_eq!(response.status, Status::Denied);
    assert!(rig.exec.actions().is_empty());
    assert!(rig.log().contains("decision deny"));
}

#[test]
fn gh_reads_may_be_remembered_and_writes_may_not() {
    let rig = Rig::new("broker-gh", Choice::Repo);
    let read = gh(&["pr", "list"]);
    assert_eq!(rig.send(&read).status, Status::Done);
    assert_eq!(rig.send(&read).status, Status::Done);
    assert_eq!(rig.approver.asked(), 1);
    assert_eq!(rig.exec.actions(), ["gh pr list", "gh pr list"]);

    let write = gh(&["pr", "merge", "3"]);
    assert_eq!(rig.send(&write).status, Status::Denied);
    assert_eq!(rig.approver.last_remember(), Remember::Never);
    rig.approver.set(Choice::Once);
    assert_eq!(rig.send(&write).status, Status::Done);
    assert_eq!(rig.send(&write).status, Status::Done);
    assert_eq!(rig.approver.asked(), 4);
}

#[test]
fn gh_aimed_at_another_repository_or_host_is_a_write_asked_every_time() {
    let rig = Rig::new("broker-gh-bound", Choice::Repo);
    let cases = [
        gh(&["pr", "view", "1", "--repo", "other/x"]),
        gh(&["pr", "view", "1", "-R=other/x"]),
        gh(&["pr", "view", "1", "--hostname=evil.example"]),
        gh(&["api", "repos/other/x/pulls"]),
        gh(&["api", "graphql"]),
        gh(&["api", "/graphql"]),
        gh(&["api", "graphql?x=1"]),
    ];
    for request in &cases {
        assert_eq!(rig.send(request).status, Status::Denied, "{request}");
        assert_eq!(rig.approver.last_remember(), Remember::Never, "{request}");
    }
    assert!(!rig.approvals_file().exists());
    let own = gh(&["api", "repos/o/r/pulls"]);
    assert_eq!(rig.send(&own).status, Status::Done);
    assert_eq!(rig.approver.last_remember(), Remember::Repo);
}

#[test]
fn open_url_is_never_remembered() {
    let rig = Rig::new("broker-url", Choice::Once);
    let open = json!({"tool": "open_url", "args": {"url": "https://example.com/a"}});
    assert_eq!(rig.send(&open).status, Status::Done);
    assert_eq!(rig.send(&open).status, Status::Done);
    assert_eq!(rig.approver.asked(), 2);
    assert_eq!(rig.approver.last_remember(), Remember::Never);
    assert_eq!(
        rig.exec.actions(),
        ["open https://example.com/a", "open https://example.com/a"]
    );
    assert!(!rig.approvals_file().exists());
    rig.approver.set(Choice::Repo);
    assert_eq!(rig.send(&open).status, Status::Denied);
    for bad in ["file:///etc/passwd", "javascript:1", "https://a b"] {
        let response = rig.send(&json!({"tool": "open_url", "args": {"url": bad}}));
        assert_eq!(response.status, Status::Error, "{bad}");
    }
}

#[test]
fn run_outside_is_asked_every_time_and_runs_zsh_unhardened() {
    let rig = Rig::new("broker-outside", Choice::Repo);
    let request = outside("echo hi");
    assert_eq!(rig.send(&request).status, Status::Denied);
    rig.approver.set(Choice::Once);
    assert_eq!(rig.send(&request).status, Status::Done);
    assert_eq!(rig.send(&request).status, Status::Done);
    assert_eq!(rig.approver.asked(), 3);
    assert_eq!(rig.exec.actions()[0], "/bin/zsh -lc echo hi");
    assert!(rig.exec.calls.lock().unwrap()[0].bound.is_none());
}

#[test]
fn the_working_directory_cannot_be_chosen_by_the_request() {
    let rig = Rig::new("broker-cwd", Choice::Once);
    let request = json!({"tool": "run_outside", "args": {"command": "id", "cwd": "/"}});
    assert_eq!(rig.send(&request).status, Status::Error);
    assert_eq!(rig.approver.asked(), 0);
}

#[test]
fn requests_with_hidden_characters_are_refused_not_cleaned() {
    let rig = Rig::new("broker-chars", Choice::Once);
    let hidden = [
        '\n',
        '\r',
        '\t',
        '\u{0}',
        '\u{1b}',
        '\u{85}',
        '\u{ad}',
        '\u{34f}',
        '\u{115f}',
        '\u{1160}',
        '\u{180b}',
        '\u{200b}',
        '\u{202e}',
        '\u{2028}',
        '\u{2029}',
        '\u{2066}',
        '\u{3164}',
        '\u{fe0f}',
        '\u{feff}',
        '\u{ffa0}',
        '\u{e0041}',
    ];
    for c in hidden {
        let requests = [
            outside(&format!("echo a{c}b")),
            gh(&["pr", "list", &format!("--search=a{c}b")]),
            json!({"tool": "open_url", "args": {"url": format!("https://a.b/{c}")}}),
            push(&json!({"remote": format!("origin{c}")})),
        ];
        for request in &requests {
            let response = rig.send(request);
            assert_eq!(
                response.status,
                Status::Error,
                "{:04x} {request}",
                u32::from(c)
            );
        }
    }
    assert!(outside_message(&rig, '\u{202e}').contains("U+202E"));
    assert_eq!(rig.approver.asked(), 0);
    assert!(rig.exec.actions().is_empty());
    assert_eq!(rig.send(&outside("echo Grüße 世界")).status, Status::Done);
}

fn outside_message(rig: &Rig, c: char) -> String {
    rig.send(&outside(&format!("echo a{c}b"))).message
}

#[test]
fn long_or_malformed_requests_are_refused_without_asking() {
    let rig = Rig::new("broker-bad", Choice::Once);
    let long = "x".repeat(2001);
    let requests = [
        outside(&long),
        outside("  "),
        gh(&[]),
        gh(&[&long]),
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
    let response = rig.send(&outside("id"));
    assert_eq!(response.status, Status::Error);
    assert!(response.message.contains("log"));
    assert!(rig.exec.actions().is_empty());
}

#[test]
fn a_request_while_another_waits_for_the_dialog_gets_a_busy_error() {
    let rig = Rig::new("broker-busy", Choice::Once);
    let (entered_tx, entered) = channel();
    let (release, release_rx) = channel();
    *rig.approver.hold.lock().unwrap() = Some((entered_tx, release_rx));
    let handler = Arc::new(rig.handler);
    let first = {
        let handler = Arc::clone(&handler);
        std::thread::spawn(move || handler.handle(&outside("first").to_string()))
    };
    entered.recv().unwrap();
    let second = handler.handle(&outside("second").to_string());
    assert_eq!(second.status, Status::Error);
    assert!(second.message.starts_with("busy"), "{}", second.message);
    release.send(()).unwrap();
    assert_eq!(first.join().unwrap().status, Status::Done);
    assert_eq!(
        handler.handle(&outside("third").to_string()).status,
        Status::Done
    );
    assert_eq!(rig.approver.asked(), 2);
}

fn ask_socket(path: &Path, request: &str) -> Value {
    let mut stream = UnixStream::connect(path).unwrap();
    writeln!(stream, "{request}").unwrap();
    let mut line = String::new();
    BufReader::new(&stream).read_line(&mut line).unwrap();
    serde_json::from_str(&line).unwrap()
}

#[test]
fn the_socket_answers_requests_and_a_slow_client_blocks_nobody() {
    let rig = Rig::new("broker-socket", Choice::Once);
    let socket = rig.scratch.path().join("b.sock");
    let registry = Arc::new(Registry::default());
    let (broker, path) = serve(&socket, Arc::new(rig.handler), Arc::clone(&registry)).unwrap();
    let mode = std::fs::metadata(&path).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o600);

    let mut dripping = UnixStream::connect(&path).unwrap();
    dripping.write_all(b"{\"tool\":").unwrap();
    let idle = UnixStream::connect(&path).unwrap();

    let reply = ask_socket(&path, r#"{"tool":"run_outside","args":{"command":"id"}}"#);
    assert_eq!(reply["status"], "done");
    assert_eq!(reply["exit_code"], 0);
    assert_eq!(ask_socket(&path, "garbage")["status"], "error");
    drop((dripping, idle));

    drop(broker);
    let blocked = exec::run_capped(
        std::process::Command::new("/usr/bin/true"),
        exec::TIMEOUT,
        10,
        &registry,
    );
    assert!(
        blocked.is_err(),
        "the registry is closed when the broker drops"
    );
}

#[test]
fn gh_runs_in_the_recorded_root_even_when_the_working_directory_is_a_subdirectory() {
    let mut rig = Rig::new("broker-gh-root", Choice::Once);
    let root = rig.handler.cwd.clone();
    let sub = root.join("sub");
    std::fs::create_dir_all(sub.join(".git")).unwrap();
    rig.handler.cwd = sub;
    assert_eq!(rig.send(&gh(&["pr", "list"])).status, Status::Done);
    let calls = rig.exec.calls.lock().unwrap();
    assert!(
        calls
            .iter()
            .all(|call| call.cwd == root && call.bound.as_ref() == Some(&root))
    );
    assert!(calls.iter().any(|call| call.line == "gh pr list"));
}

#[test]
fn gh_searches_are_writes_and_a_remembered_read_is_bound_to_the_origin() {
    let rig = Rig::new("broker-gh-search", Choice::Repo);
    for args in [
        &["issue", "list", "--search", "repo:other/x"][..],
        &["issue", "list", "--search=repo:other/x"],
        &["pr", "list", "-S", "repo:other/x"],
        &["pr", "list", "-Srepo:other/x"],
    ] {
        assert_eq!(rig.send(&gh(args)).status, Status::Denied, "{args:?}");
        assert_eq!(rig.approver.last_remember(), Remember::Never, "{args:?}");
    }
    let read = gh(&["pr", "list"]);
    assert_eq!(rig.send(&read).status, Status::Done);
    assert_eq!(rig.send(&read).status, Status::Done);
    let before = rig.approver.asked();
    rig.exec.answer(
        "git remote get-url --push origin",
        0,
        "git@github.com:evil/r.git\n",
    );
    assert_eq!(rig.send(&read).status, Status::Done);
    assert_eq!(rig.approver.asked(), before + 1);
}
