// SPDX-License-Identifier: MPL-2.0
use acus_decide::{Question, body, parse_answer};
use serde_json::json;

#[test]
fn builds_systemone_body() {
    let q = Question::Choice(vec![
        ("ok".into(), "Build passed".into()),
        ("fail".into(), "Build failed".into()),
    ]);
    let b = body(
        "jev-latest",
        "log text",
        "Did the build pass?",
        &q,
        &["aGk=".into()],
    );
    assert_eq!(
        b,
        json!({
            "model": "jev-latest",
            "state": "log text",
            "questions": {"q": {"type": "choice", "instructions": "Did the build pass?",
                "criteria": {"ok": "Build passed", "fail": "Build failed"}}},
            "images": ["aGk="],
        })
    );
    let s = body(
        "m",
        "s",
        "i",
        &Question::Score(vec!["low".into(), "high".into()]),
        &[],
    );
    assert_eq!(s["questions"]["q"]["criteria"], json!(["low", "high"]));
    assert!(s.get("images").is_none());
    let y = body("m", "s", "i", &Question::YesNo, &[]);
    assert_eq!(
        y["questions"]["q"],
        json!({"type": "noul", "instructions": "i"})
    );
}

#[test]
fn parses_answers_including_workers_ai_wrapper() {
    let yes = parse_answer(
        &json!({"answers": {"q": {"type": "noul", "noul": 0.93}}}),
        &Question::YesNo,
    )
    .unwrap();
    assert_eq!(
        (yes.value.as_str(), yes.confidence, yes.yes),
        ("yes", 0.93, Some(true))
    );
    let no = parse_answer(
        &json!({"result": {"answers": {"q": {"type": "noul", "noul": 0.2}}}}),
        &Question::YesNo,
    )
    .unwrap();
    assert_eq!(
        (no.value.as_str(), no.confidence, no.yes),
        ("no", 0.8, Some(false))
    );
    let c = parse_answer(
        &json!({"answers": {"q": {"type": "choice", "choice": "fail", "confidence": 0.7}}}),
        &Question::Choice(vec![]),
    )
    .unwrap();
    assert_eq!((c.value.as_str(), c.confidence), ("fail", 0.7));
    let levels = Question::Score(vec!["low".into(), "high".into()]);
    let s = parse_answer(
        &json!({"answers": {"q": {"type": "score", "score": 1, "confidence": 0.6}}}),
        &levels,
    )
    .unwrap();
    assert_eq!(s.value, "high");
    // Real OpenRouter response shape: fractional score plus legend and probabilities.
    let s = parse_answer(
        &json!({"answers": {"q": {"type": "score", "score": 0.6, "legend": {"0": "low", "1": "high"}, "confidence": 0.55}}}),
        &levels,
    )
    .unwrap();
    assert_eq!(s.value, "high");
    assert!(
        parse_answer(&json!({"error": "bad key"}), &Question::YesNo)
            .unwrap_err()
            .to_string()
            .contains("bad key")
    );
}

#[test]
fn base64_matches_rfc4648() {
    assert_eq!(acus_decide::base64(b""), "");
    assert_eq!(acus_decide::base64(b"f"), "Zg==");
    assert_eq!(acus_decide::base64(b"fo"), "Zm8=");
    assert_eq!(acus_decide::base64(b"foobar"), "Zm9vYmFy");
}

#[test]
fn decide_posts_to_endpoint_with_bearer_token() {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/v1/systemone", listener.local_addr().unwrap());
    let server = std::thread::spawn(move || {
        let (mut s, _) = listener.accept().unwrap();
        s.set_read_timeout(Some(std::time::Duration::from_secs(5)))
            .unwrap();
        let mut req = Vec::new();
        let mut buf = [0u8; 4096];
        // Headers, then exactly Content-Length body bytes.
        let complete = |r: &[u8]| {
            let t = String::from_utf8_lossy(r).to_lowercase();
            let Some(h) = t.find("\r\n\r\n") else {
                return false;
            };
            let len = t
                .lines()
                .find_map(|l| l.strip_prefix("content-length:"))
                .and_then(|v| v.trim().parse::<usize>().ok())
                .unwrap_or(0);
            r.len() >= h + 4 + len
        };
        while !complete(&req) {
            let n = s.read(&mut buf).unwrap();
            assert!(n > 0, "client closed early");
            req.extend_from_slice(&buf[..n]);
        }
        let body =
            r#"{"model":"jev-1.13.0","answers":{"q":{"type":"noul","noul":0.9}},"usage":{}}"#;
        write!(s, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
        String::from_utf8(req).unwrap()
    });
    let ep = acus_decide::Endpoint {
        url,
        model: "jev-latest".into(),
        api_key: Some("k123".into()),
    };
    let a = acus_decide::decide(
        &ep,
        "all tests passed",
        "Did the build pass?",
        &Question::YesNo,
        &[],
    )
    .unwrap();
    assert_eq!((a.value.as_str(), a.yes), ("yes", Some(true)));
    let req = server.join().unwrap();
    let lower = req.to_lowercase();
    assert!(lower.starts_with("post /v1/systemone"), "{req}");
    assert!(lower.contains("authorization: bearer k123"), "{req}");
    let body: serde_json::Value =
        serde_json::from_str(req.split("\r\n\r\n").nth(1).unwrap()).unwrap();
    assert_eq!(body["state"], "all tests passed");
    assert_eq!(body["model"], "jev-latest");
}
