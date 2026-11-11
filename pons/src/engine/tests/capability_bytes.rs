// SPDX-License-Identifier: MPL-2.0

use super::*;

/// The `initialize` line pons sends the agent for `caps`, byte for byte.
fn wire(caps: &str) -> String {
    format!(
        r#"{{"jsonrpc":"2.0","id":0,"method":"initialize","params":{{"protocolVersion":1,"clientCapabilities":{caps}}}}}"#
    )
}

fn sent_bytes(out: &[Action]) -> String {
    let sent: Vec<_> = out
        .iter()
        .filter_map(|a| match a {
            Action::ToAgent(l) => Some(String::from_utf8(l.clone()).unwrap()),
            _ => None,
        })
        .collect();
    assert_eq!(sent.len(), 1, "{out:?}");
    sent[0].clone()
}

struct Row {
    caps: &'static str,
    sent: &'static str,
    aside: bool,
    text: bool,
}

const fn row(caps: &'static str, sent: &'static str, aside: bool, text: bool) -> Row {
    Row {
        caps,
        sent,
        aside,
        text,
    }
}

/// Client capabilities, the capabilities the agent receives, whether pons steps aside and
/// whether it expects subagent text from the adapter, under the default flags.
const DEFAULT_FLAGS: &[Row] = &[
    // fs and terminal are forwarded as they came; only the transcript capability is added.
    row(
        r#"{"fs":{"readTextFile":true,"writeTextFile":false},"terminal":false}"#,
        r#"{"fs":{"readTextFile":true,"writeTextFile":false},"terminal":false,"_meta":{"subagent-transcript":true}}"#,
        false,
        true,
    ),
    row(
        r#"{"fs":{"readTextFile":false,"writeTextFile":false},"terminal":true}"#,
        r#"{"fs":{"readTextFile":false,"writeTextFile":false},"terminal":true,"_meta":{"subagent-transcript":true}}"#,
        false,
        true,
    ),
    row(
        r#"{"fs":null,"terminal":null}"#,
        r#"{"fs":null,"terminal":null,"_meta":{"subagent-transcript":true}}"#,
        false,
        true,
    ),
    // subagents present: aside, nothing added, same bytes.
    row(
        r#"{"fs":{"readTextFile":true},"terminal":true,"subagents":{}}"#,
        r#"{"fs":{"readTextFile":true},"terminal":true,"subagents":{}}"#,
        true,
        false,
    ),
    row(
        r#"{"subagents":true,"_meta":{"subagent-transcript":true}}"#,
        r#"{"subagents":true,"_meta":{"subagent-transcript":true}}"#,
        true,
        false,
    ),
    // subagents null or false: not aside.
    row(
        r#"{"subagents":null}"#,
        r#"{"subagents":null,"_meta":{"subagent-transcript":true}}"#,
        false,
        true,
    ),
    row(
        r#"{"subagents":false,"terminal":true}"#,
        r#"{"subagents":false,"terminal":true,"_meta":{"subagent-transcript":true}}"#,
        false,
        true,
    ),
    // subagent-transcript present: true is kept, false is respected, null is replaced.
    row(
        r#"{"fs":{},"_meta":{"subagent-transcript":true}}"#,
        r#"{"fs":{},"_meta":{"subagent-transcript":true}}"#,
        false,
        true,
    ),
    row(
        r#"{"fs":{},"_meta":{"subagent-transcript":false}}"#,
        r#"{"fs":{},"_meta":{"subagent-transcript":false}}"#,
        false,
        false,
    ),
    row(
        r#"{"_meta":{"subagent-transcript":null,"x":1}}"#,
        r#"{"_meta":{"subagent-transcript":true,"x":1}}"#,
        false,
        true,
    ),
    row(
        r#"{"_meta":{"x":1},"terminal":true}"#,
        r#"{"_meta":{"x":1,"subagent-transcript":true},"terminal":true}"#,
        false,
        true,
    ),
    // Values that cannot hold the capability stay as they are.
    row(r#"{"_meta":5}"#, r#"{"_meta":5}"#, false, false),
    row(
        r#"{"_meta":"m","fs":{}}"#,
        r#"{"_meta":"m","fs":{}}"#,
        false,
        false,
    ),
    row(r#""caps""#, r#""caps""#, false, false),
];

#[test]
fn client_capabilities_reach_the_agent_byte_for_byte_as_pinned() {
    for r in DEFAULT_FLAGS {
        let mut e = engine();
        let out = e.on_client(&wire(r.caps).into_bytes(), Instant::now());
        assert_eq!(sent_bytes(&out), wire(r.sent), "caps {}", r.caps);
        assert_eq!(e.aside, r.aside, "aside for {}", r.caps);
        assert_eq!(e.forwards_text, r.text, "forwards_text for {}", r.caps);
    }
}

#[test]
fn an_unchanged_initialize_is_forwarded_as_the_client_wrote_it() {
    // Whitespace and key order survive only when the bytes are forwarded, not re-serialized.
    let raw = br#"{ "id": 0, "jsonrpc": "2.0", "method": "initialize", "params": { "clientCapabilities": { "fs": { "readTextFile": true }, "subagents": {} }, "protocolVersion": 1 } }"#.to_vec();
    let out = engine().on_client(&raw, Instant::now());
    assert_eq!(
        out.iter()
            .filter(|a| matches!(a, Action::ToAgent(_)))
            .count(),
        1
    );
    assert!(out.contains(&Action::ToAgent(raw)));
}

#[test]
fn flags_decide_whether_the_transcript_capability_is_added() {
    let caps = r#"{"fs":{"readTextFile":true},"terminal":true}"#;
    let with_sub = r#"{"fs":{"readTextFile":true},"terminal":true,"subagents":{}}"#;
    let injected =
        r#"{"fs":{"readTextFile":true},"terminal":true,"_meta":{"subagent-transcript":true}}"#;
    let cases = [
        (
            Config {
                subagent_text: false,
                ..test_config()
            },
            caps,
            caps,
        ),
        (
            Config {
                fold: false,
                ..test_config()
            },
            caps,
            caps,
        ),
        (
            Config {
                force_features: true,
                ..test_config()
            },
            caps,
            injected,
        ),
        // Forced on beside native subagents the capability is still not added.
        (
            Config {
                force_features: true,
                ..test_config()
            },
            with_sub,
            with_sub,
        ),
        (
            Config {
                subagent_text: false,
                ..test_config()
            },
            with_sub,
            with_sub,
        ),
    ];
    for (cfg, input, want) in cases {
        let mut e = Engine::new(cfg);
        let out = e.on_client(&wire(input).into_bytes(), Instant::now());
        assert_eq!(sent_bytes(&out), wire(want), "caps {input}");
    }
}
