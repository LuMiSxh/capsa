// SPDX-License-Identifier: MPL-2.0

use super::*;
use capsa_core::claude_settings::permissions::Permissions;
use std::fmt::Write as _;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;

/// Serves `handler(path, base)` for each connection on a local port and returns the base URL.
/// The server runs until the test process exits.
fn serve(handler: impl Fn(&str, &str) -> Vec<u8> + Send + 'static) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind a local port");
    let base = format!("http://{}", listener.local_addr().expect("local address"));
    let server_base = base.clone();
    std::thread::spawn(move || {
        for mut stream in listener.incoming().flatten() {
            let path = request_path(&stream);
            let _ = stream.write_all(&handler(&path, &server_base));
        }
    });
    base
}

/// Returns the path of the request on `stream`, after reading its headers.
fn request_path(stream: &TcpStream) -> String {
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    let _ = reader.read_line(&mut line);
    let mut header = String::new();
    while reader.read_line(&mut header).is_ok_and(|n| n > 2) {
        header.clear();
    }
    line.split_whitespace().nth(1).unwrap_or("/").to_owned()
}

/// A raw HTTP/1.1 response that closes the connection after its body.
fn reply(status: &str, headers: &[(&str, &str)], body: &[u8]) -> Vec<u8> {
    let mut head = format!(
        "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n",
        body.len()
    );
    for (name, value) in headers {
        let _ = write!(head, "{name}: {value}\r\n");
    }
    head.push_str("\r\n");
    let mut out = head.into_bytes();
    out.extend_from_slice(body);
    out
}

/// Permissions with no rules.
fn open() -> Permissions {
    Permissions::with_files(Vec::new(), PathBuf::new(), PathBuf::new(), None)
}

fn config(deny: &[&str]) -> FetchConfig {
    FetchConfig {
        deny: deny.iter().map(|host| (*host).to_owned()).collect(),
    }
}

fn run(url: &str, deny: &[&str]) -> Result<FetchResult, HostError> {
    fetch(
        &open(),
        &config(deny),
        url,
        FetchOpts::default(),
        Duration::from_secs(10),
    )
}

#[test]
fn html_comes_back_as_markdown() {
    let base = serve(|_, _| {
        reply(
            "200 OK",
            &[("Content-Type", "text/html; charset=utf-8")],
            b"<h1>Title</h1><p>See <a href=\"/x\">the link</a>.</p>",
        )
    });
    let page = run(&format!("{base}/"), &[]).expect("fetch");
    assert_eq!(page.status, 200);
    assert!(page.text.contains("# Title"), "{}", page.text);
    assert!(page.text.contains("[the link](/x)"), "{}", page.text);
}

#[test]
fn text_comes_back_as_it_is_and_grep_then_head_filter_it() {
    let base = serve(|_, _| {
        reply(
            "200 OK",
            &[("Content-Type", "text/plain")],
            b"alpha\nbeta\nalpine\ngamma\n",
        )
    });
    let url = format!("{base}/t");
    let page = run(&url, &[]).expect("fetch");
    assert_eq!(page.text, "alpha\nbeta\nalpine\ngamma\n");
    assert_eq!(page.url, url);

    let opts = FetchOpts {
        grep: Some("^al".to_owned()),
        head: Some(1),
    };
    let filtered =
        fetch(&open(), &config(&[]), &url, opts, Duration::from_secs(10)).expect("fetch");
    assert_eq!(filtered.text, "alpha");

    let opts = FetchOpts {
        grep: Some("(".to_owned()),
        head: None,
    };
    let err = fetch(&open(), &config(&[]), &url, opts, Duration::from_secs(10))
        .err()
        .expect("an invalid regex is refused");
    assert!(err.message.contains("not a regex"), "{}", err.message);
}

#[test]
fn binary_is_refused_with_a_hint() {
    let base = serve(|_, _| {
        reply(
            "200 OK",
            &[("Content-Type", "image/png")],
            &[0x89, b'P', b'N', b'G', 0],
        )
    });
    let err = run(&format!("{base}/img.png"), &[])
        .err()
        .expect("binary is refused");
    assert!(err.message.contains("image/png"), "{}", err.message);
    assert!(err.hint.is_some_and(|h| h.contains("text")));
}

#[test]
fn a_denied_host_is_refused_before_any_request() {
    let err = run("http://denied.test/page", &["denied.test"])
        .err()
        .expect("refused");
    assert!(err.message.contains("denied.test"), "{}", err.message);
    let err = run("https://a.blocked.test/", &["*.blocked.test"])
        .err()
        .expect("refused");
    assert!(err.message.contains("a.blocked.test"), "{}", err.message);
}

#[test]
fn a_redirect_into_a_denied_host_or_scheme_is_refused() {
    let base = serve(|path, _| match path {
        "/denied" => reply(
            "302 Found",
            &[("Location", "http://sub.denied.test/secret")],
            b"",
        ),
        "/file" => reply("302 Found", &[("Location", "file:///etc/passwd")], b""),
        _ => reply("404 Not Found", &[], b""),
    });
    let err = run(&format!("{base}/denied"), &["*.denied.test"])
        .err()
        .expect("refused");
    assert!(err.message.contains("sub.denied.test"), "{}", err.message);
    let err = run(&format!("{base}/file"), &[]).err().expect("refused");
    assert!(
        err.message.contains("only http and https"),
        "{}",
        err.message
    );
}

#[test]
fn a_webfetch_deny_rule_refuses_the_host() {
    let base = serve(|_, _| reply("200 OK", &[], b"hi"));
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path().to_path_buf();
    let settings = root.join(".claude/settings.json");
    std::fs::create_dir_all(settings.parent().expect("parent")).expect("create dir");
    std::fs::write(
        &settings,
        r#"{"permissions":{"deny":["WebFetch(domain:127.0.0.1)"]}}"#,
    )
    .expect("write settings");
    let perms = Permissions::with_files(vec![settings], root.clone(), root, None);
    let err = fetch(
        &perms,
        &config(&[]),
        &format!("{base}/"),
        FetchOpts::default(),
        Duration::from_secs(10),
    )
    .err()
    .expect("refused");
    assert!(
        err.message.contains("WebFetch(domain:127.0.0.1)"),
        "{}",
        err.message
    );
}

#[test]
fn only_http_and_https_are_fetched() {
    for url in ["file:///etc/passwd", "ftp://example.test/file"] {
        let err = run(url, &[]).err().expect("refused");
        assert!(
            err.message.contains("only http and https"),
            "{}",
            err.message
        );
    }
}

#[test]
fn redirects_are_followed_to_the_final_url_and_capped() {
    let base = serve(|path, _| match path {
        "/start" => reply("301 Moved Permanently", &[("Location", "/final")], b""),
        "/final" => reply("200 OK", &[("Content-Type", "text/plain")], b"done"),
        "/loop" => reply("302 Found", &[("Location", "/loop")], b""),
        _ => reply("404 Not Found", &[], b""),
    });
    let page = run(&format!("{base}/start"), &[]).expect("fetch");
    assert_eq!(page.url, format!("{base}/final"));
    assert_eq!((page.status, page.text.as_str()), (200, "done"));

    let err = run(&format!("{base}/loop"), &[]).err().expect("capped");
    assert!(
        err.message.contains("redirects more than 5 times"),
        "{}",
        err.message
    );
}

#[test]
fn the_time_left_bounds_the_request() {
    let base = serve(|path, _| {
        if path == "/slow" {
            std::thread::sleep(Duration::from_secs(3));
        }
        reply("200 OK", &[], b"late")
    });
    let started = Instant::now();
    let err = fetch(
        &open(),
        &config(&[]),
        &format!("{base}/slow"),
        FetchOpts::default(),
        Duration::from_millis(300),
    )
    .err()
    .expect("timed out");
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "took {:?}",
        started.elapsed()
    );
    assert!(err.message.contains("timed out"), "{}", err.message);
}

#[test]
fn a_body_over_five_megabytes_is_refused() {
    let big = vec![b'a'; usize::try_from(MAX_BODY).expect("fits") + 1];
    let base = serve(move |_, _| reply("200 OK", &[("Content-Type", "text/plain")], &big));
    let err = run(&format!("{base}/big"), &[]).err().expect("refused");
    assert!(err.message.contains("over 5 MB"), "{}", err.message);
}
