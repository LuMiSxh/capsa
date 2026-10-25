// SPDX-License-Identifier: MPL-2.0

use super::*;

fn velum() -> McpServer {
    McpServer {
        name: "velum".to_owned(),
        command: "/bin/velum".to_owned(),
        args: vec!["mcp".to_owned()],
    }
}

fn velum_entry() -> Value {
    json!({"name": "velum", "command": "/bin/velum", "args": ["mcp"], "env": []})
}

fn with_servers(mcp_servers: Vec<McpServer>) -> Engine {
    Engine::new(Config {
        mcp_servers,
        ..test_config()
    })
}

fn request(method: &str, params: Value) -> Vec<u8> {
    line(json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}))
}

#[test]
fn session_new_gets_the_server_in_a_new_mcp_servers_list() {
    let t0 = Instant::now();
    let raw = request("session/new", json!({"cwd": "/work"}));
    let out = with_servers(vec![velum()]).on_client(&raw, t0);
    assert_eq!(
        to_agent(&out)["params"],
        json!({"cwd": "/work", "mcpServers": [velum_entry()]})
    );
    assert_eq!(notes(&out).len(), 1);
    assert!(notes(&out)[0].contains("added MCP server velum"));
}

#[test]
fn the_server_is_appended_to_the_servers_the_client_sends() {
    let t0 = Instant::now();
    let raw = request(
        "session/new",
        json!({"cwd": "/work", "mcpServers": [{"name": "other", "command": "o", "args": [], "env": []}]}),
    );
    let out = with_servers(vec![velum()]).on_client(&raw, t0);
    assert_eq!(
        to_agent(&out)["params"]["mcpServers"],
        json!([{"name": "other", "command": "o", "args": [], "env": []}, velum_entry()])
    );
}

#[test]
fn a_name_the_client_already_sends_keeps_the_clients_entry() {
    let t0 = Instant::now();
    let raw = request(
        "session/new",
        json!({"mcpServers": [{"name": "velum", "command": "client-velum", "args": [], "env": []}]}),
    );
    let out = with_servers(vec![velum()]).on_client(&raw, t0);
    assert_eq!(
        to_agent(&out)["params"]["mcpServers"],
        json!([{"name": "velum", "command": "client-velum", "args": [], "env": []}])
    );
    assert_eq!(notes(&out).len(), 1);
    assert!(notes(&out)[0].contains("kept its entry"));
}

#[test]
fn each_server_added_or_skipped_gets_its_own_note() {
    let t0 = Instant::now();
    let lint = McpServer {
        name: "lint".to_owned(),
        command: "lint-mcp".to_owned(),
        args: Vec::new(),
    };
    let raw = request(
        "session/new",
        json!({"mcpServers": [{"name": "lint", "command": "client", "args": [], "env": []}]}),
    );
    let out = with_servers(vec![velum(), lint]).on_client(&raw, t0);
    assert_eq!(notes(&out).len(), 2);
    assert!(notes(&out)[0].contains("added MCP server velum"));
    assert!(notes(&out)[1].contains("kept its entry"));
    assert_eq!(
        to_agent(&out)["params"]["mcpServers"],
        json!([{"name": "lint", "command": "client", "args": [], "env": []}, velum_entry()])
    );
}

#[test]
fn session_load_and_resume_get_the_servers_too() {
    let t0 = Instant::now();
    for method in ["session/load", "session/resume"] {
        let raw = request(method, json!({"sessionId": "s", "cwd": "/work"}));
        let out = with_servers(vec![velum()]).on_client(&raw, t0);
        assert_eq!(
            to_agent(&out)["params"]["mcpServers"],
            json!([velum_entry()]),
            "{method}"
        );
        assert_eq!(to_agent(&out)["params"]["sessionId"], "s", "{method}");
    }
}

#[test]
fn other_requests_are_forwarded_byte_for_byte() {
    let t0 = Instant::now();
    let mut e = with_servers(vec![velum()]);
    for method in ["session/prompt", "session/list", "authenticate"] {
        let raw = request(method, json!({"sessionId": "s", "mcpServers": []}));
        assert_eq!(
            e.on_client(&raw, t0),
            vec![Action::ToAgent(raw.clone())],
            "{method}"
        );
    }
}

#[test]
fn without_configured_servers_session_requests_pass_unchanged() {
    let t0 = Instant::now();
    let raw = request("session/new", json!({"cwd": "/work"}));
    assert_eq!(
        engine().on_client(&raw, t0),
        vec![Action::ToAgent(raw.clone())]
    );
}

#[test]
fn servers_are_added_when_pons_steps_aside_for_native_subagents() {
    let t0 = Instant::now();
    let mut e = with_servers(vec![velum()]);
    let out = e.on_client(&initialize_with(json!({"subagents": {}})), t0);
    assert!(notes(&out)[0].contains("stepping aside"));
    let out = e.on_client(&request("session/new", json!({"cwd": "/work"})), t0);
    assert_eq!(
        to_agent(&out)["params"]["mcpServers"],
        json!([velum_entry()])
    );
}
