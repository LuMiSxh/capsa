// SPDX-License-Identifier: MPL-2.0
#![expect(clippy::unwrap_used, reason = "test code")]
#![expect(clippy::print_stdout, reason = "the server answers on stdout")]
// A stdio MCP server for the `acus script` tests. `hang` never answers; with `FAKE_PID_FILE` set,
// the server writes its pid there when it starts.

use serde_json::{Value, json};
use std::io::BufRead;

fn tool_result(name: &str, args: &Value) -> Option<Value> {
    match name {
        "echo" => Some(json!({"content": [{"type": "text",
            "text": format!("echo: {}", args["text"].as_str().unwrap_or_default())}]})),
        "fail" => Some(json!({"isError": true, "content": [{"type": "text", "text": "boom"}]})),
        "structured" => Some(json!({"content": [{"type": "text", "text": "ignored"}],
            "structuredContent": {"n": 1}})),
        "image" => Some(json!({"content": [{"type": "image", "data": "AA==",
            "mimeType": "image/png"}]})),
        _ => None,
    }
}

fn main() {
    if let Some(path) = std::env::var_os("FAKE_PID_FILE") {
        std::fs::write(path, std::process::id().to_string()).unwrap();
    }
    for line in std::io::stdin().lock().lines() {
        let message: Value = serde_json::from_str(&line.unwrap()).unwrap();
        let Some(id) = message.get("id") else {
            continue;
        };
        let result = match message["method"].as_str().unwrap() {
            "initialize" => json!({"protocolVersion": "2025-06-18", "capabilities": {"tools": {}},
                "serverInfo": {"name": "fake", "version": "0"}}),
            "tools/list" => json!({"tools": [
                {"name": "echo", "description": "Echoes text"},
                {"name": "fail", "description": "Always fails"},
                {"name": "structured", "description": "Returns structured content"},
                {"name": "image", "description": "Returns an image"},
                {"name": "hang", "description": "Never answers"},
            ]}),
            "tools/call" => {
                let params = &message["params"];
                let name = params["name"].as_str().unwrap();
                if name == "hang" {
                    continue;
                }
                tool_result(name, &params["arguments"]).unwrap()
            }
            _ => continue,
        };
        println!("{}", json!({"jsonrpc": "2.0", "id": id, "result": result}));
    }
}
