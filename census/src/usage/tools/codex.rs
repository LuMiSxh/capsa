// SPDX-License-Identifier: MPL-2.0
//! Decodes the shell commands of one Codex tool call.

use serde_json::Value;

/// The shell commands of one Codex tool call. Arguments are JSON, often a JSON string that holds
/// the JSON object again, so string literals are decoded until none is left. The `cmd` or
/// `command` value is a string or an argv array; other input (a JS tool body) is scanned for
/// `cmd` literals.
pub(crate) fn codex_commands(input: &str) -> Vec<String> {
    let mut args = input.to_owned();
    while let Ok(s) = serde_json::from_str::<String>(&args) {
        args = s;
    }
    let Ok(v) = serde_json::from_str::<Value>(&args) else {
        return js_cmds(&args);
    };
    match v.get("cmd").or_else(|| v.get("command")) {
        Some(Value::String(s)) => vec![s.clone()],
        Some(Value::Array(a)) => vec![argv_command(a)],
        _ => js_cmds(&args),
    }
}

/// The script of an argv such as `["bash", "-lc", "cargo test"]`, or else the words joined.
fn argv_command(argv: &[Value]) -> String {
    let words: Vec<&str> = argv.iter().filter_map(Value::as_str).collect();
    let script = words
        .iter()
        .position(|w| matches!(*w, "-c" | "-lc"))
        .and_then(|i| words.get(i + 1));
    match script {
        Some(s) => (*s).to_owned(),
        None => words.join(" "),
    }
}

/// Extracts every `cmd` literal from a Codex JS shell call, such as
/// `tools.exec_command({cmd: "..."})`.
// ponytail: scans for `cmd: <string literal>`; computed commands are not seen.
fn js_cmds(js: &str) -> Vec<String> {
    let mut out = vec![];
    let mut rest = js;
    while let Some(i) = rest.find("cmd") {
        rest = &rest[i + 3..];
        let t = rest.trim_start_matches(['"', '\'', ' ']);
        let Some(t) = t.strip_prefix(':') else {
            continue;
        };
        let mut cs = t.trim_start().chars();
        let Some(q @ ('"' | '\'' | '`')) = cs.next() else {
            continue;
        };
        let mut s = String::new();
        while let Some(c) = cs.next() {
            match c {
                c if c == q => break,
                '\\' => match cs.next() {
                    Some('n') => s.push('\n'),
                    Some(c) => s.push(c),
                    None => break,
                },
                c => s.push(c),
            }
        }
        out.push(s);
    }
    out
}
