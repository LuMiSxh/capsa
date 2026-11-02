// SPDX-License-Identifier: MPL-2.0
//! Handles `initialize`: detects native subagent support and adds the transcript capability.

use super::*;

impl Engine {
    /// Handles the client's `initialize`: notes its capabilities and, when pons folds
    /// subagents, asks the adapter to forward their text.
    pub(super) fn on_initialize(&mut self, msg: Value, raw: &[u8]) -> Vec<Action> {
        self.init_id = msg.get("id").map(Value::to_string);
        self.client_notices = present(msg.pointer("/params/clientCapabilities/session/notices"));
        let mut out = Vec::new();
        let native = present(msg.pointer("/params/clientCapabilities/subagents"));
        if native {
            out.push(Action::Note(if self.cfg.force_features {
                "client renders subagents natively; --force-features keeps pons' subagent features on".into()
            } else {
                self.aside = true;
                "client renders subagents natively: stepping aside (folding, background cards, finished entries, roster, notices and the cancel filter are off; --force-features keeps them)".into()
            }));
        }
        let mut msg = msg;
        let on = self.cfg.fold && !self.aside;
        let had = msg
            .pointer("/params/clientCapabilities/_meta/subagent-transcript")
            .and_then(Value::as_bool)
            == Some(true);
        if on && had {
            self.forwards_text = true;
        }
        if on && self.cfg.subagent_text && !native && inject_transcript(&mut msg) {
            self.forwards_text = true;
            out.push(Action::Note(
                "asked the adapter to forward subagent text: added clientCapabilities._meta[\"subagent-transcript\"] = true to initialize (--no-subagent-text disables)".into(),
            ));
            out.push(Action::ToAgent(msg.to_string().into_bytes()));
        } else {
            out.push(Action::ToAgent(raw.to_vec()));
        }
        if on {
            self.folds.set_text_visible(self.forwards_text);
        }
        out
    }
}
