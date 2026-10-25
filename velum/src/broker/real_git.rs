// SPDX-License-Identifier: MPL-2.0
use super::exec::{Executor, Registry, System};
use super::repo::Recorded;
use super::request::{GitPushArgs, Refusal, Request};
use super::tools::plan;
use crate::gitguard;
use crate::policy::Policy;
use crate::scratch::Scratch;
use crate::srt;
use serde_json::Value;
use std::ffi::OsStr;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::Arc;
use std::time::Duration;

fn git(dir: &Path, args: &[&str]) -> Output {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@example.com")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@example.com")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    out
}

/// A repository with one commit on `feat/x`.
fn init_repo(dir: &Path) {
    std::fs::create_dir_all(dir).unwrap();
    git(dir, &["init", "-q"]);
    git(dir, &["checkout", "-q", "-b", "feat/x"]);
    git(dir, &["commit", "-q", "--allow-empty", "-m", "first"]);
}

/// The srt settings `velum run` would build for `work`: the built-in policy, the deny entries
/// from the real `gitguard::Start`, and `extra_write` as additional writable paths.
fn run_settings(work: &Path, extra_write: &[&Path]) -> Value {
    let mut policy = Policy::builtin(work);
    // The scratch directories lie under the temporary directory, which the defaults write-allow.
    policy
        .write
        .retain(|entry| !matches!(entry.as_str(), "/private/tmp" | "/private/var/folders"));
    policy
        .write
        .extend(extra_write.iter().map(|path| path.display().to_string()));
    let start = gitguard::Start::capture(work);
    policy.deny_write.extend(start.deny_entries());
    srt::settings(&policy, "/Users/test").unwrap()
}

fn root_of(scratch: &Scratch) -> PathBuf {
    std::fs::canonicalize(scratch.path()).unwrap()
}

#[test]
fn a_real_repository_resolves_from_the_real_gitguard_denies() {
    let scratch = Scratch::new("real-resolve");
    let work = root_of(&scratch).join("work");
    init_repo(&work);
    std::fs::create_dir_all(work.join("sub/deeper")).unwrap();
    let settings = run_settings(&work, &[]);
    let recorded = Recorded::resolve(&work, &settings).unwrap();
    assert_eq!(recorded.root, work);
    assert_eq!(
        Recorded::resolve(&work.join("sub/deeper"), &settings)
            .unwrap()
            .root,
        work
    );
    assert!(recorded.verify().is_ok());
}

#[test]
fn a_repository_whose_config_the_policy_makes_writable_is_refused() {
    let scratch = Scratch::new("real-open");
    let work = root_of(&scratch).join("work");
    init_repo(&work);
    let mut settings = run_settings(&work, &[]);
    settings["filesystem"]["denyWrite"] = Value::Array(Vec::new());
    let err = Recorded::resolve(&work, &settings).unwrap_err();
    assert!(err.contains("writable inside the sandbox"), "{err}");
}

#[test]
fn a_git_link_or_a_missing_repository_is_refused() {
    let scratch = Scratch::new("real-link");
    let base = root_of(&scratch);
    init_repo(&base.join("elsewhere"));
    let work = base.join("work");
    std::fs::create_dir_all(&work).unwrap();
    std::os::unix::fs::symlink(base.join("elsewhere/.git"), work.join(".git")).unwrap();
    assert!(Recorded::resolve(&work, &run_settings(&work, &[])).is_err());

    let empty = base.join("empty");
    std::fs::create_dir_all(&empty).unwrap();
    assert!(Recorded::resolve(&empty, &run_settings(&empty, &[])).is_err());
}

#[test]
fn a_linked_worktree_resolves_unless_its_admin_directory_is_writable() {
    let scratch = Scratch::new("real-worktree");
    let base = root_of(&scratch);
    let main = base.join("main");
    init_repo(&main);
    let work = base.join("linked");
    git(&main, &["worktree", "add", "-q", work.to_str().unwrap()]);
    let recorded = Recorded::resolve(&work, &run_settings(&work, &[])).unwrap();
    assert_eq!(recorded.root, work);
    assert_eq!(
        recorded.git_dir().parent().unwrap().file_name().unwrap(),
        "worktrees"
    );

    let open_main = run_settings(&work, &[&main]);
    assert!(Recorded::resolve(&work, &open_main).is_err());
    let admin = recorded.git_dir().to_path_buf();
    let open_admin = run_settings(&work, &[&admin]);
    let err = Recorded::resolve(&work, &open_admin).unwrap_err();
    assert!(err.contains("writable inside the sandbox"), "{err}");
}

#[test]
fn the_git_environment_names_the_recorded_repository_and_keeps_the_system_config() {
    let scratch = Scratch::new("real-env");
    let work = root_of(&scratch).join("work");
    init_repo(&work);
    let recorded = Recorded::resolve(&work, &run_settings(&work, &[])).unwrap();
    let argv = ["gh".to_string(), "pr".to_string(), "list".to_string()];
    let command = System::command(&work, &argv, Some(&recorded)).unwrap();
    let envs: Vec<(&OsStr, Option<&OsStr>)> = command.get_envs().collect();
    let value = |name: &str| {
        envs.iter()
            .find(|(key, _)| *key == OsStr::new(name))
            .map(|(_, value)| value.and_then(|v| v.to_str()))
    };
    assert_eq!(value("GIT_DIR"), Some(recorded.git_dir().to_str()));
    assert_eq!(value("GIT_WORK_TREE"), Some(work.to_str()));
    assert_eq!(
        value("GIT_COMMON_DIR"),
        Some(recorded.common_dir().to_str())
    );
    for name in ["GIT_INDEX_FILE", "GIT_CONFIG", "GIT_CONFIG_PARAMETERS"] {
        assert_eq!(value(name), Some(None), "{name} is removed");
    }
    assert_eq!(value("GIT_CONFIG_NOSYSTEM"), None);
    assert_eq!(value("GIT_CONFIG_COUNT"), Some(Some("2")));
    assert_eq!(value("GIT_CONFIG_KEY_0"), Some(Some("core.hooksPath")));
    assert_eq!(value("GIT_CONFIG_VALUE_0"), Some(Some("/dev/null")));
    assert_eq!(value("GIT_CONFIG_KEY_1"), Some(Some("core.fsmonitor")));
    assert_eq!(value("GIT_CONFIG_VALUE_1"), Some(Some("false")));
    let plain = System::command(&work, &argv, None).unwrap();
    assert!(
        plain
            .get_envs()
            .all(|(key, _)| key != OsStr::new("GIT_DIR"))
    );
}

fn write_hook(work: &Path, name: &str, marker: &Path) {
    let hook = work.join(".git/hooks").join(name);
    std::fs::write(&hook, format!("#!/bin/sh\ntouch '{}'\n", marker.display())).unwrap();
    std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
}

#[test]
fn a_real_push_succeeds_without_running_the_repositorys_hooks() {
    let scratch = Scratch::new("real-push");
    let base = root_of(&scratch);
    let remote = base.join("remote.git");
    std::fs::create_dir_all(&remote).unwrap();
    git(&remote, &["init", "-q", "--bare"]);
    let work = base.join("work");
    init_repo(&work);
    git(
        &work,
        &["remote", "add", "origin", remote.to_str().unwrap()],
    );
    let marker = base.join("hook-ran");
    for hook in ["pre-push", "reference-transaction"] {
        write_hook(&work, hook, &marker);
    }

    // Without the broker's options the hooks do run, so the test can tell.
    git(&work, &["push", "-q", "origin", "HEAD:refs/heads/sanity"]);
    assert!(marker.exists(), "the hooks must fire in a plain push");
    std::fs::remove_file(&marker).unwrap();

    let recorded = Recorded::resolve(&work, &run_settings(&work, &[])).unwrap();
    let system = System {
        timeout: Duration::from_secs(60),
        registry: Arc::new(Registry::default()),
    };
    let request = Request::GitPush(GitPushArgs::default());
    let planned = plan(&system, &work, &Ok(recorded.clone()), &request).unwrap();
    let ran = system
        .run(&recorded.root, &planned.argv, Some(&recorded))
        .unwrap();
    assert_eq!(ran.code, 0, "{}", ran.output);
    assert!(!marker.exists(), "no hook may run during a broker push");
    let pushed = git(&remote, &["rev-parse", "refs/heads/feat/x"]);
    let local = git(&work, &["rev-parse", "HEAD"]);
    assert_eq!(pushed.stdout, local.stdout);
}

/// A bare remote, a repository with `origin` pointing at it, and a script that records that it
/// ran. Returns `(remote, work, marker, script)`.
fn push_setup(base: &Path) -> (PathBuf, PathBuf, PathBuf, PathBuf) {
    let remote = base.join("remote.git");
    std::fs::create_dir_all(&remote).unwrap();
    git(&remote, &["init", "-q", "--bare"]);
    let work = base.join("work");
    init_repo(&work);
    git(
        &work,
        &["remote", "add", "origin", remote.to_str().unwrap()],
    );
    let marker = base.join("ran");
    let script = base.join("receive.sh");
    let body = format!(
        "#!/bin/sh\ntouch '{}'\nexec git receive-pack \"$@\"\n",
        marker.display()
    );
    std::fs::write(&script, body).unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    (remote, work, marker, script)
}

fn receivepack_config(script: &Path) -> String {
    format!(
        "[remote \"origin\"]\n\treceivepack = {}\n",
        script.display()
    )
}

fn append(path: &Path, text: &str) {
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new().append(true).open(path).unwrap();
    file.write_all(text.as_bytes()).unwrap();
}

fn system() -> System {
    System {
        timeout: Duration::from_secs(60),
        registry: Arc::new(Registry::default()),
    }
}

fn push_request() -> Request {
    Request::GitPush(GitPushArgs::default())
}

/// Pushes without the broker's options and reports whether the planted program ran.
fn plain_push_runs_it(work: &Path, marker: &Path) -> bool {
    let out = Command::new("git")
        .arg(format!("--git-dir={}", work.join(".git").display()))
        .args(["push", "-q", "origin", "HEAD:refs/heads/sanity"])
        .current_dir(work)
        .output()
        .unwrap();
    let ran = marker.exists();
    let _ = std::fs::remove_file(marker);
    assert!(
        ran,
        "the planted config must take effect without the broker: {out:?}"
    );
    ran
}

#[test]
fn a_planted_commondir_is_refused_and_even_unverified_git_ignores_it() {
    let scratch = Scratch::new("real-commondir");
    let base = root_of(&scratch);
    let (remote, work, marker, script) = push_setup(&base);
    let recorded = Recorded::resolve(&work, &run_settings(&work, &[])).unwrap();
    let sys = system();
    let planned = plan(&sys, &work, &Ok(recorded.clone()), &push_request()).unwrap();

    let evil = base.join("evil");
    git(
        &base,
        &[
            "clone",
            "-q",
            "--no-hardlinks",
            "--bare",
            work.to_str().unwrap(),
            "evil",
        ],
    );
    append(&evil.join("config"), &receivepack_config(&script));
    std::fs::write(work.join(".git/commondir"), format!("{}\n", evil.display())).unwrap();
    plain_push_runs_it(&work, &marker);

    let again = plan(&sys, &work, &Ok(recorded.clone()), &push_request());
    assert!(matches!(again, Err(Refusal::RepoChanged)), "{again:?}");
    assert_eq!(recorded.verify(), Err(Refusal::RepoChanged));

    let ran = sys
        .run(&recorded.root, &planned.argv, Some(&recorded))
        .unwrap();
    assert!(
        !marker.exists(),
        "GIT_COMMON_DIR must keep git on the recorded common directory"
    );
    assert_eq!(ran.code, 0, "{}", ran.output);
    let pushed = git(&remote, &["rev-parse", "refs/heads/feat/x"]);
    assert_eq!(pushed.stdout, git(&work, &["rev-parse", "HEAD"]).stdout);
}

#[test]
fn a_planted_worktree_config_is_refused() {
    let scratch = Scratch::new("real-worktree-config");
    let base = root_of(&scratch);
    let (_remote, work, marker, script) = push_setup(&base);
    git(&work, &["config", "core.repositoryformatversion", "1"]);
    git(&work, &["config", "extensions.worktreeConfig", "true"]);
    let recorded = Recorded::resolve(&work, &run_settings(&work, &[])).unwrap();
    std::fs::write(
        work.join(".git/config.worktree"),
        receivepack_config(&script),
    )
    .unwrap();
    plain_push_runs_it(&work, &marker);
    let refused = plan(&system(), &work, &Ok(recorded), &push_request());
    assert!(matches!(refused, Err(Refusal::RepoChanged)), "{refused:?}");
    assert!(!marker.exists());
}

#[test]
fn a_changed_repository_config_is_refused() {
    let scratch = Scratch::new("real-config-changed");
    let base = root_of(&scratch);
    let (_remote, work, _marker, script) = push_setup(&base);
    let recorded = Recorded::resolve(&work, &run_settings(&work, &[])).unwrap();
    assert!(recorded.verify().is_ok());
    append(&work.join(".git/config"), &receivepack_config(&script));
    assert_eq!(recorded.verify(), Err(Refusal::RepoChanged));
}

#[test]
fn includes_that_lead_to_writable_locations_are_refused() {
    let scratch = Scratch::new("real-include");
    let base = root_of(&scratch);
    let (_remote, work, _marker, _script) = push_setup(&base);
    let open = base.join("open");
    std::fs::create_dir_all(&open).unwrap();
    let closed = base.join("closed");
    std::fs::create_dir_all(&closed).unwrap();
    let settings = run_settings(&work, &[&open]);
    assert!(Recorded::resolve(&work, &settings).is_ok());

    std::fs::write(closed.join("safe.cfg"), "[user]\n\tname = t\n").unwrap();
    git(
        &work,
        &[
            "config",
            "include.path",
            closed.join("safe.cfg").to_str().unwrap(),
        ],
    );
    assert!(Recorded::resolve(&work, &settings).is_ok());

    std::fs::write(
        closed.join("nested.cfg"),
        format!("[include]\n\tpath = {}\n", open.join("x.cfg").display()),
    )
    .unwrap();
    git(
        &work,
        &[
            "config",
            "--add",
            "include.path",
            closed.join("nested.cfg").to_str().unwrap(),
        ],
    );
    let err = Recorded::resolve(&work, &settings).unwrap_err();
    assert!(
        err.contains("includes") && err.contains("writable"),
        "{err}"
    );
    git(&work, &["config", "--unset-all", "include.path"]);

    git(
        &work,
        &[
            "config",
            "include.path",
            open.join("missing.cfg").to_str().unwrap(),
        ],
    );
    assert!(Recorded::resolve(&work, &settings).is_err());
    git(&work, &["config", "--unset-all", "include.path"]);

    let conditional = "includeIf.gitdir:/nowhere/.path";
    git(
        &work,
        &["config", conditional, open.join("x.cfg").to_str().unwrap()],
    );
    assert!(Recorded::resolve(&work, &settings).is_err());
}
