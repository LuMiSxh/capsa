// SPDX-License-Identifier: MPL-2.0
//! Removes the adapter's raw `<task-notification>` blocks, and while a session
//! is replayed its `[Request interrupted by user]` markers, from the user
//! message stream.
//!
//! When Zed loads a session, the adapter replays the history. A background
//! task's notification (`<task-notification><task-id>...</result>...
//! </task-notification>`) is persisted as a user record, and adapters that do
//! not hide it send it as a `user_message_chunk`, which Zed shows as a user
//! message holding the raw XML. The finished entry already tells the user the
//! same news, so pons holds such a message back until it is complete and drops
//! it. Anything else is forwarded unchanged: text that cannot start a
//! notification passes at once, and text held back on a prefix is released in
//! its original lines as soon as it turns out not to be one.
//!
//! Claude Code writes an interrupt marker into its transcript as a user
//! record, so a replay shows it as a user message the user never typed. It is
//! matched the same way (whole trimmed text, possibly split over chunks), but
//! only when the caller says the session is replaying.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use serde_json::Value;

const OPEN: &str = "<task-notification>";
const CLOSE: &str = "</task-notification>";
/// The texts Claude Code records when the user interrupts a turn.
const MARKERS: [&str; 2] = [
    "[Request interrupted by user]",
    "[Request interrupted by user for tool use]",
];
/// A held message is not completed after this long: forward it as it came.
const MAX_HOLD: Duration = Duration::from_secs(1);
/// A message longer than this is no notification pons wants to buffer.
const MAX_BYTES: usize = 256 * 1024;

#[derive(Debug)]
struct Pending {
    /// The chunks as the adapter sent them.
    lines: Vec<Vec<u8>>,
    text: String,
    /// The `update` object of the first chunk, for the remainder.
    update: Value,
    last: Instant,
}

#[derive(Debug, Default)]
pub struct TaskNotifications {
    pending: HashMap<String, Pending>,
}

/// Which user messages the filter looks for.
#[derive(Debug, Clone, Copy)]
pub struct Watch {
    pub notifications: bool,
    pub markers: bool,
}

/// Tells whether `text`, trimmed, is an interrupt marker.
pub fn is_interrupt_marker(text: &str) -> bool {
    MARKERS.contains(&text.trim())
}

/// What to do with one agent message.
#[derive(Debug, PartialEq)]
pub enum Verdict {
    /// Not ours: handle the message as usual.
    Pass,
    /// Held back; send nothing yet.
    Hold,
    /// The notification is complete and dropped. `rest` is an update for the
    /// text that followed it in the same message, if any.
    Drop { chars: usize, rest: Option<Value> },
    /// An interrupt marker, complete and dropped.
    DropMarker,
    /// Not a notification after all: send these original lines, in order,
    /// instead of the message (it is the last of them).
    Release(Vec<Vec<u8>>),
}

/// Returns the text of a plain `user_message_chunk` of the main agent.
fn user_text(update: &Value) -> Option<&str> {
    if update.get("sessionUpdate")?.as_str()? != "user_message_chunk"
        || update
            .pointer("/_meta/claudeCode/parentToolUseId")
            .is_some()
    {
        return None;
    }
    let content = update.get("content")?;
    if content.get("type")?.as_str()? != "text" {
        return None;
    }
    content.get("text")?.as_str()
}

/// What `text` can still become: a notification, certainly not, or unknown.
enum Shape {
    Notification,
    Marker,
    Maybe,
    Other,
}

fn shape(text: &str, watch: Watch) -> Shape {
    let head = text.trim_start();
    if watch.notifications && head.starts_with(OPEN) {
        Shape::Notification
    } else if watch.markers && is_interrupt_marker(head) {
        Shape::Marker
    } else if !head.is_empty()
        && ((watch.notifications && OPEN.starts_with(head))
            || (watch.markers && MARKERS.iter().any(|m| m.starts_with(head))))
    {
        Shape::Maybe
    } else {
        Shape::Other
    }
}

impl TaskNotifications {
    /// Drops what is held for the closed session.
    pub fn forget(&mut self, session: &str) {
        self.pending.remove(session);
    }

    /// Looks at an agent `session/update` of `session`.
    pub fn on_update(
        &mut self,
        session: &str,
        update: &Value,
        raw: &[u8],
        now: Instant,
        watch: Watch,
    ) -> Verdict {
        let Some(chunk) = user_text(update) else {
            return Verdict::Pass;
        };
        let held = self.pending.remove(session);
        if held.is_none() && matches!(shape(chunk, watch), Shape::Other) {
            // The common case: ordinary user text, which is neither copied nor held.
            return Verdict::Pass;
        }
        let mut p = held.unwrap_or_else(|| Pending {
            lines: Vec::new(),
            text: String::new(),
            update: update.clone(),
            last: now,
        });
        p.lines.push(raw.to_vec());
        p.text.push_str(chunk);
        p.last = now;
        match shape(&p.text, watch) {
            Shape::Marker => Verdict::DropMarker,
            Shape::Other => {
                if p.lines.len() == 1 {
                    return Verdict::Pass;
                }
                Verdict::Release(p.lines)
            }
            Shape::Maybe => self.hold(session, p),
            Shape::Notification => {
                let head = p.text.trim_start();
                if let Some(end) = head.find(CLOSE) {
                    let rest = head[end + CLOSE.len()..].trim_start();
                    let rest = (!rest.is_empty()).then(|| {
                        let mut u = p.update.clone();
                        u["content"]["text"] = Value::from(rest);
                        u
                    });
                    Verdict::Drop {
                        chars: p.text.chars().count(),
                        rest,
                    }
                } else if p.text.len() > MAX_BYTES {
                    Verdict::Release(p.lines)
                } else {
                    self.hold(session, p)
                }
            }
        }
    }

    fn hold(&mut self, session: &str, p: Pending) -> Verdict {
        self.pending.insert(session.to_owned(), p);
        Verdict::Hold
    }

    /// Flushes what is held for `session` when any other message arrives (`None`: of any
    /// session), returning the lines unchanged.
    pub fn flush(&mut self, session: Option<&str>) -> Vec<Vec<u8>> {
        let ids: Vec<String> = self
            .pending
            .keys()
            .filter(|id| session.is_none_or(|s| s == id.as_str()))
            .cloned()
            .collect();
        let mut ids = ids;
        ids.sort();
        ids.iter()
            .filter_map(|id| self.pending.remove(id))
            .flat_map(|p| p.lines)
            .collect()
    }

    /// Returns the held text that has not been completed for [`MAX_HOLD`].
    pub fn flush_stale(&mut self, now: Instant) -> Vec<Vec<u8>> {
        let mut stale: Vec<String> = self
            .pending
            .iter()
            .filter(|(_, p)| now.saturating_duration_since(p.last) >= MAX_HOLD)
            .map(|(id, _)| id.clone())
            .collect();
        stale.sort();
        stale
            .iter()
            .filter_map(|id| self.pending.remove(id))
            .flat_map(|p| p.lines)
            .collect()
    }

    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn chunk(text: &str) -> Value {
        json!({"sessionUpdate": "user_message_chunk",
            "content": {"type": "text", "text": text}, "messageId": "m1"})
    }

    fn feed(f: &mut TaskNotifications, text: &str, now: Instant) -> Verdict {
        f.on_update("s", &chunk(text), text.as_bytes(), now, BOTH)
    }

    const BOTH: Watch = Watch {
        notifications: true,
        markers: true,
    };

    fn feed_with(f: &mut TaskNotifications, text: &str, watch: Watch) -> Verdict {
        f.on_update("s", &chunk(text), text.as_bytes(), Instant::now(), watch)
    }

    const NOTE: &str = "<task-notification><task-id>t1</task-id><tool-use-id>toolu_1</tool-use-id>\
        <output-file>/tmp/x</output-file><status>completed</status><summary>Agent done</summary>\
        <note>n</note><result>The result</result><usage>1</usage></task-notification>";

    #[test]
    fn a_whole_notification_in_one_chunk_is_dropped() {
        let mut f = TaskNotifications::default();
        let v = feed(&mut f, NOTE, Instant::now());
        assert_eq!(
            v,
            Verdict::Drop {
                chars: NOTE.chars().count(),
                rest: None
            }
        );
        assert!(f.is_empty());
    }

    #[test]
    fn a_split_notification_is_held_until_it_closes() {
        let mut f = TaskNotifications::default();
        let t = Instant::now();
        let (a, b) = NOTE.split_at(10);
        let (b, c) = b.split_at(60);
        assert_eq!(feed(&mut f, a, t), Verdict::Hold);
        assert_eq!(feed(&mut f, b, t), Verdict::Hold);
        assert!(matches!(
            feed(&mut f, c, t),
            Verdict::Drop { rest: None, .. }
        ));
        assert!(f.is_empty());
    }

    #[test]
    fn text_after_the_notification_is_kept() {
        let mut f = TaskNotifications::default();
        let v = feed(&mut f, &format!("{NOTE}\nPlease go on"), Instant::now());
        let Verdict::Drop { rest: Some(u), .. } = v else {
            panic!("{v:?}");
        };
        assert_eq!(u["content"]["text"], "Please go on");
        assert_eq!(u["messageId"], "m1");
    }

    #[test]
    fn ordinary_user_text_passes_at_once() {
        let mut f = TaskNotifications::default();
        let t = Instant::now();
        assert_eq!(feed(&mut f, "hello", t), Verdict::Pass);
        assert_eq!(feed(&mut f, "<b>bold</b>", t), Verdict::Pass);
        assert_eq!(feed(&mut f, "", t), Verdict::Pass);
        assert!(f.is_empty());
    }

    #[test]
    fn the_matcher_accepts_only_the_two_markers_trimmed() {
        assert!(is_interrupt_marker("[Request interrupted by user]"));
        assert!(is_interrupt_marker(
            " \n[Request interrupted by user for tool use]\n"
        ));
        assert!(!is_interrupt_marker("[Request interrupted by user] why?"));
        assert!(!is_interrupt_marker("[request interrupted by user]"));
        assert!(!is_interrupt_marker("[Request interrupted by user"));
        assert!(!is_interrupt_marker(""));
    }

    #[test]
    fn a_marker_is_dropped_whole_or_split() {
        let mut f = TaskNotifications::default();
        let t = Instant::now();
        assert_eq!(
            feed(&mut f, "[Request interrupted by user]", t),
            Verdict::DropMarker
        );
        assert_eq!(feed(&mut f, "[Request inter", t), Verdict::Hold);
        assert_eq!(feed(&mut f, "rupted by user for tool", t), Verdict::Hold);
        assert_eq!(feed(&mut f, " use]", t), Verdict::DropMarker);
        assert!(f.is_empty());
    }

    #[test]
    fn text_that_only_starts_like_a_marker_is_released_unchanged() {
        let mut f = TaskNotifications::default();
        let t = Instant::now();
        assert_eq!(feed(&mut f, "[Request", t), Verdict::Hold);
        assert_eq!(
            feed(&mut f, " for help]", t),
            Verdict::Release(vec![b"[Request".to_vec(), b" for help]".to_vec()])
        );
        assert_eq!(feed(&mut f, "[todo] x", t), Verdict::Pass);
        assert_eq!(
            feed(&mut f, "[Request interrupted by user] x", t),
            Verdict::Pass
        );
    }

    #[test]
    fn markers_pass_when_not_watched() {
        let mut f = TaskNotifications::default();
        let only_notes = Watch {
            notifications: true,
            markers: false,
        };
        assert_eq!(
            feed_with(&mut f, "[Request interrupted by user]", only_notes),
            Verdict::Pass
        );
        let only_markers = Watch {
            notifications: false,
            markers: true,
        };
        assert_eq!(feed_with(&mut f, NOTE, only_markers), Verdict::Pass);
        assert!(f.is_empty());
    }

    #[test]
    fn a_prefix_that_turns_out_different_is_released_unchanged() {
        let mut f = TaskNotifications::default();
        let t = Instant::now();
        assert_eq!(feed(&mut f, "<task", t), Verdict::Hold);
        assert_eq!(
            feed(&mut f, "s are fun", t),
            Verdict::Release(vec![b"<task".to_vec(), b"s are fun".to_vec()])
        );
        assert!(f.is_empty());
    }

    #[test]
    fn other_updates_and_other_content_are_not_touched() {
        let mut f = TaskNotifications::default();
        let t = Instant::now();
        let agent = json!({"sessionUpdate": "agent_message_chunk",
            "content": {"type": "text", "text": NOTE}});
        assert_eq!(f.on_update("s", &agent, b"x", t, BOTH), Verdict::Pass);
        let image = json!({"sessionUpdate": "user_message_chunk",
            "content": {"type": "image", "data": "", "mimeType": "image/png"}});
        assert_eq!(f.on_update("s", &image, b"x", t, BOTH), Verdict::Pass);
        let child = json!({"sessionUpdate": "user_message_chunk",
            "content": {"type": "text", "text": NOTE},
            "_meta": {"claudeCode": {"parentToolUseId": "P"}}});
        assert_eq!(f.on_update("s", &child, b"x", t, BOTH), Verdict::Pass);
    }

    #[test]
    fn held_text_is_flushed_by_the_next_message_or_after_a_second() {
        let mut f = TaskNotifications::default();
        let t = Instant::now();
        assert_eq!(feed(&mut f, NOTE.split_at(30).0, t), Verdict::Hold);
        assert!(f.flush_stale(t + Duration::from_millis(999)).is_empty());
        assert_eq!(f.flush_stale(t + Duration::from_secs(1)).len(), 1);

        assert_eq!(feed(&mut f, NOTE.split_at(30).0, t), Verdict::Hold);
        assert!(f.flush(Some("other")).is_empty());
        assert_eq!(f.flush(Some("s")).len(), 1);
        assert!(f.is_empty());
    }

    #[test]
    fn sessions_are_held_apart() {
        let mut f = TaskNotifications::default();
        let t = Instant::now();
        let (a, b) = NOTE.split_at(30);
        assert_eq!(
            f.on_update("s1", &chunk(a), a.as_bytes(), t, BOTH),
            Verdict::Hold
        );
        assert_eq!(feed(&mut f, "hello", t), Verdict::Pass);
        assert!(matches!(
            f.on_update("s1", &chunk(b), b.as_bytes(), t, BOTH),
            Verdict::Drop { .. }
        ));
    }
}
