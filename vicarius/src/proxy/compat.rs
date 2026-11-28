// SPDX-License-Identifier: MPL-2.0
//! Applies a provider's request rewrites: the inline tools and the fields its upstream rejects.

use super::inline_tools;
use crate::config::Compat;
use serde_json::Value;
use std::borrow::Cow;

const CACHE_CONTROL: &[u8] = b"cache_control";
const THINKING: &[u8] = b"thinking";

/// Returns the body with every rewrite of `compat` applied.
///
/// The body comes back borrowed and unchanged when no enabled rewrite finds its marker, so a
/// request that needs none keeps its exact bytes.
pub fn apply<'a>(body: &'a [u8], compat: &Compat) -> Cow<'a, [u8]> {
    let mut current = Cow::Borrowed(body);
    if compat.inline_tools
        && let Cow::Owned(rewritten) = inline_tools::rewrite(&current)
    {
        current = Cow::Owned(rewritten);
    }
    if !compat.strip_cache_control && !compat.strip_thinking {
        return current;
    }
    let cache_control = compat.strip_cache_control && contains(&current, CACHE_CONTROL);
    let thinking = compat.strip_thinking && contains(&current, THINKING);
    if !cache_control && !thinking {
        return current;
    }
    let Ok(mut value) = serde_json::from_slice::<Value>(&current) else {
        return current;
    };
    let cache_removed = cache_control && remove_key(&mut value, "cache_control");
    let thinking_removed = thinking && remove_thinking(&mut value);
    if !cache_removed && !thinking_removed {
        return current;
    }
    let Ok(bytes) = serde_json::to_vec(&value) else {
        return current;
    };
    Cow::Owned(bytes)
}

fn contains(body: &[u8], marker: &[u8]) -> bool {
    body.windows(marker.len()).any(|window| window == marker)
}

/// Removes every object member named `key`, at any depth.
fn remove_key(value: &mut Value, key: &str) -> bool {
    match value {
        Value::Object(map) => {
            let mut changed = map.remove(key).is_some();
            for child in map.values_mut() {
                changed |= remove_key(child, key);
            }
            changed
        }
        Value::Array(items) => {
            let mut changed = false;
            for item in items {
                changed |= remove_key(item, key);
            }
            changed
        }
        _ => false,
    }
}

/// Removes `thinking` members and thinking content blocks, at any depth.
fn remove_thinking(value: &mut Value) -> bool {
    match value {
        Value::Object(map) => {
            let mut changed = map.remove("thinking").is_some();
            for child in map.values_mut() {
                changed |= remove_thinking(child);
            }
            changed
        }
        Value::Array(items) => {
            let before = items.len();
            items.retain(|item| !is_thinking_block(item));
            let mut changed = items.len() != before;
            for item in items {
                changed |= remove_thinking(item);
            }
            changed
        }
        _ => false,
    }
}

fn is_thinking_block(value: &Value) -> bool {
    matches!(
        value.get("type").and_then(Value::as_str),
        Some("thinking" | "redacted_thinking")
    )
}
