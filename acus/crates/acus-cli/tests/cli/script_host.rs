// SPDX-License-Identifier: MPL-2.0
use crate::common::*;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::path::PathBuf;

/// A git repository in a temporary directory with `files` written under it, and the user's Claude
/// Code settings and state hidden.
struct Sandbox {
    dir: tempfile::TempDir,
}

impl Sandbox {
    fn new(files: &[(&str, &str)]) -> Self {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("repo/.git")).unwrap();
        for (rel, text) in files {
            let file = dir.path().join("repo").join(rel);
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(file, text).unwrap();
        }
        Sandbox { dir }
    }

    fn repo(&self) -> PathBuf {
        self.dir.path().join("repo")
    }

    fn script(&self, program: &str, env: &[(&str, &str)]) -> (i32, String, String) {
        let path = |p: &str| self.dir.path().join(p).display().to_string();
        let hidden = [
            ("HOME", path("home")),
            ("USERPROFILE", path("home")),
            ("CLAUDE_CONFIG_DIR", path("claude")),
            ("ACUS_MANAGED_SETTINGS", path("managed.json")),
            ("XDG_STATE_HOME", path("state")),
            ("LOCALAPPDATA", path("state")),
        ];
        let mut vars: Vec<(&str, &str)> = hidden.iter().map(|(k, v)| (*k, v.as_str())).collect();
        vars.extend_from_slice(env);
        acus_in_env(&self.repo(), &["script"], program, &vars)
    }
}

#[test]
fn read_takes_offset_limit_and_numbered_options() {
    let sandbox = Sandbox::new(&[("notes.txt", "a\nb\nc\nd\n")]);
    let program = "return JSON.stringify([await read('notes.txt', {offset: 2, limit: 2}), await read('notes.txt', {offset: 3, limit: 1, numbered: true}), await read('notes.txt')]);";
    let (code, out, err) = sandbox.script(program, &[]);
    assert_eq!(code, 0, "{err}");
    assert_eq!(out, "[\"b\\nc\\n\",\"3\\tc\\n\",\"a\\nb\\nc\\nd\\n\"]\n");
}

#[test]
fn stat_and_exists_report_a_missing_path_without_failing() {
    let sandbox = Sandbox::new(&[("notes.txt", "a\nb\nc\nd\n")]);
    let program = "const s = await stat('notes.txt'); return JSON.stringify([s.exists, s.bytes, s.lines, s.dir, await stat('missing.txt'), await exists('notes.txt'), await exists('missing.txt')]);";
    let (code, out, err) = sandbox.script(program, &[]);
    assert_eq!(code, 0, "{err}");
    assert_eq!(
        out,
        "[true,8,4,false,{\"exists\":false,\"dir\":false},true,false]\n"
    );
}

#[test]
fn env_masks_the_value_of_a_secret_name() {
    let sandbox = Sandbox::new(&[]);
    let program = "return JSON.stringify([await env('ACUS_TEST_TOKEN'), await env('ACUS_TEST_NAME'), await env('ACUS_TEST_UNSET')]);";
    let env = [("ACUS_TEST_TOKEN", "s3cret"), ("ACUS_TEST_NAME", "plain")];
    let (code, out, err) = sandbox.script(program, &env);
    assert_eq!(code, 0, "{err}");
    assert_eq!(out, "[\"<set>\",\"plain\",null]\n");
}

#[test]
fn parse_toml_returns_the_document_as_json_and_refuses_invalid_text() {
    let sandbox = Sandbox::new(&[]);
    let program = r#"return JSON.stringify(await parse.toml('title = "x"\n[owner]\nname = "a"\nwhen = 1979-05-27T07:32:00Z\n'));"#;
    let (code, out, err) = sandbox.script(program, &[]);
    assert_eq!(code, 0, "{err}");
    assert_eq!(
        out,
        "{\"owner\":{\"name\":\"a\",\"when\":\"1979-05-27T07:32:00Z\"},\"title\":\"x\"}\n"
    );
    let (code, _, err) = sandbox.script("return await parse.toml('a =');", &[]);
    assert_eq!(code, 2, "{err}");
    assert!(err.contains("cannot parse TOML"), "{err}");

    let program = r#"return JSON.stringify(await parse.yaml('title: x\nowner:\n  name: a\n---\nlater: 1\n'));"#;
    let (code, out, err) = sandbox.script(program, &[]);
    assert_eq!(code, 0, "{err}");
    assert_eq!(out, "{\"title\":\"x\",\"owner\":{\"name\":\"a\"}}\n");
}

#[test]
fn text_diff_shows_each_change_with_the_context_asked_for() {
    let sandbox = Sandbox::new(&[]);
    let program = r#"return await textDiff('a\nb\nc\nd\ne\nf\ng\nh\n', 'a\nb\nc\nD\ne\nf\ng\nh\n', {context: 1});"#;
    let (code, out, err) = sandbox.script(program, &[]);
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("\n-d\n+D\n e\n"), "{out}");
    assert!(!out.contains("\n b\n"), "{out}");
}

/// Answers every connection on a local port with `body` as text, and returns the base URL. The
/// server runs until the test process exits.
fn serve(body: &'static str) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        for mut stream in listener.incoming().flatten() {
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut header = String::new();
            while reader.read_line(&mut header).is_ok_and(|n| n > 2) {
                header.clear();
            }
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
        }
    });
    base
}

#[test]
fn fetch_returns_status_url_and_text_with_grep_and_head_applied() {
    let base = serve("one\nskip\ntwo\n");
    let sandbox = Sandbox::new(&[]);
    let program =
        format!("return JSON.stringify(await fetch('{base}/page', {{grep: 'o', head: 1}}));");
    let (code, out, err) = sandbox.script(&program, &[]);
    assert_eq!(code, 0, "{err}");
    assert_eq!(
        out,
        format!("{{\"status\":200,\"url\":\"{base}/page\",\"text\":\"one\"}}\n")
    );

    let denied = serve("unused\n");
    let sandbox = Sandbox::new(&[]);
    let config = sandbox.dir.path().join("acus.toml");
    std::fs::write(&config, "[script]\nfetch_deny = [\"127.0.0.1\"]\n").unwrap();
    let program = format!("return await fetch('{denied}/page');");
    let env = [("ACUS_CONFIG", config.to_str().unwrap())];
    let (code, _, err) = sandbox.script(&program, &env);
    assert_eq!(code, 2, "{err}");
    assert!(
        err.contains("its host `127.0.0.1` is in fetch_deny"),
        "{err}"
    );
}
