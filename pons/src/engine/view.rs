// SPDX-License-Identifier: MPL-2.0
//! Borrowed view of an agent response, read once and used for inspection only.

use serde_json::Value;

/// Holds what the engine decides on in a response. A field of another type than expected
/// reads as absent. The response is forwarded as the bytes that came in, never from here.
pub(super) struct ResponseView<'a> {
    pub id: Option<&'a Value>,
    pub is_response: bool,
    pub cancelled: bool,
    pub steering_supported: bool,
    pub outcome: Option<&'a str>,
}

impl<'a> ResponseView<'a> {
    pub(super) fn new(msg: &'a Value) -> Self {
        let result = msg.get("result");
        Self {
            id: msg.get("id"),
            is_response: result.is_some() || msg.get("error").is_some(),
            cancelled: result
                .and_then(|r| r.get("stopReason"))
                .and_then(Value::as_str)
                == Some("cancelled"),
            steering_supported: result
                .and_then(|r| r.get("_meta")?.get("steering")?.get("supported"))
                == Some(&Value::Bool(true)),
            outcome: result
                .and_then(|r| r.get("outcome"))
                .and_then(Value::as_str),
        }
    }

    /// Returns the id when the message is a response.
    pub(super) fn response_id(&self) -> Option<&'a Value> {
        self.id.filter(|_| self.is_response)
    }
}

/// Returns the tool call a permission request is about.
pub(super) fn permission_tool_call_id(params: &Value) -> Option<&str> {
    params.get("toolCall")?.get("toolCallId")?.as_str()
}
