// SPDX-License-Identifier: MPL-2.0
use crate::common::*;
use std::path::PathBuf;
use std::time::{Duration, Instant};

fn project() -> PathBuf {
    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/proj"))
}

fn script(args: &[&str], code: &str) -> (i32, String, String) {
    acus_in(&project(), &[&["script"], args].concat(), code)
}

#[test]
fn script_prints_the_return_value_or_the_last_expression() {
    assert_eq!(
        acus(&["script", "-e", "return 1 + 1"]),
        (0, "2\n".to_owned(), String::new())
    );
    assert_eq!(
        acus(&["script", "-e", "find('let needle')"]),
        (
            0,
            "src/lib.rs:11: let needle = input.len();\n".to_owned(),
            String::new()
        )
    );
}

#[test]
fn script_sleep_waits_for_its_milliseconds() {
    let started = Instant::now();
    let (code, out, err) = script(&[], "await sleep(120);\n'slept'");
    assert_eq!((code, out.as_str()), (0, "slept\n"), "{err}");
    assert!(started.elapsed() >= Duration::from_millis(120));
}

#[test]
fn script_sleep_past_the_time_limit_times_the_script_out() {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("acus.toml");
    std::fs::write(&config, "[script]\ntimeout = 1\n").unwrap();
    let started = Instant::now();
    let (code, _, err) = acus_in_env(
        &project(),
        &["script"],
        "await sleep(10000);\n'late'",
        &[("ACUS_CONFIG", config.to_str().unwrap())],
    );
    assert_eq!(code, 2, "{err}");
    assert!(err.contains("script timed out after 1s"), "{err}");
    assert!(started.elapsed() < Duration::from_secs(5));
}

#[test]
fn script_find_renders_and_filters_hits() {
    let code = "return await find('let needle')";
    assert_eq!(
        script(&[], code),
        (
            0,
            "src/lib.rs:11: let needle = input.len();\n".to_owned(),
            String::new()
        )
    );
    assert_eq!(
        script(&["--json"], code),
        (
            0,
            "{\"prints\":[],\"unawaited\":[],\"result\":{\"kind\":\"hits\",\"items\":[{\"path\":\"src/lib.rs\",\"line\":11,\"symbol\":\"Parser::parse\",\"text\":\"        let needle = input.len();\"}]}}\n"
                .to_owned(),
            String::new()
        )
    );
    let (code, out, _) = script(&[], "return (await find('pos')).grep('self').head(1)");
    assert_eq!((code, out.lines().count()), (0, 1));
    let (code, out, _) = script(&[], "return (await find('needle')).grep('zzz')");
    assert_eq!((code, out.as_str()), (1, ""));
}

#[test]
fn script_run_returns_code_text_and_parsed_json() {
    let (code, out, _) = script(
        &["--json"],
        "const r = await run('echo 7', {json: true}); return [r.ok, r.code, r.json]",
    );
    assert_eq!(code, 0);
    assert_eq!(
        out,
        "{\"prints\":[],\"unawaited\":[],\"result\":[true,0,7]}\n"
    );
    let (code, out, _) = script(&[], "return (await run('exit 3')).code");
    assert_eq!((code, out.as_str()), (0, "3\n"));
}

#[test]
fn script_run_refuses_what_the_guard_refuses_and_hints_the_acus_command() {
    let (code, out, err) = script(&[], "return await run('cat src/lib.rs')");
    assert_eq!((code, out.as_str()), (2, ""));
    assert!(
        err.starts_with("error: line 1: run() refused the command"),
        "{err}"
    );
    assert!(err.contains("hint: Use acus instead of cat"), "{err}");
}

#[test]
fn script_option_objects_become_flags_checked_by_the_command() {
    let (code, _, err) = script(&[], "return await find('x', {bogus: true})");
    assert_eq!(code, 2);
    assert!(
        err.starts_with("error: line 1: unexpected argument '--bogus'"),
        "{err}"
    );
}

#[test]
fn script_read_stays_inside_the_repository() {
    let outside = tempfile::NamedTempFile::new().unwrap();
    let path = outside.path().to_string_lossy().replace('\\', "/");
    let (code, _, err) = script(&[], &format!("return read('{path}')"));
    assert_eq!(code, 2);
    assert!(
        err.contains(&format!("{path} is outside the allowed roots")),
        "{err}"
    );
}

#[test]
fn script_syntax_error_exits_2_and_names_the_program_line() {
    let (code, _, err) = script(&[], "const a = 1;\nconst b = ;\n");
    assert_eq!(code, 2);
    assert!(err.starts_with("error: line 2: SyntaxError"), "{err}");
}

#[test]
fn script_keeps_what_it_printed_before_a_throw() {
    let (code, out, err) = script(&[], "print('before');\nthrow new Error('boom');");
    assert_eq!((code, out.as_str()), (2, "before\n"), "{err}");
    assert!(err.starts_with("error: line 2: boom"), "{err}");
    let (code, out, err) = script(&["--json"], "print('before');\nthrow new Error('boom');");
    assert_eq!(code, 2, "{err}");
    assert_eq!(
        out,
        "{\"prints\":[\"before\"],\"unawaited\":[],\"result\":null}\n"
    );
}

#[test]
fn script_failure_reports_a_patch_it_never_awaited() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join(".git")).unwrap();
    let code = r"patch('*** Begin Patch\n*** Add File: unawaited.txt\n+hi\n*** End Patch');
throw new Error('boom');";
    let (exit, out, err) = acus_in(dir.path(), &["script"], code);
    assert_eq!(exit, 2, "{err}");
    assert!(out.contains("unawaited.txt"), "{out}");
    assert!(dir.path().join("unawaited.txt").exists());
}

#[test]
fn script_keeps_what_it_printed_before_the_deadline() {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("acus.toml");
    std::fs::write(&config, "[script]\ntimeout = 1\n").unwrap();
    let config = config.display().to_string();
    let (code, out, err) = acus_in_env(
        &project(),
        &["script"],
        "print('before');\nwhile (true) {}",
        &[("ACUS_CONFIG", &config)],
    );
    assert_eq!((code, out.as_str()), (2, "before\n"), "{err}");
    assert!(err.starts_with("error: script timed out after 1s"), "{err}");
}

#[test]
fn script_patch_check_reports_the_change_without_writing() {
    let (code, out, err) = script(
        &["--json"],
        "const r = await patch('*** Begin Patch\\n*** Add File: new.txt\\n+hi\\n*** End Patch', {check: true}); return [r.kind, r.written, r.changes.length]",
    );
    assert_eq!(code, 0, "{err}");
    assert_eq!(
        out,
        "{\"prints\":[],\"unawaited\":[],\"result\":[\"patch\",false,1]}\n"
    );
    assert!(!project().join("new.txt").exists());
}

#[cfg(unix)]
#[test]
fn script_run_started_after_a_patch_sees_the_patched_file() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join(".git")).unwrap();
    let code = "const w = patch('*** Begin Patch\\n*** Add File: a.txt\\n+patched\\n*** End Patch');\n\
                const r = run('test -f a.txt');\n\
                return (await Promise.all([w, r]))[1].code;";
    let (exit, out, err) = acus_in(dir.path(), &["script"], code);
    assert_eq!((exit, out.as_str()), (0, "0\n"), "{err}");
}

#[test]
fn script_patch_with_a_refused_run_command_writes_nothing() {
    let (code, _, err) = script(
        &[],
        "await patch('*** Begin Patch\\n*** Add File: refused.txt\\n+hi\\n*** End Patch', {run: 'cat src/lib.rs'})",
    );
    assert_eq!(code, 2, "{err}");
    assert!(err.contains("run() refused the command"), "{err}");
    assert!(!project().join("refused.txt").exists());
}

#[cfg(unix)]
#[test]
fn script_read_refuses_dotdot_outside_symlinks_and_a_missing_repository() {
    let base = std::env::temp_dir().join(format!("acus-script-read-{}", std::process::id()));
    let repo = base.join("repo");
    let plain = base.join("plain");
    std::fs::create_dir_all(repo.join(".git")).unwrap();
    std::fs::create_dir_all(&plain).unwrap();
    std::fs::write(base.join("secret.txt"), "s").unwrap();
    std::fs::write(repo.join("ok.txt"), "fine").unwrap();
    std::fs::write(plain.join("ok.txt"), "fine").unwrap();
    std::os::unix::fs::symlink(base.join("secret.txt"), repo.join("link.txt")).unwrap();
    let cases = [
        (&repo, "ok.txt", None),
        (&repo, "../secret.txt", Some("is outside the allowed roots")),
        (&repo, "link.txt", Some("is outside the allowed roots")),
        (
            &plain,
            "../repo/ok.txt",
            Some("is outside the allowed roots"),
        ),
    ];
    for (dir, path, refusal) in cases {
        let (code, out, err) = acus_in(
            dir,
            &["script", "-e", &format!("return read('{path}')")],
            "",
        );
        match refusal {
            None => assert_eq!((code, out.as_str()), (0, "fine\n"), "{path}: {err}"),
            Some(text) => assert!(code == 2 && err.contains(text), "{path}: {err}"),
        }
    }
    std::fs::remove_dir_all(&base).unwrap();
}
