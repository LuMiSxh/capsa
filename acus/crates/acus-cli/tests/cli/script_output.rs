// SPDX-License-Identifier: MPL-2.0
use crate::common::*;
use std::path::{Path, PathBuf};

fn project() -> PathBuf {
    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/proj"))
}

/// A repository with two commits of `a.rs` and an uncommitted change to it.
fn repo() -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    let p = d.path();
    let (first, second) = ("2024-01-10T12:00:00+0000", "2024-02-01T12:00:00+0000");
    git_at(p, first, &["init", "-q"]);
    let write = |text: &str| std::fs::write(p.join("a.rs"), text).unwrap();
    write("fn one() {\n    1\n}\n\nfn two() {\n    2\n}\n");
    git_at(p, first, &["add", "."]);
    git_at(p, first, &["commit", "-qm", "first"]);
    write("fn one() {\n    1\n}\n\nfn two() {\n    22\n}\n");
    git_at(p, second, &["commit", "-qam", "second"]);
    write("fn one() {\n    11\n}\n\nfn two() {\n    22\n}\n");
    d
}

fn script_in(dir: &Path, args: &[&str], code: &str) -> (i32, String, String) {
    acus_in(dir, &[&["script"], args].concat(), code)
}

fn stdout(dir: &Path, args: &[&str]) -> String {
    let (code, out, err) = acus_in(dir, args, "");
    assert_eq!(code, 0, "{err}");
    assert!(!out.is_empty());
    out
}

#[test]
fn printing_a_result_gives_the_text_its_command_prints() {
    let d = repo();
    let cases = [
        ("await outline('a.rs')", vec!["outline", "a.rs"]),
        (
            "await outline('a.rs', {depth: 1})",
            vec!["outline", "a.rs", "--depth", "1"],
        ),
        ("await diff({p: true})", vec!["diff", "-p"]),
        ("await log({n: 1})", vec!["log", "-n", "1"]),
        ("await blame('a.rs#two')", vec!["blame", "a.rs#two"]),
    ];
    for (js, cli) in cases {
        let (code, out, err) = script_in(d.path(), &[], &format!("print({js});"));
        assert_eq!(code, 0, "{js}: {err}");
        assert_eq!(out, stdout(d.path(), &cli), "{js}");
    }

    let follow = tempfile::tempdir().unwrap();
    std::fs::write(
        follow.path().join("lib.rs"),
        "fn a() {\n    b();\n}\n\nfn b() {}\n",
    )
    .unwrap();
    let (code, out, err) = script_in(follow.path(), &[], "return await follow('lib.rs#b');");
    assert_eq!(code, 0, "{err}");
    assert_eq!(out, stdout(follow.path(), &["follow", "lib.rs#b"]));
}

#[test]
fn a_filtered_result_and_a_plain_object_print_as_compact_json() {
    let d = repo();
    let (_, out, err) = script_in(
        d.path(),
        &[],
        "print((await outline('a.rs')).head(1)); return {a: [1, 2]};",
    );
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines.len(), 2, "{out}{err}");
    assert!(
        lines[0].starts_with("{\"kind\":\"outline\",\"items\":["),
        "{out}"
    );
    assert_eq!(lines[1], "{\"a\":[1,2]}");
}

const TEN_THOUSAND: &str = "return Array.from({length: 10000}, (_, i) => 'line ' + i).join('\\n');";

#[test]
fn a_long_return_value_is_cut_with_a_footer_naming_the_options() {
    let (code, out, err) = script_in(&project(), &[], TEN_THOUSAND);
    assert_eq!(code, 0, "{err}");
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines.len(), 401, "{out}");
    assert_eq!(lines[399], "line 399");
    assert_eq!(
        lines[400],
        "… cut 9600 lines; return less, use .head()/.grep(), or rerun with --max-output N"
    );

    let code = "print(Array.from({length: 1000}, (_, i) => 'p' + i).join('\\n')); return 'RESULT';";
    let (exit, out, err) = script_in(&project(), &[], code);
    assert_eq!(exit, 0, "{err}");
    assert!(out.contains("\nRESULT\n"), "{out}");
    assert!(out.contains("p0\n") && !out.contains("p1000"), "{out}");
    assert!(out.contains("… cut "), "{out}");
}

#[test]
fn max_output_and_the_config_set_the_lines_shown() {
    let (_, out, err) = script_in(&project(), &["--max-output", "5"], TEN_THOUSAND);
    assert_eq!(out.lines().count(), 6, "{out}{err}");

    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("acus.toml");
    std::fs::write(&config, "[script]\noutput = { lines = 3, bytes = 1000 }\n").unwrap();
    let env = [("ACUS_CONFIG", config.to_str().unwrap())];
    let (_, out, err) = acus_in_env(&project(), &["script"], TEN_THOUSAND, &env);
    assert_eq!(out.lines().count(), 4, "{out}{err}");
    let (_, out, _) = acus_in_env(
        &project(),
        &["script", "--max-output", "7"],
        TEN_THOUSAND,
        &env,
    );
    assert_eq!(out.lines().count(), 8, "{out}");
}

#[test]
fn json_output_is_not_capped_and_has_no_text_field() {
    let (code, out, err) = script_in(&project(), &["--json"], TEN_THOUSAND);
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("line 9999"), "{}", &out[..200]);

    let d = repo();
    let (_, out, _) = script_in(d.path(), &["--json"], "return await outline('a.rs');");
    let value: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert!(value["result"].get("text").is_none(), "{out}");
}

#[test]
fn json_failure_lists_unawaited_calls_apart_from_the_prints() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join(".git")).unwrap();
    let code = r"print('before');
patch('*** Begin Patch\n*** Add File: unawaited.txt\n+hi\n*** End Patch');
throw new Error('boom');";
    let (exit, out, err) = script_in(dir.path(), &["--json"], code);
    assert_eq!(exit, 2, "{err}");
    let value: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(value["prints"], serde_json::json!(["before"]), "{out}");
    assert_eq!(value["unawaited"].as_array().unwrap().len(), 1, "{out}");
    assert_eq!(value["unawaited"][0]["kind"], "patch", "{out}");
}
