// SPDX-License-Identifier: MPL-2.0
//! Message-level logic of the proxy, free of I/O.
//! Each input line yields the lines to write to either side; unhandled lines pass unchanged.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::failsafe::{FailSafe, catch_panic, panic_text};
use crate::fold::{EntryOut, Finished, Folds, clip_head, one_line};
use crate::notification::{TaskNotifications, Verdict, Watch};
use crate::permission::{AutoApprove, Decision, Known, Policy};
use crate::roster::Roster;
use crate::state::{OpenTool, PendingCancel, Session};

mod cancel;
mod clock;
mod hold;
mod initialize;
mod response;
mod route;
mod steer;
mod updates;
mod wire;

use wire::*;

#[cfg(test)]
#[expect(
    clippy::needless_pass_by_value,
    reason = "test fixtures take the JSON they wrap by value, so each call reads as one expression"
)]
mod tests;

const FOLD_THROTTLE: Duration = Duration::from_millis(300);
/// Least time between two roster plans of a session.
const ROSTER_INTERVAL: Duration = Duration::from_secs(1);

/// When pons sends a `notice` session update.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum NoticeMode {
    /// Only to a client that advertised `session.notices` at `initialize`.
    #[default]
    Auto,
    /// Whatever the client advertised.
    Always,
    Never,
}

/// What happens to a message the user sends while subagents run (Zed sends
/// `session/cancel` and then `session/prompt`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum InterruptMode {
    /// Inject the message into the running turn with `_session/steering`.
    Steer,
    /// Drop the cancel; the adapter queues the new prompt behind the turn.
    Queue,
    /// Forward the cancel (kills the background subagents).
    Cancel,
}

#[derive(Debug, Clone)]
pub struct Config {
    pub cancel_filter: bool,
    pub fold: bool,
    /// How long a `session/cancel` is held to see whether a prompt follows.
    pub cancel_defer: Duration,
    /// `None`: steer when the adapter advertised steering, else queue.
    pub interrupt: Option<InterruptMode>,
    pub stop_window: Duration,
    /// Silence after which a background subagent is shown as done; zero: never.
    pub subagent_idle: Duration,
    /// Safety cap used instead of the idle rule when the adapter's subagent
    /// text is not visible (`--no-subagent-text`); zero: never.
    pub subagent_max_quiet: Duration,
    /// How long the proxy waits for the agent to exit after the client
    /// closed, before it signals the agent's process group.
    pub shutdown_grace: Duration,
    /// Announce finished background subagents with a new tool call.
    pub finish_entry: bool,
    /// The main agent must have streamed no text for this long before a
    /// "finished" entry is appended to the thread (other boundaries release
    /// it sooner); zero appends at once.
    pub entry_gap: Duration,
    /// How long a `task-notification` waits before pons picks the subagent
    /// that ended; zero picks at once.
    pub notify_settle: Duration,
    /// List background subagents in the plan.
    pub roster: bool,
    pub notices: NoticeMode,
    /// Drop the adapter's raw `<task-notification>` user messages.
    pub drop_task_notifications: bool,
    /// Drop the `[Request interrupted by user]` user messages a replay shows.
    pub drop_interrupt_markers: bool,
    /// Keep pons' subagent features on even when the client renders
    /// subagents natively.
    pub force_features: bool,
    /// Ask the adapter to forward subagent text by adding
    /// `_meta["subagent-transcript"]` to the client capabilities.
    pub subagent_text: bool,
    /// Consecutive failures after which a session becomes pure passthrough;
    /// 0 never switches.
    pub max_failures: u32,
    /// Test hook: a line containing this text makes the rules fail.
    pub inject_failure: Option<String>,
    /// A turn whose last word was text or a usage update and that then goes
    /// silent this long, with nothing open, is answered `end_turn` by pons;
    /// zero never does.
    pub turn_stall: Duration,
    /// Close the open tool calls of a cancelled turn with `failed`.
    pub close_ghost_calls: bool,
    /// How long a steered message waits for a streaming tool call to finish
    /// its input; zero steers at once.
    pub steer_hold: Duration,
    /// Answer matching permission requests without asking the client.
    pub auto_approve: AutoApprove,
    /// Comma separated tool names for `auto_approve` (empty: read-only kinds).
    pub auto_approve_tools: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            cancel_filter: true,
            fold: true,
            cancel_defer: Duration::from_millis(400),
            interrupt: None,
            stop_window: Duration::from_millis(3000),
            subagent_idle: Duration::from_millis(20000),
            subagent_max_quiet: Duration::from_millis(600_000),
            shutdown_grace: Duration::from_millis(2000),
            finish_entry: true,
            entry_gap: Duration::from_millis(600),
            notify_settle: Duration::ZERO,
            roster: true,
            notices: NoticeMode::Auto,
            drop_task_notifications: true,
            drop_interrupt_markers: true,
            force_features: false,
            subagent_text: true,
            max_failures: 5,
            inject_failure: None,
            turn_stall: Duration::from_millis(120_000),
            close_ghost_calls: true,
            steer_hold: Duration::from_millis(1500),
            auto_approve: AutoApprove::None,
            auto_approve_tools: String::new(),
        }
    }
}

/// A "finished" entry waiting for a safe place in the main agent's output.
#[derive(Debug)]
struct Held {
    session: String,
    parent: String,
    update: Value,
}

/// A `_session/steering` request sent to the agent, waiting for its reply.
#[derive(Debug)]
struct SteerPending {
    session: String,
    /// The prompt (as JSON text) whose turn the message goes into.
    old_key: String,
    /// Zed's new prompt, which pons answers when that turn ends.
    new_id: Value,
    /// Zed's `session/prompt` line, forwarded if steering does not take.
    raw: Vec<u8>,
    /// The old prompt's response, if it came before the steering reply.
    ended: Option<Value>,
}

/// A steered message waiting for a streaming tool call to complete its input.
#[derive(Debug)]
struct HeldSteer {
    id: Value,
    raw: Vec<u8>,
    prompt: Value,
    /// The prompt (as JSON text) whose turn the message goes into.
    old_key: String,
    until: Instant,
    head: String,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Action {
    ToAgent(Vec<u8>),
    ToClient {
        line: Vec<u8>,
        synthesized: bool,
    },
    /// Something worth a line in the log.
    Note(String),
}

pub struct Engine {
    cfg: Config,
    sessions: HashMap<String, Session>,
    /// Outstanding prompt request ids (as JSON text) to their session.
    prompts: HashMap<String, String>,
    /// Prompt ids answered by us already; the agent's response is dropped.
    swallow: HashMap<String, String>,
    /// `initialize` request id (as JSON text) whose reply is still awaited.
    init_id: Option<String>,
    /// The agent advertised `_meta.steering.supported` at `initialize`.
    steering: bool,
    steer_seq: u64,
    /// Steering requests (by their id as JSON text) without a reply yet.
    steers: HashMap<String, SteerPending>,
    /// Steering waiting for a tool call's input to complete, by session.
    held_steers: HashMap<String, HeldSteer>,
    policy: Policy,
    /// Prompts whose turn took a steered message: when the agent answers the
    /// key, pons answers the session's newer prompt with that result.
    handoffs: HashMap<String, (String, Value)>,
    folds: Folds,
    roster: Roster,
    /// Entries in finish order, not yet sent.
    held: Vec<Held>,
    /// The client advertised `session.notices` at `initialize`.
    client_notices: bool,
    notes: TaskNotifications,
    /// The client renders subagents natively: pons' subagent features are off.
    aside: bool,
    /// The adapter forwards subagent text to this client (pons asked for it,
    /// or the client did), and pons folds it.
    forwards_text: bool,
    /// Replayed subagent text chunks dropped, per replaying session.
    replay_dropped: HashMap<String, usize>,
    /// Replayed interrupt markers dropped, per replaying session.
    replay_markers: HashMap<String, usize>,
    /// Sessions replaying their history after `session/load` or
    /// `session/resume`, with the updates forwarded untouched so far.
    replaying: HashMap<String, usize>,
    /// Outstanding load/resume request ids (as JSON text) to their session.
    loads: HashMap<String, String>,
    guard: FailSafe,
    /// Sessions the client closed; their agent updates are forwarded as is.
    closed: HashSet<String>,
    /// Consecutive failures of the periodic work (separate from line rules).
    tick_failures: u32,
}

impl Engine {
    pub fn new(cfg: Config) -> Self {
        let folds = Folds::new(FOLD_THROTTLE)
            .with_idle(cfg.subagent_idle)
            .with_max_quiet(cfg.subagent_max_quiet)
            .with_settle(cfg.notify_settle)
            .with_finish_entry(cfg.finish_entry);
        let guard = FailSafe::new(cfg.max_failures);
        let policy = Policy::new(cfg.auto_approve, &cfg.auto_approve_tools);
        Self {
            cfg,
            sessions: HashMap::new(),
            prompts: HashMap::new(),
            swallow: HashMap::new(),
            init_id: None,
            steering: false,
            steer_seq: 0,
            steers: HashMap::new(),
            held_steers: HashMap::new(),
            policy,
            handoffs: HashMap::new(),
            folds,
            roster: Roster::new(ROSTER_INTERVAL),
            held: Vec::new(),
            client_notices: false,
            notes: TaskNotifications::default(),
            aside: false,
            forwards_text: false,
            replay_dropped: HashMap::new(),
            replay_markers: HashMap::new(),
            replaying: HashMap::new(),
            loads: HashMap::new(),
            guard,
            closed: HashSet::new(),
            tick_failures: 0,
        }
    }
}
