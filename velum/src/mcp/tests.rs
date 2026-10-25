// SPDX-License-Identifier: MPL-2.0
use super::*;
use crate::scratch::Scratch;
use std::os::unix::net::UnixListener;

fn talk(messages: &[Value], socket: Option<&Path>) -> Vec<Value> {
    let input: String = messages.iter().map(|m| format!("{m}\n")).collect();
    let mut output = Vec::new();
    serve(input.as_bytes(), &mut output, socket).unwrap();
    String::from_utf8(output)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

fn request(id: u32, method: &str, params: &Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params})
}

/// Answers each connection with `reply` and sends the request line it got down `seen`.
fn fake_broker(
    socket: &Path,
    reply: Value,
) -> (
    std::thread::JoinHandle<()>,
    std::sync::mpsc::Receiver<String>,
) {
    let listener = UnixListener::bind(socket).unwrap();
    let (sender, seen) = std::sync::mpsc::channel();
    let thread = std::thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let mut line = String::new();
        BufReader::new(&stream).read_line(&mut line).unwrap();
        sender.send(line).unwrap();
        writeln!(&stream, "{reply}").unwrap();
    });
    (thread, seen)
}

#[test]
fn initialize_and_tools_list_work_without_a_broker() {
    let answers = talk(
        &[
            request(1, "initialize", &json!({"protocolVersion": "2025-03-26"})),
            json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
            request(2, "tools/list", &json!({})),
        ],
        None,
    );
    assert_eq!(answers.len(), 2);
    assert_eq!(answers[0]["result"]["protocolVersion"], "2025-03-26");
    assert_eq!(answers[0]["result"]["serverInfo"]["name"], "velum");
    let names: Vec<&str> = answers[1]["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| tool["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["git_push", "gh", "open_url", "run_outside"]);
}

#[test]
fn an_unknown_protocol_version_gets_the_newest_one() {
    let answers = talk(
        &[request(
            1,
            "initialize",
            &json!({"protocolVersion": "1999-01-01"}),
        )],
        None,
    );
    assert_eq!(answers[0]["result"]["protocolVersion"], PROTOCOLS[0]);
}

#[test]
fn a_call_without_a_broker_says_it_is_not_running_under_velum() {
    let params = json!({"name": "run_outside", "arguments": {"command": "id"}});
    let answers = talk(&[request(3, "tools/call", &params)], None);
    let result = &answers[0]["result"];
    assert_eq!(result["isError"], true);
    assert!(
        result["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("not running under velum")
    );
}

#[test]
fn unknown_tools_and_methods_are_protocol_errors() {
    let answers = talk(
        &[
            request(1, "tools/call", &json!({"name": "rm_rf"})),
            request(2, "resources/list", &json!({})),
        ],
        None,
    );
    assert_eq!(answers[0]["error"]["code"], -32602);
    assert_eq!(answers[1]["error"]["code"], -32601);
}

#[test]
fn garbage_gets_a_parse_error_and_the_server_keeps_going() {
    let mut output = Vec::new();
    serve(&b"{nope\n"[..], &mut output, None).unwrap();
    let answer: Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(answer["error"]["code"], -32700);
    assert_eq!(answer["id"], Value::Null);
}

#[test]
fn a_call_is_forwarded_to_the_broker_and_its_answer_is_returned() {
    let scratch = Scratch::new("mcp-forward");
    let socket = scratch.path().join("b.sock");
    let (thread, seen) = fake_broker(
        &socket,
        json!({"status": "done", "exit_code": 0, "output": "pushed\n", "message": ""}),
    );
    let params = json!({"name": "git_push", "arguments": {"remote": "origin"}});
    let answers = talk(&[request(4, "tools/call", &params)], Some(&socket));
    thread.join().unwrap();
    let sent: Value = serde_json::from_str(&seen.recv().unwrap()).unwrap();
    assert_eq!(
        sent,
        json!({"tool": "git_push", "args": {"remote": "origin"}})
    );
    let result = &answers[0]["result"];
    assert_eq!(result["isError"], false);
    assert_eq!(result["content"][0]["text"], "exit code: 0\npushed\n");
}

#[test]
fn a_denial_and_a_failing_command_are_tool_errors() {
    let scratch = Scratch::new("mcp-denied");
    let socket = scratch.path().join("b.sock");
    let (thread, _seen) = fake_broker(&socket, json!({"status": "denied", "message": "no"}));
    let params = json!({"name": "open_url", "arguments": {"url": "https://a.b"}});
    let answers = talk(&[request(5, "tools/call", &params)], Some(&socket));
    thread.join().unwrap();
    assert_eq!(answers[0]["result"]["isError"], true);
    assert!(
        answers[0]["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .starts_with("denied")
    );

    let failed = Response::done(2, "boom".to_string());
    assert_eq!(describe(&failed)["isError"], true);
}

#[test]
fn a_dead_broker_socket_is_a_tool_error_not_a_crash() {
    let scratch = Scratch::new("mcp-dead");
    let socket = scratch.path().join("gone.sock");
    let params = json!({"name": "gh", "arguments": {"args": ["pr", "list"]}});
    let answers = talk(&[request(6, "tools/call", &params)], Some(&socket));
    assert_eq!(answers[0]["result"]["isError"], true);
    assert!(
        answers[0]["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("did not answer")
    );
}
