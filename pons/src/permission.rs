// SPDX-License-Identifier: MPL-2.0
//! Opt-in answers to the agent's `session/request_permission` requests.
//! The policy is pure: it decides from the request and never does I/O.

use serde_json::Value;

/// Tools that are never answered by pons, whatever the policy: they ask the
/// user a question or a decision, and an automatic "allow" would answer it.
const NEVER: [&str; 2] = ["exitplanmode", "askuserquestion"];

/// Tool kinds approved when `--auto-approve` is set without a tool list.
const READ_ONLY_KINDS: [&str; 4] = ["read", "search", "think", "fetch"];

/// What `--auto-approve` does with a matching request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, clap::ValueEnum)]
pub enum AutoApprove {
    /// Forward every request to the client.
    #[default]
    None,
    /// Pick the `allow_once` option.
    AllowOnce,
    /// Pick the `allow_always` option (`allow_once` if there is none).
    AllowAlways,
}

/// The policy: a mode and the tool names it covers.
#[derive(Debug, Clone, Default)]
pub struct Policy {
    mode: AutoApprove,
    /// Lowercase prefixes of a tool title, kind or name. Empty: read-only
    /// kinds only.
    tools: Vec<String>,
}

/// What to do with one request.
#[derive(Debug, PartialEq, Eq)]
pub enum Decision {
    /// Forward it to the client untouched.
    Forward(String),
    /// Answer it with this option.
    Answer { option_id: String, why: String },
}

/// Title, kind and name pons already knows from the tool call's own updates.
#[derive(Debug, Default, Clone, Copy)]
pub struct Known<'a> {
    pub title: &'a str,
    pub kind: &'a str,
    pub name: &'a str,
}

impl Policy {
    pub fn new(mode: AutoApprove, list: &str) -> Self {
        let tools = list
            .split(',')
            .map(|t| t.trim().to_ascii_lowercase())
            .filter(|t| !t.is_empty())
            .collect();
        Self { mode, tools }
    }

    pub fn is_on(&self) -> bool {
        self.mode != AutoApprove::None
    }

    /// Decides on the `params` of a `session/request_permission` request.
    pub fn decide(&self, params: &Value, known: Known<'_>) -> Decision {
        if !self.is_on() {
            return Decision::Forward("--auto-approve is none".into());
        }
        let call = params.get("toolCall").unwrap_or(&Value::Null);
        let text = |v: Option<&Value>, fallback: &str| {
            v.and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .unwrap_or(fallback)
                .to_ascii_lowercase()
        };
        let title = text(call.get("title"), known.title);
        let kind = text(call.get("kind"), known.kind);
        let name = text(call.get("name"), known.name);
        let meta_name = text(call.pointer("/_meta/claudeCode/toolName"), "");
        let names = [&title, &kind, &name, &meta_name];
        if names.iter().any(|n| NEVER.contains(&n.as_str())) || kind == "switch_mode" {
            return Decision::Forward("a plan or question request is never auto-answered".into());
        }
        let shown = if title.is_empty() { &name } else { &title };
        let why = if self.tools.is_empty() {
            if !READ_ONLY_KINDS.contains(&kind.as_str()) {
                return Decision::Forward(format!(
                    "kind `{kind}` of `{shown}` is not read-only and --auto-approve-tools is empty"
                ));
            }
            format!("read-only kind `{kind}` ({shown})")
        } else {
            let hit = self.tools.iter().find(|t| {
                names
                    .iter()
                    .any(|n| !n.is_empty() && n.starts_with(t.as_str()))
            });
            match hit {
                Some(t) => format!("`{shown}` matches --auto-approve-tools entry `{t}`"),
                None => {
                    return Decision::Forward(format!("`{shown}` is not in --auto-approve-tools"));
                }
            }
        };
        let wanted: &[&str] = match self.mode {
            AutoApprove::AllowAlways => &["allow_always", "allow_once"],
            _ => &["allow_once"],
        };
        let options = params.get("options").and_then(Value::as_array);
        for want in wanted {
            let found = options.and_then(|o| {
                o.iter().find_map(|o| {
                    (o.get("kind").and_then(Value::as_str) == Some(want))
                        .then(|| o.get("optionId").and_then(Value::as_str))
                        .flatten()
                })
            });
            if let Some(id) = found {
                return Decision::Answer {
                    option_id: id.to_owned(),
                    why: format!("{why}; chose {want}"),
                };
            }
        }
        Decision::Forward(format!("no {} option in the request", wanted[0]))
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn request(title: &str, kind: &str) -> Value {
        json!({"sessionId": "s", "toolCall": {"toolCallId": "t1", "title": title, "kind": kind},
            "options": [
                {"optionId": "always", "name": "Always", "kind": "allow_always"},
                {"optionId": "once", "name": "Allow", "kind": "allow_once"},
                {"optionId": "no", "name": "Reject", "kind": "reject_once"}]})
    }

    fn answer(d: Decision) -> String {
        match d {
            Decision::Answer { option_id, .. } => option_id,
            Decision::Forward(why) => format!("forward: {why}"),
        }
    }

    #[test]
    fn off_forwards_everything() {
        let p = Policy::new(AutoApprove::None, "Read");
        assert!(matches!(
            p.decide(&request("Read a", "read"), Known::default()),
            Decision::Forward(_)
        ));
    }

    #[test]
    fn an_empty_list_covers_only_read_only_kinds() {
        let p = Policy::new(AutoApprove::AllowOnce, "");
        for kind in ["read", "search", "think", "fetch"] {
            assert_eq!(
                answer(p.decide(&request("x", kind), Known::default())),
                "once"
            );
        }
        for kind in ["edit", "execute", "delete", "move", "other", ""] {
            assert!(
                answer(p.decide(&request("x", kind), Known::default())).starts_with("forward"),
                "{kind}"
            );
        }
        // Whitespace and empty entries are no list.
        let p = Policy::new(AutoApprove::AllowOnce, " , ");
        assert_eq!(
            answer(p.decide(&request("x", "read"), Known::default())),
            "once"
        );
    }

    #[test]
    fn a_list_matches_by_prefix_of_title_kind_or_name_and_replaces_the_default() {
        let p = Policy::new(AutoApprove::AllowOnce, "Grep, edit");
        assert_eq!(
            answer(p.decide(&request("Grep foo", "search"), Known::default())),
            "once"
        );
        // Explicitly listed: an edit is approved.
        assert_eq!(
            answer(p.decide(&request("Write a.rs", "edit"), Known::default())),
            "once"
        );
        // The list replaces the read-only default: a read is not approved.
        assert!(
            answer(p.decide(&request("Read a", "read"), Known::default())).starts_with("forward")
        );
        // The tool name from the request or from what pons saw.
        let mut r = request("run it", "execute");
        r["toolCall"]["_meta"] = json!({"claudeCode": {"toolName": "Bash"}});
        let p = Policy::new(AutoApprove::AllowOnce, "bash");
        assert_eq!(answer(p.decide(&r, Known::default())), "once");
        let r = request("", "");
        let known = Known {
            title: "Glob **",
            kind: "search",
            name: "Glob",
        };
        let p = Policy::new(AutoApprove::AllowOnce, "glob");
        assert_eq!(answer(p.decide(&r, known)), "once");
    }

    #[test]
    fn allow_always_prefers_its_option_and_falls_back_to_once() {
        let p = Policy::new(AutoApprove::AllowAlways, "");
        assert_eq!(
            answer(p.decide(&request("x", "read"), Known::default())),
            "always"
        );
        let mut r = request("x", "read");
        r["options"] = json!([{"optionId": "once", "kind": "allow_once"}]);
        assert_eq!(answer(p.decide(&r, Known::default())), "once");
    }

    #[test]
    fn a_request_without_a_matching_option_is_forwarded() {
        let p = Policy::new(AutoApprove::AllowOnce, "");
        let mut r = request("x", "read");
        r["options"] = json!([{"optionId": "no", "kind": "reject_once"}]);
        assert!(answer(p.decide(&r, Known::default())).starts_with("forward"));
        r.as_object_mut().unwrap().remove("options");
        assert!(answer(p.decide(&r, Known::default())).starts_with("forward"));
    }

    #[test]
    fn plan_and_question_requests_are_never_answered() {
        let p = Policy::new(AutoApprove::AllowAlways, "exit, ask, other, switch");
        let mut r = request("Ready to code?", "switch_mode");
        r["toolCall"]["_meta"] = json!({"claudeCode": {"toolName": "ExitPlanMode"}});
        assert!(answer(p.decide(&r, Known::default())).starts_with("forward"));
        let r = request("AskUserQuestion", "other");
        assert!(answer(p.decide(&r, Known::default())).starts_with("forward"));
    }
}
