// SPDX-License-Identifier: MPL-2.0
//! Folds subagent activity into the parent Task/Agent tool call.
//!
//! Pure state and rendering: no I/O, time is injected, so everything here is
//! unit-testable without processes. Everything pons renders is plain ASCII.
//!
//! Finish detection for a background card (see `Folds::poll`):
//!
//! - confirmed: a Stop, a `task-notification` marker matched to a candidate,
//!   or the response of the prompt that launched the card;
//! - provisional: silence. Only a card whose last sign of life was agent text
//!   (no tool open) counts as done after `--subagent-idle-ms`; with the text
//!   not visible at all only the long `--subagent-max-quiet-ms` cap applies.
//!   A provisional card is never retired; later activity reopens it.

mod background;
mod child;
mod entry;
mod label;
mod retire;
#[cfg(test)]
mod tests;

use std::collections::{HashMap, HashSet, VecDeque};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

const MAX_LINES: usize = 30;
const MAX_TITLE_CHARS: usize = 160;
const MAX_TEXT_CHARS: usize = 300;
const MAX_TEXT_BUFFER: usize = 2000;
/// Longest final agent message of a subagent shown in a finished card or
/// entry (the head is kept: results lead with the answer).
const MAX_RESULT_CHARS: usize = 2000;
/// How much of the current agent message is kept raw to build the result
/// from (whitespace is squeezed afterwards).
const MAX_RESULT_BUFFER: usize = 8000;
/// Longest "last action" shown in a card title or roster entry.
const MAX_ACTION_CHARS: usize = 60;
/// A running card's elapsed time is shown in steps of this size, so its title
/// and roster text change at most this often.
const ELAPSED_STEP: u64 = 5;
/// Silence after which a running card says `quiet Ns` in its title.
const QUIET_SHOWN: u64 = 10;
/// Silence after a tool result before the idle rule finishes a subagent whose
/// last event was that result (no text since). A result is often followed by
/// thinking, so the wait is longer than `--subagent-idle-ms`; the configured
/// idle wins when it is longer.
const TOOL_RESULT_IDLE: Duration = Duration::from_secs(60);
/// How long a finished background subagent stays known (roster, reopening).
const FINISHED_KEEP: Duration = Duration::from_secs(60);
/// A card that shows activity this long after a `task-notification` ended it
/// was ended wrongly (the notification was another subagent's): the marker is
/// matched again. Later activity is a resumed subagent, whose notification was
/// its own, and must not end another card.
const REMATCH_WINDOW: Duration = Duration::from_secs(30);
/// How long a provisionally finished card keeps its log (hidden from the
/// roster after [`FINISHED_KEEP`]) before only a tombstone remains.
const HIDDEN_KEEP: Duration = Duration::from_secs(3600);
/// Most tombstones kept; the oldest go first.
const MAX_TOMBSTONES: usize = 512;
/// Most simultaneously open tool calls tracked per card.
const MAX_OPEN_TOOLS: usize = 1024;
/// Prefix of a tool call id that pons made up; never a subagent parent.
pub const ENTRY_PREFIX: &str = "pons-finished-";

/// End state of a background subagent card.
#[derive(Debug, Clone, Copy)]
struct Done {
    failed: bool,
    /// The `task-notification` that ended it, if any; given back when the
    /// card is reopened because it turned out to be a different subagent.
    by: Option<Instant>,
}

/// What a subagent did last, as far as the wire shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// Launched; nothing seen from it yet.
    Launched,
    /// A tool call started (or reported progress while running).
    ToolOpen,
    /// A tool call ended.
    ToolResult,
    /// Thinking was forwarded.
    Thought,
    /// Agent text: the shape of a subagent writing its final message.
    Text,
}

/// The "finished" entry as it goes out.
#[derive(Debug)]
pub enum EntryOut {
    /// A new tool call appended to the thread (held for a safe boundary).
    Append(Value),
    /// An update of the entry that was appended before; sent at once.
    Update(Value),
}

/// A background subagent card that pons finished on its own.
#[derive(Debug)]
pub struct Finished {
    pub session: String,
    pub parent: String,
    pub reason: &'static str,
    /// A confirmed finish (Stop, marker, prompt response); idle finishes are
    /// provisional guesses the subagent can contradict.
    pub confirmed: bool,
    /// The card's final update; `None` when a provisional finish was only
    /// confirmed (the card already shows it).
    pub update: Option<Value>,
    pub entry: Option<EntryOut>,
    /// The same news as a session notice (sent only where the client takes it,
    /// and only for confirmed finishes).
    pub notice: Value,
}

/// One subagent as the plan roster lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RosterItem {
    pub content: String,
    /// `pending`, `in_progress` or `completed`.
    pub status: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LineState {
    Running,
    Done,
    Failed,
}

impl LineState {
    fn from_status(status: &str) -> Self {
        match status {
            "completed" => Self::Done,
            "failed" => Self::Failed,
            _ => Self::Running,
        }
    }

    fn word(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Done => "done",
            Self::Failed => "failed",
        }
    }
}

#[derive(Debug)]
struct Line {
    key: Option<String>,
    state: LineState,
    title: String,
}

/// What survives of a retired card: enough to keep the entry id stable and to
/// recreate the card as a background one if the subagent turns out alive.
#[derive(Debug)]
struct Tombstone {
    session: String,
    finishes: usize,
    entry_sent: bool,
    title: Option<String>,
}

/// Fold log of one parent tool call.
#[derive(Debug)]
pub struct FoldEntry {
    session: String,
    parent: String,
    lines: VecDeque<Line>,
    /// Agent text since the last tool call of the subagent.
    text: String,
    /// The head of the same text, for the final result.
    head: String,
    /// Content of the parent's own tool call, kept above the log.
    base: Vec<Value>,
    /// The log changed: the next emit carries the full content.
    dirty: bool,
    /// Only the title changed (elapsed or quiet time): a small update.
    title_dirty: bool,
    last_emit: Option<Instant>,
    /// Launched in the background: the adapter reported the tool call
    /// completed at launch, so pons owns the card's lifecycle from here.
    background: bool,
    launched: Option<Instant>,
    last_child: Option<Instant>,
    done: Option<Done>,
    finished_at: Option<Instant>,
    tool_calls: usize,
    footer: Option<String>,
    /// The parent card's own title, once known (the retitled description).
    /// Never overwritten by the counters pons puts into the card title.
    title: Option<String>,
    /// The card title pons sent last.
    last_title: Option<String>,
    /// How many times this card was finished.
    finishes: usize,
    /// Tool calls of the subagent that have not ended. Kept apart from the
    /// capped log lines.
    open_tools: HashSet<String>,
    last_kind: Kind,
    /// The prompt (request id as JSON text) during which the card launched.
    launched_by: Option<String>,
    /// The "finished" entry was appended to the thread.
    entry_sent: bool,
    /// Finished by silence only: the subagent may still be working.
    provisional: bool,
    /// A provisionally finished card that left the roster but kept its log.
    hidden: bool,
    /// The parent's `rawInput` once it has content: the subagent's type, description and prompt.
    input: Option<Value>,
}

impl FoldEntry {}

/// All fold logs, keyed by parent tool call id.
#[derive(Debug)]
pub struct Folds {
    throttle: Duration,
    entries: HashMap<String, FoldEntry>,
    /// Tool calls made by subagents, mapped to the top-level parent they
    /// belong to, so nested subagents fold into the card the client knows.
    child_root: HashMap<String, String>,
    /// Silence after which a background subagent that ended with agent text
    /// counts as done; zero: never by silence.
    idle: Duration,
    /// Silence after which a subagent counts as done when its text is not
    /// visible (so `idle` cannot tell); zero: never.
    max_quiet: Duration,
    /// The adapter forwards subagent text, so a final message can be seen.
    text_visible: bool,
    /// How long a `task-notification` waits for the other background
    /// subagents to prove they are still active before pons picks which one
    /// ended. Zero picks at once, by the order of last activity.
    settle: Duration,
    /// `task-notification` markers not yet matched to a subagent.
    notifications: Vec<(String, Instant)>,
    /// Announce a finished background subagent with a new tool call.
    finish_entry: bool,
    /// What is left of retired cards, by parent id.
    tombstones: HashMap<String, Tombstone>,
    tomb_order: VecDeque<String>,
    /// Decisions worth a log line, taken by the engine.
    notes: Vec<String>,
}

impl Folds {
    pub fn new(throttle: Duration) -> Self {
        Self {
            throttle,
            entries: HashMap::new(),
            child_root: HashMap::new(),
            idle: Duration::ZERO,
            max_quiet: Duration::ZERO,
            text_visible: true,
            settle: Duration::ZERO,
            notifications: Vec::new(),
            finish_entry: false,
            tombstones: HashMap::new(),
            tomb_order: VecDeque::new(),
            notes: Vec::new(),
        }
    }

    pub fn with_finish_entry(mut self, on: bool) -> Self {
        self.finish_entry = on;
        self
    }

    pub fn with_idle(mut self, idle: Duration) -> Self {
        self.idle = idle;
        self
    }

    pub fn with_max_quiet(mut self, max_quiet: Duration) -> Self {
        self.max_quiet = max_quiet;
        self
    }

    pub fn with_settle(mut self, settle: Duration) -> Self {
        self.settle = settle;
        self
    }

    /// Records whether the adapter forwards subagent text (known after `initialize`).
    pub fn set_text_visible(&mut self, visible: bool) {
        self.text_visible = visible;
    }

    /// Takes the decisions made since the last call, for the log.
    pub fn take_notes(&mut self) -> Vec<String> {
        std::mem::take(&mut self.notes)
    }

    /// Returns the top-level parent a `parentToolUseId` folds into.
    pub fn resolve(&self, parent: &str) -> String {
        self.child_root
            .get(parent)
            .cloned()
            .unwrap_or_else(|| parent.to_owned())
    }

    /// Remembers the parent's own `tool_call` so its content stays above the log.
    pub fn register_parent(&mut self, session: &str, update: &Value) {
        let Some(id) = str_field(update, "toolCallId") else {
            return;
        };
        let entry = self
            .entries
            .entry(id.to_owned())
            .or_insert_with(|| FoldEntry::new(session, id));
        entry.note_title(update);
        if let Some(base) = base_of(update) {
            entry.base = base;
        }
    }

    /// Keeps the parent's `rawInput` once it is known, for the roster label.
    pub fn note_input(&mut self, parent: &str, update: &Value) {
        if let Some(entry) = self.entries.get_mut(parent) {
            entry.note_input(update);
        }
    }
}

fn base_of(update: &Value) -> Option<Vec<Value>> {
    update.get("content")?.as_array().cloned()
}

fn str_field<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    v.get(key)?.as_str()
}

fn tool_name(update: &Value) -> Option<&str> {
    update.pointer("/_meta/claudeCode/toolName")?.as_str()
}

/// Formats elapsed seconds as `45s`, `3m 05s` or `1h 02m`.
fn fmt_elapsed(secs: u64) -> String {
    match secs {
        0..60 => format!("{secs}s"),
        60..3600 => format!("{}m {:02}s", secs / 60, secs % 60),
        _ => format!("{}h {:02}m", secs / 3600, secs % 3600 / 60),
    }
}

pub(crate) fn one_line(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub(crate) fn clip_head(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_owned();
    }
    let mut out: String = s.chars().take(max.saturating_sub(3)).collect();
    out.push_str("...");
    out
}

fn clip_tail(s: &str, max: usize) -> String {
    let count = s.chars().count();
    if count <= max {
        return s.to_owned();
    }
    let tail: String = s.chars().skip(count - (max - 3)).collect();
    format!("...{tail}")
}
