// SPDX-License-Identifier: MPL-2.0
//! Moves the inline tool definitions Claude Code puts in messages into the request's `tools`.

use serde_json::{Map, Value, json};
use std::borrow::Cow;

const MARKER: &[u8] = b"tool_definition";
const TOOL_DEFINITION: &str = "tool_definition";
const TOOL_RESULT: &str = "tool_result";
/// The fields that may hold the tool object of a `tool_definition` block, in order.
const NESTED_KEYS: [&str; 2] = ["tool", "definition"];
const PLACEHOLDER: &str = "(tool loaded)";

/// Returns the body with its inline tool definitions moved into `tools`.
///
/// The body comes back borrowed and unchanged when it has no `tool_definition` block or does not
/// parse as JSON. A block is removed even when a tool with its name is already in `tools`.
pub fn rewrite(body: &[u8]) -> Cow<'_, [u8]> {
    if !body.windows(MARKER.len()).any(|window| window == MARKER) {
        return Cow::Borrowed(body);
    }
    let Ok(mut value) = serde_json::from_slice::<Value>(body) else {
        return Cow::Borrowed(body);
    };
    if !move_definitions(&mut value) {
        return Cow::Borrowed(body);
    }
    serde_json::to_vec(&value).map_or(Cow::Borrowed(body), Cow::Owned)
}

/// Moves every definition into `tools` and reports whether the body had any.
fn move_definitions(value: &mut Value) -> bool {
    let Some(root) = value.as_object_mut() else {
        return false;
    };
    if root.get("tools").is_some_and(|tools| !tools.is_array()) {
        return false;
    }
    let Some(Value::Array(messages)) = root.get_mut("messages") else {
        return false;
    };
    let mut found = Vec::new();
    let mut changed = false;
    for message in messages.iter_mut() {
        if let Some(Value::Array(blocks)) = message.get_mut("content") {
            changed |= take_definitions(blocks, &mut found);
        }
    }
    if !changed {
        return false;
    }
    let Some(tools) = root
        .entry("tools")
        .or_insert_with(|| json!([]))
        .as_array_mut()
    else {
        return false;
    };
    for tool in found {
        let duplicate = tool
            .get("name")
            .is_some_and(|name| tools.iter().any(|t| t.get("name") == Some(name)));
        if !duplicate {
            tools.push(tool);
        }
    }
    true
}

/// Removes the definition blocks from one content array, looking into `tool_result` blocks.
///
/// Reports whether it removed or changed a block. An array that the removal empties gets a text
/// placeholder, so the message stays valid.
fn take_definitions(blocks: &mut Vec<Value>, found: &mut Vec<Value>) -> bool {
    let before = blocks.len();
    let mut changed = false;
    blocks.retain_mut(|block| {
        let Some(fields) = block.as_object_mut() else {
            return true;
        };
        if fields.get("type").and_then(Value::as_str) == Some(TOOL_DEFINITION) {
            found.push(definition(fields));
            changed = true;
            return false;
        }
        if fields.get("type").and_then(Value::as_str) == Some(TOOL_RESULT)
            && let Some(Value::Array(inner)) = fields.get_mut("content")
        {
            changed |= take_definitions(inner, found);
        }
        true
    });
    if before > 0 && blocks.is_empty() {
        blocks.push(json!({"type": "text", "text": PLACEHOLDER}));
    }
    changed
}

/// Returns the tool a `tool_definition` block carries.
///
/// A tool object under one of `NESTED_KEYS` is the tool. Otherwise the block's own fields are,
/// less the bookkeeping Claude Code adds to them.
fn definition(block: &mut Map<String, Value>) -> Value {
    for key in NESTED_KEYS {
        if block.get(key).is_some_and(Value::is_object)
            && let Some(tool) = block.remove(key)
        {
            return tool;
        }
    }
    Value::Object(
        std::mem::take(block)
            .into_iter()
            .filter(|(key, _)| !is_bookkeeping(key))
            .collect(),
    )
}

/// Names the fields Claude Code keeps on a block for itself, not on the tool.
fn is_bookkeeping(key: &str) -> bool {
    matches!(key, "type" | "cache_control" | "id") || key.ends_with("_id")
}

#[cfg(test)]
mod tests {
    use super::rewrite;
    use serde_json::{Value, json};
    use std::borrow::Cow;

    fn rewritten(body: &Value) -> Value {
        let bytes = serde_json::to_vec(body).unwrap();
        serde_json::from_slice(&rewrite(&bytes)).unwrap()
    }

    #[test]
    fn a_flat_block_moves_its_tool_fields_without_bookkeeping() {
        let out = rewritten(&json!({
            "model": "deepseek-flash",
            "tools": [{"name": "Bash"}],
            "messages": [{"role": "user", "content": [{
                "type": "tool_definition",
                "name": "Lookup",
                "description": "Finds a record",
                "input_schema": {"type": "object"},
                "cache_control": {"type": "ephemeral"},
                "id": "def_1"
            }]}]
        }));
        assert_eq!(
            out["tools"],
            json!([
                {"name": "Bash"},
                {"name": "Lookup", "description": "Finds a record", "input_schema": {"type": "object"}}
            ])
        );
    }

    #[test]
    fn a_nested_tool_or_definition_object_is_taken_whole() {
        let out = rewritten(&json!({
            "messages": [{"role": "user", "content": [
                {
                    "type": "tool_definition",
                    "tool": {"name": "Lookup", "input_schema": {"type": "object"}},
                    "cache_control": {"type": "ephemeral"}
                },
                {"type": "tool_definition", "definition": {"name": "Fetch", "input_schema": {}}}
            ]}]
        }));
        assert_eq!(
            out["tools"],
            json!([
                {"name": "Lookup", "input_schema": {"type": "object"}},
                {"name": "Fetch", "input_schema": {}}
            ])
        );
    }

    #[test]
    fn a_block_inside_a_tool_result_moves_and_the_result_keeps_its_text() {
        let out = rewritten(&json!({
            "messages": [{"role": "user", "content": [{
                "type": "tool_result",
                "tool_use_id": "toolu_1",
                "content": [
                    {"type": "tool_definition", "name": "Lookup", "input_schema": {}},
                    {"type": "text", "text": "ok"}
                ]
            }]}]
        }));
        assert_eq!(
            out["messages"][0]["content"][0]["content"],
            json!([{"type": "text", "text": "ok"}])
        );
        assert_eq!(out["tools"][0]["name"], "Lookup");
    }

    #[test]
    fn an_emptied_tool_result_content_gets_a_placeholder() {
        let out = rewritten(&json!({
            "messages": [{"role": "user", "content": [{
                "type": "tool_result",
                "tool_use_id": "toolu_1",
                "content": [{"type": "tool_definition", "name": "Lookup", "input_schema": {}}]
            }]}]
        }));
        assert_eq!(out["messages"][0]["content"][0]["tool_use_id"], "toolu_1");
        assert_eq!(
            out["messages"][0]["content"][0]["content"],
            json!([{"type": "text", "text": "(tool loaded)"}])
        );
    }

    #[test]
    fn an_emptied_content_array_gets_a_placeholder() {
        let out = rewritten(&json!({
            "messages": [{"role": "user", "content": [
                {"type": "tool_definition", "name": "Lookup", "input_schema": {}}
            ]}]
        }));
        assert_eq!(
            out["messages"][0]["content"],
            json!([{"type": "text", "text": "(tool loaded)"}])
        );
    }

    #[test]
    fn a_tool_already_in_tools_is_not_added_again() {
        let out = rewritten(&json!({
            "tools": [{"name": "Lookup", "description": "kept"}],
            "messages": [
                {"role": "user", "content": [
                    {"type": "tool_definition", "name": "Lookup", "description": "again"}
                ]},
                {"role": "user", "content": [
                    {"type": "tool_definition", "name": "Fetch"},
                    {"type": "text", "text": "hi"}
                ]},
                {"role": "user", "content": [{"type": "tool_definition", "name": "Fetch"}]}
            ]
        }));
        assert_eq!(
            out["tools"],
            json!([{"name": "Lookup", "description": "kept"}, {"name": "Fetch"}])
        );
    }

    #[test]
    fn tools_is_created_when_the_body_has_none() {
        let out = rewritten(&json!({
            "messages": [{"role": "user", "content": [
                {"type": "text", "text": "x"},
                {"type": "tool_definition", "name": "Lookup", "input_schema": {}}
            ]}]
        }));
        assert_eq!(
            out["tools"],
            json!([{"name": "Lookup", "input_schema": {}}])
        );
        assert_eq!(
            out["messages"][0]["content"],
            json!([{"type": "text", "text": "x"}])
        );
    }

    #[test]
    fn a_body_without_a_block_is_returned_borrowed() {
        let body: &[u8] = br#"{"model":"deepseek-flash","messages":[{"role":"user","content":"tool_definition is a word"}]}"#;
        assert!(matches!(rewrite(body), Cow::Borrowed(bytes) if std::ptr::eq(bytes, body)));
    }

    #[test]
    fn a_body_that_does_not_parse_is_returned_borrowed() {
        let body: &[u8] = br#"{"model":"deepseek-flash","messages":[{"role":"user","content":[{"type":"tool_definition""#;
        assert!(matches!(rewrite(body), Cow::Borrowed(bytes) if std::ptr::eq(bytes, body)));
    }

    #[test]
    fn a_tools_value_that_is_not_an_array_leaves_the_body_unchanged() {
        let body: &[u8] = br#"{"tools":{"name":"Bash"},"messages":[{"role":"user","content":[{"type":"tool_definition","name":"Lookup"}]}]}"#;
        assert!(matches!(rewrite(body), Cow::Borrowed(bytes) if std::ptr::eq(bytes, body)));
    }
}
