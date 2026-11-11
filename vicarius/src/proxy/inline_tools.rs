// SPDX-License-Identifier: MPL-2.0
//! Moves the inline tool definitions Claude Code puts in messages into the request's `tools`.
//!
//! The rewrite reads only the fields it changes; every other part of the body stays raw JSON text
//! and is written back in its original order.

use serde::Deserialize;
use serde::de::{MapAccess, Visitor};
use serde_json::Value;
use serde_json::value::RawValue;
use std::borrow::Cow;

const MARKER: &[u8] = b"tool_definition";
const TOOL_DEFINITION: &str = "tool_definition";
const TOOL_RESULT: &str = "tool_result";
/// The block of a mid-conversation `system` message that adds a tool under `tool`.
const TOOL_ADDITION: &str = "tool_addition";
/// The fields that may hold the tool object of a `tool_definition` block, in order.
const NESTED_KEYS: [&str; 2] = ["tool", "definition"];
const PLACEHOLDER: &str = "(tool loaded)";

/// The fields of an object in request order, each value kept as its raw JSON text.
struct Fields<'a>(Vec<(String, &'a RawValue)>);

impl<'a> Fields<'a> {
    fn insert(&mut self, key: String, value: &'a RawValue) {
        if let Some((_, current)) = self.0.iter_mut().find(|(name, _)| name == &key) {
            *current = value;
        } else {
            self.0.push((key, value));
        }
    }

    fn get(&self, key: &str) -> Option<&'a RawValue> {
        self.0
            .iter()
            .find(|(name, _)| name.as_str() == key)
            .map(|(_, value)| *value)
    }

    fn iter(&self) -> impl Iterator<Item = (&str, &'a RawValue)> {
        self.0.iter().map(|(key, value)| (key.as_str(), *value))
    }

    fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl<'de: 'a, 'a> Deserialize<'de> for Fields<'a> {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct FieldsVisitor;

        impl<'de> Visitor<'de> for FieldsVisitor {
            type Value = Fields<'de>;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("a JSON object")
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let mut fields = Fields(Vec::with_capacity(map.size_hint().unwrap_or_default()));
                while let Some((key, value)) = map.next_entry::<String, &'de RawValue>()? {
                    fields.insert(key, value);
                }
                Ok(fields)
            }
        }

        deserializer.deserialize_map(FieldsVisitor)
    }
}

/// The fields of a block that the rewrite reads. Every other field is ignored, and the raw block
/// is forwarded unchanged.
#[derive(Deserialize)]
struct Block<'a> {
    #[serde(rename = "type")]
    kind: Option<&'a str>,
    #[serde(borrow)]
    tool: Option<&'a RawValue>,
    #[serde(borrow)]
    content: Option<&'a RawValue>,
}

/// One entry of a rendered JSON array: request text, or text this rewrite built.
enum Part<'a> {
    Raw(&'a str),
    Owned(String),
}

impl Part<'_> {
    fn text(&self) -> &str {
        match self {
            Self::Raw(text) => text,
            Self::Owned(text) => text,
        }
    }
}

/// Returns the body with its inline tool definitions moved into `tools`.
///
/// The body comes back borrowed and unchanged when it has no `tool_definition` block or does not
/// parse as a JSON object. A block is removed even when a tool with its name is already in `tools`.
pub fn rewrite(body: &[u8]) -> Cow<'_, [u8]> {
    if !body.windows(MARKER.len()).any(|window| window == MARKER) {
        return Cow::Borrowed(body);
    }
    let Ok(root) = serde_json::from_slice::<Fields<'_>>(body) else {
        return Cow::Borrowed(body);
    };
    let Some(out) = rewrite_root(&root) else {
        return Cow::Borrowed(body);
    };
    Cow::Owned(out.into_bytes())
}

/// Moves every definition into `tools` and returns the new body, or `None` when there was none.
fn rewrite_root<'a>(root: &Fields<'a>) -> Option<String> {
    let tools: Option<Vec<&'a RawValue>> = match root.get("tools") {
        Some(raw) => Some(serde_json::from_str(raw.get()).ok()?),
        None => None,
    };
    let messages: Vec<&'a RawValue> =
        serde_json::from_str(raw_field(root, "messages")?.get()).ok()?;
    let mut found: Vec<Cow<'a, str>> = Vec::new();
    let mut rewritten = Vec::with_capacity(messages.len());
    let mut changed = false;
    for &message in &messages {
        match rewrite_message(message, &mut found) {
            Some(text) => {
                changed = true;
                rewritten.push(Part::Owned(text));
            }
            None => rewritten.push(Part::Raw(message.get())),
        }
    }
    if !changed {
        return None;
    }
    let mut names = Vec::new();
    let mut tools_text: Vec<Cow<'a, str>> = Vec::new();
    for &raw in tools.iter().flatten() {
        names.push(name_of(raw.get()));
        tools_text.push(Cow::Borrowed(raw.get()));
    }
    for tool in found {
        let name = name_of(tool.as_ref());
        if name
            .as_ref()
            .is_some_and(|name| names.iter().any(|seen| seen.as_ref() == Some(name)))
        {
            continue;
        }
        names.push(name);
        tools_text.push(tool);
    }
    let mut out = String::from("{");
    for (index, (key, raw)) in root.iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        push_key(&mut out, key);
        match key {
            "messages" => out.push_str(&render_list(rewritten.iter().map(Part::text))),
            "tools" => out.push_str(&render_list(tools_text.iter().map(|text| text.as_ref()))),
            _ => out.push_str(raw.get()),
        }
    }
    if tools.is_none() {
        if !root.is_empty() {
            out.push(',');
        }
        push_key(&mut out, "tools");
        out.push_str(&render_list(tools_text.iter().map(|text| text.as_ref())));
    }
    out.push('}');
    Some(out)
}

/// Rewrites the content of one message, or returns `None` when it holds no definition.
fn rewrite_message<'a>(message: &'a RawValue, found: &mut Vec<Cow<'a, str>>) -> Option<String> {
    let fields = object(message)?;
    let blocks: Vec<&'a RawValue> =
        serde_json::from_str(raw_field(&fields, "content")?.get()).ok()?;
    let (rewritten, changed) = rewrite_blocks(&blocks, found);
    changed.then(|| {
        let content = render_list(rewritten.iter().map(Part::text));
        render_with_content(&fields, &content)
    })
}

/// Removes the definition blocks from one content array, looking into `tool_result` blocks.
///
/// Reports whether it removed or changed a block. An array that the removal empties gets a text
/// placeholder, so the message stays valid.
fn rewrite_blocks<'a>(
    blocks: &[&'a RawValue],
    found: &mut Vec<Cow<'a, str>>,
) -> (Vec<Part<'a>>, bool) {
    let before = blocks.len();
    let mut out = Vec::with_capacity(before);
    let mut changed = false;
    for &raw in blocks {
        let Ok(block) = serde_json::from_str::<Block<'a>>(raw.get()) else {
            out.push(Part::Raw(raw.get()));
            continue;
        };
        match block.kind {
            Some(TOOL_DEFINITION) => match definition_of(raw) {
                Some(tool) => {
                    found.push(tool);
                    changed = true;
                }
                None => out.push(Part::Raw(raw.get())),
            },
            Some(TOOL_ADDITION) => match block.tool.and_then(inline_tool).and_then(definition_of) {
                Some(tool) => {
                    found.push(tool);
                    changed = true;
                }
                None => out.push(Part::Raw(raw.get())),
            },
            Some(TOOL_RESULT) => match nested_blocks(block.content) {
                Some(inner) => {
                    let (rendered, inner_changed) = rewrite_blocks(&inner, found);
                    if inner_changed {
                        changed = true;
                        let content = render_list(rendered.iter().map(Part::text));
                        match object(raw) {
                            Some(fields) => {
                                out.push(Part::Owned(render_with_content(&fields, &content)));
                            }
                            None => out.push(Part::Raw(raw.get())),
                        }
                    } else {
                        out.push(Part::Raw(raw.get()));
                    }
                }
                None => out.push(Part::Raw(raw.get())),
            },
            _ => out.push(Part::Raw(raw.get())),
        }
    }
    if before > 0 && out.is_empty() {
        out.push(Part::Owned(format!(
            r#"{{"type":"text","text":"{PLACEHOLDER}"}}"#
        )));
    }
    (out, changed)
}

/// Returns the raw block when a `tool_addition` carries a `tool_definition` under `tool`.
fn inline_tool<'a>(raw: &'a RawValue) -> Option<&'a RawValue> {
    let block: Block<'a> = serde_json::from_str(raw.get()).ok()?;
    (block.kind == Some(TOOL_DEFINITION)).then_some(raw)
}

/// Returns the content array of a `tool_result` block, or `None` when its content is not one.
fn nested_blocks(content: Option<&RawValue>) -> Option<Vec<&RawValue>> {
    serde_json::from_str(content?.get()).ok()
}

/// Returns the tool a `tool_definition` block carries, as raw JSON text.
///
/// A tool object under one of `NESTED_KEYS` is the tool. Otherwise the block's own fields are,
/// less the bookkeeping Claude Code adds to them.
fn definition_of<'a>(raw: &'a RawValue) -> Option<Cow<'a, str>> {
    let fields = object(raw)?;
    for key in NESTED_KEYS {
        let Some(value) = raw_field(&fields, key) else {
            continue;
        };
        if is_object(value) {
            return Some(Cow::Borrowed(value.get()));
        }
    }
    let mut out = String::from("{");
    let mut first = true;
    for (key, value) in fields.iter() {
        if is_bookkeeping(key) {
            continue;
        }
        if !first {
            out.push(',');
        }
        first = false;
        push_key(&mut out, key);
        out.push_str(value.get());
    }
    out.push('}');
    Some(Cow::Owned(out))
}

/// Parses `raw` as an object's fields in request order.
fn object<'a>(raw: &'a RawValue) -> Option<Fields<'a>> {
    serde_json::from_str(raw.get()).ok()
}

fn raw_field<'a>(fields: &Fields<'a>, key: &str) -> Option<&'a RawValue> {
    fields.get(key)
}

fn is_object(raw: &RawValue) -> bool {
    raw.get().trim_start().starts_with('{')
}

/// Renders an object's fields in their original order, with `content` replaced.
fn render_with_content(fields: &Fields<'_>, content: &str) -> String {
    let mut out = String::from("{");
    for (index, (key, value)) in fields.iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        push_key(&mut out, key);
        if key == "content" {
            out.push_str(content);
        } else {
            out.push_str(value.get());
        }
    }
    out.push('}');
    out
}

/// Renders already-serialized values as a JSON array.
fn render_list<'a>(items: impl IntoIterator<Item = &'a str>) -> String {
    let mut out = String::from("[");
    for (index, item) in items.into_iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        out.push_str(item);
    }
    out.push(']');
    out
}

fn push_key(out: &mut String, key: &str) {
    // A key is always a string, so serializing it cannot fail.
    if let Ok(quoted) = serde_json::to_string(key) {
        out.push_str(&quoted);
    }
    out.push(':');
}

/// Returns the `name` of a tool, or `None` when it has none or is not an object.
fn name_of(raw: &str) -> Option<Value> {
    #[derive(Deserialize)]
    struct Named {
        name: Option<Value>,
    }
    serde_json::from_str::<Named>(raw).ok()?.name
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
    fn a_tool_addition_in_a_system_message_moves_its_definition() {
        let body = json!({
            "tools": [],
            "messages": [
                {"role": "user", "content": "hi"},
                {"role": "system", "content": [
                    {"type": "text", "text": "tools changed"},
                    {"type": "tool_addition", "tool": {
                        "type": "tool_definition",
                        "definition": {"name": "load", "input_schema": {"type": "object"}}
                    }},
                    {"type": "tool_addition", "tool": {"type": "tool_reference", "tool_name": "x"}}
                ]}
            ]
        });
        let out = rewritten(&body);
        assert_eq!(
            out["tools"],
            json!([{"name": "load", "input_schema": {"type": "object"}}])
        );
        assert_eq!(
            out["messages"][1]["content"],
            json!([
                {"type": "text", "text": "tools changed"},
                {"type": "tool_addition", "tool": {"type": "tool_reference", "tool_name": "x"}}
            ])
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
