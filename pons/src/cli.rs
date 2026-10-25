// SPDX-License-Identifier: MPL-2.0
//! Command line: the proxy flags and the `pons self` subcommands.

use std::path::PathBuf;

use capsa_core::update::{UninstallArgs, UpdateArgs};
use clap::{Parser, Subcommand};

use crate::engine::{InterruptMode, McpServer};
use crate::log::LogLevel;
use crate::permission::AutoApprove;

/// Transparent ACP proxy between Zed and the Claude ACP adapter.
#[derive(Debug, Parser)]
#[command(
    version,
    override_usage = "pons [OPTIONS] [-- <AGENT> [ARGS]...]\n       pons self <doctor|setup|update|uninstall>",
    args_conflicts_with_subcommands = true,
    after_help = "AGENT\n  Without `-- <AGENT> [ARGS]...` pons starts the first of these it finds, on\n  PATH and then in ~/.local/bin, ~/.cargo/bin, ~/Library/pnpm(/bin),\n  ~/.local/share/pnpm(/bin), ~/.bun/bin, /opt/homebrew/bin, /usr/local/bin,\n  ~/.volta/bin, nvm and fnm node dirs (Windows: %APPDATA%\\npm,\n  %LOCALAPPDATA%\\pnpm, %USERPROFILE%\\.bun\\bin):\n    1. claude-agent-acp\n    2. npx -y @agentclientprotocol/claude-agent-acp\n    3. pnpm dlx @agentclientprotocol/claude-agent-acp (or pnpx)\n    4. bunx @agentclientprotocol/claude-agent-acp\n  An explicit command always wins. `pons self doctor` shows what would be used."
)]
pub struct Cli {
    #[command(subcommand)]
    pub sub: Option<Sub>,

    /// Append every message with direction and timestamp to FILE.
    #[arg(long, value_name = "FILE", env = "PONS_LOG")]
    pub log: Option<PathBuf>,

    /// Do not write a wire log (overrides --log and PONS_LOG).
    #[arg(long)]
    pub no_log: bool,

    /// Directory for the default per-start log file (used when no --log is
    /// given). Default: ~/Library/Logs/pons on macOS, $XDG_STATE_HOME/pons
    /// (else ~/.local/state/pons) on Linux, %LOCALAPPDATA%\pons\logs on Windows.
    #[arg(long, value_name = "DIR")]
    pub log_dir: Option<PathBuf>,

    /// Deprecated and ignored: whether the main agent is streaming no longer
    /// decides what a cancel does (see --cancel-defer-ms).
    #[arg(long, value_name = "MS")]
    pub quiet_ms: Option<u64>,

    /// While subagents run, a session/cancel is held this long to tell a
    /// Stop from a message the user is sending (Zed sends cancel, then
    /// prompt, ~100 ms apart). A prompt inside the window drops the cancel;
    /// otherwise it is forwarded when the window ends. 0 decides at the next
    /// 100 ms tick, so a prompt still has to be sent first.
    #[arg(long, value_name = "MS", default_value_t = 400)]
    pub cancel_defer_ms: u64,

    /// What happens to a message sent while subagents run. steer: inject it
    /// into the running turn with the adapter's `_session/steering`; queue:
    /// drop the cancel and let the adapter queue the prompt behind the turn;
    /// cancel: forward the cancel (kills the subagents). Default: steer if
    /// the adapter advertised steering at initialize, else queue. Steering
    /// that fails falls back to queue.
    #[arg(long, value_enum, value_name = "MODE")]
    pub interrupt_mode: Option<InterruptMode>,

    /// A second session/cancel within this long of a held one is a real
    /// Stop and is forwarded at once. 0 never forwards a repeated cancel.
    #[arg(long, value_name = "MS", default_value_t = 3000)]
    pub stop_window_ms: u64,

    /// A background subagent whose last sign of life was agent text (no tool
    /// call open) and that is silent this long is shown as done, provisionally:
    /// later activity reopens it. A subagent that is thinking or has a tool
    /// call open is only labelled "quiet Ns". 0 disables.
    #[arg(long, value_name = "MS", default_value_t = 20000)]
    pub subagent_idle_ms: u64,

    /// With --no-subagent-text the text of a subagent cannot be seen, so
    /// the idle rule cannot tell a final message from thinking: only this long
    /// silence (with no tool call open) shows it as done. 0 disables.
    #[arg(long, value_name = "MS", default_value_t = 600_000)]
    pub subagent_max_quiet_ms: u64,

    /// When the client closes the connection, wait this long for the agent to
    /// exit by itself (its stdin is closed) before SIGTERM, and as long again
    /// before SIGKILL, to its whole process group.
    #[arg(long, value_name = "MS", default_value_t = 2000)]
    pub shutdown_grace_ms: u64,

    /// A "finished" entry is appended to the thread only after the main agent
    /// has streamed no text for this long (or at a tool call or the end of
    /// the turn). 0 appends at once.
    #[arg(long, value_name = "MS", default_value_t = 600)]
    pub entry_gap_ms: u64,

    /// After a task-notification, wait this long before deciding which
    /// background subagent ended (the one quiet longest). 0 decides at once.
    #[arg(long, value_name = "MS", default_value_t = 0)]
    pub notify_settle_ms: u64,

    /// Always forward session/cancel.
    #[arg(long)]
    pub no_cancel_filter: bool,

    /// Forward subagent updates flat instead of folding them into the parent.
    #[arg(long)]
    pub no_fold: bool,

    /// Do not add a new "finished" tool call at the end of the thread when a
    /// background subagent finishes.
    #[arg(long)]
    pub no_finish_entry: bool,

    /// Do not list background subagents in the plan (activity bar).
    #[arg(long)]
    pub no_roster: bool,

    /// Never send a session notice when a background subagent finishes. By
    /// default one is sent only if the client advertised notice support.
    #[arg(long, conflicts_with = "notices")]
    pub no_notice: bool,

    /// Send the finish notice even if the client did not advertise notice
    /// support (Zed does so only with its beta features on).
    #[arg(long)]
    pub notices: bool,

    /// Forward the adapter's raw <task-notification> user messages (shown by
    /// Zed when it replays a session) instead of dropping them.
    #[arg(long)]
    pub keep_task_notifications: bool,

    /// Forward the `[Request interrupted by user]` user messages the adapter
    /// replays when a session is loaded or resumed, instead of dropping them.
    #[arg(long)]
    pub keep_interrupt_markers: bool,

    /// Keep pons' subagent features on even if the client advertises native
    /// subagent support (`clientCapabilities.subagents`), where pons steps
    /// aside by default.
    #[arg(long)]
    pub force_features: bool,

    /// Do not ask the adapter to forward subagent text (it adds
    /// `_meta["subagent-transcript"]` to the client capabilities in
    /// `initialize`). Without that text the cards and finished entries only
    /// show the tool call count.
    #[arg(long)]
    pub no_subagent_text: bool,

    /// After this many consecutive message failures in the rules a session
    /// becomes pure passthrough (the failing lines are always forwarded
    /// unchanged). 0 never switches.
    #[arg(long, value_name = "N", default_value_t = 5)]
    pub max_failures: u32,

    /// A prompt that got no response although the agent's last word was text
    /// or a usage update, with no tool call, subagent or question open and
    /// nothing received for this long, is answered `end_turn` by pons (a late
    /// real response is dropped). Works around adapter turns that never end.
    /// 0 disables.
    #[arg(long, value_name = "MS", default_value_t = 120_000)]
    pub turn_stall_ms: u64,

    /// Do not mark the open tool calls of a cancelled turn as failed. By
    /// default a Stop (not a message sent while subagents run) ends them with
    /// `tool_call_update status=failed`, so none stays "running" forever.
    #[arg(long)]
    pub no_close_ghost_calls: bool,

    /// A message steered into a running turn waits up to this long for a tool
    /// call that is still streaming its input to finish it (the adapter fails
    /// the steering otherwise). Steering that fails is queued. 0 steers at once.
    #[arg(long, value_name = "MS", default_value_t = 1500)]
    pub steer_hold_ms: u64,

    /// Answer the agent's permission requests without asking the client:
    /// none (default), allow-once or allow-always (see --auto-approve-tools).
    /// Anything pons answers is never shown to you. Opt in with care.
    #[arg(long, value_enum, value_name = "MODE", default_value_t = AutoApprove::None)]
    pub auto_approve: AutoApprove,

    /// Tools --auto-approve covers: a comma separated list of prefixes of a
    /// tool's title, kind or name (`Read,Grep,Glob`). Empty: only the
    /// read-only kinds read, search, think and fetch; edit, execute, delete
    /// and move only when listed.
    #[arg(long, value_name = "LIST", default_value = "")]
    pub auto_approve_tools: String,

    /// Adds a stdio MCP server to every session/new, session/load and
    /// session/resume request the client sends, as NAME=COMMAND[,ARG...].
    /// Repeatable. A name the client already sends keeps the client's entry.
    #[arg(long, value_name = "NAME=COMMAND[,ARG...]", value_parser = parse_mcp_server)]
    pub mcp_server: Vec<McpServer>,

    /// Test hook: lines containing this text make the rules fail.
    #[arg(long, value_name = "TEXT", env = "PONS_INJECT_FAILURE", hide = true)]
    pub inject_failure: Option<String>,

    /// How much of each message the wire log keeps.
    #[arg(long, value_enum, value_name = "LEVEL", default_value_t = LogLevel::Default)]
    pub log_level: LogLevel,

    /// Rotate an explicit --log file to FILE.1 once it would exceed this many
    /// bytes (0: never). Default per-start logs are not rotated.
    #[arg(long, value_name = "BYTES", default_value_t = 5 * 1024 * 1024)]
    pub log_max_bytes: u64,

    /// The real agent command; auto-detected when omitted (see below).
    #[arg(last = true, value_name = "AGENT")]
    pub agent: Vec<String>,
}

#[derive(Debug, Subcommand)]
pub enum Sub {
    /// Installation and health of pons itself: `self doctor`, `self setup`, `self update`,
    /// `self uninstall`.
    #[command(name = "self")]
    SelfCmd {
        #[command(subcommand)]
        action: SelfAction,
    },
}

#[derive(Debug, Subcommand)]
pub enum SelfAction {
    /// Print a plain-text report: version, launcher that would be used, Zed
    /// settings and adapter, and a summary of the wire log.
    Doctor {
        /// Print the report as one JSON object instead of text.
        #[arg(long, conflicts_with_all = ["human", "agent"])]
        json: bool,

        /// Print the report as text, coloured on a terminal (never with NO_COLOR).
        #[arg(long, conflicts_with_all = ["json", "agent"])]
        human: bool,

        /// Print the report as plain text without colour (the default for agents).
        #[arg(long, conflicts_with_all = ["json", "human"])]
        agent: bool,

        /// Wire log to summarize (default: PONS_LOG, else the newest log in
        /// the log directory, else pons.log in the temp directory if present).
        #[arg(long, value_name = "FILE", env = "PONS_LOG")]
        log: Option<PathBuf>,

        /// Directory whose newest pons-*.log is summarized when --log is not
        /// given (default: the default log directory).
        #[arg(long, value_name = "DIR")]
        log_dir: Option<PathBuf>,

        /// Report on this agent command instead of auto-detecting; takes
        /// everything after it.
        #[arg(long, value_name = "CMD", num_args = 1.., allow_hyphen_values = true)]
        agent_cmd: Vec<String>,
    },

    /// Make sure the Claude ACP adapter is installed: report it and its version
    /// when it is found, otherwise install `@agentclientprotocol/claude-agent-acp`
    /// globally.
    Setup {
        /// Print the report as one JSON object instead of text.
        #[arg(long, conflicts_with_all = ["human", "agent"])]
        json: bool,

        /// Print the report as text, coloured on a terminal (never with NO_COLOR).
        #[arg(long, conflicts_with_all = ["json", "agent"])]
        human: bool,

        /// Print the report as plain text without colour (the default for agents).
        #[arg(long, conflicts_with_all = ["json", "human"])]
        agent: bool,

        /// Package manager to install with. Default: the first of pnpm, bun and
        /// npm found on PATH.
        #[arg(long, value_enum, value_name = "NAME")]
        package_manager: Option<crate::setup::ManagerArg>,

        /// Print the command that would install the adapter and install nothing.
        #[arg(long)]
        dry_run: bool,
    },

    /// Replace this binary with the latest GitHub release.
    Update(UpdateArgs),

    /// Remove this binary. The wire logs and the Zed `agent_servers` entry stay.
    Uninstall(UninstallArgs),
}

/// Parses `NAME=COMMAND[,ARG...]`; the name ends at the first `=`.
fn parse_mcp_server(value: &str) -> Result<McpServer, String> {
    let (name, spec) = value
        .split_once('=')
        .ok_or_else(|| "expected NAME=COMMAND[,ARG...]".to_owned())?;
    if name.is_empty()
        || !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return Err(format!(
            "MCP server name `{name}` must be made of letters, digits, `_` and `-`"
        ));
    }
    let mut parts = spec.split(',').map(str::to_owned);
    let command = parts.next().unwrap_or_default();
    if command.is_empty() {
        return Err("the MCP server command must not be empty".to_owned());
    }
    Ok(McpServer {
        name: name.to_owned(),
        command,
        args: parts.collect(),
    })
}

#[cfg(test)]
mod tests {
    use clap::error::ErrorKind;

    use super::*;

    fn parse(values: &[&str]) -> Result<Vec<McpServer>, clap::Error> {
        let mut argv = vec!["pons"];
        for &value in values {
            argv.push("--mcp-server");
            argv.push(value);
        }
        Cli::try_parse_from(argv).map(|cli| cli.mcp_server)
    }

    fn server(name: &str, command: &str, args: &[&str]) -> McpServer {
        McpServer {
            name: name.to_owned(),
            command: command.to_owned(),
            args: args.iter().map(|arg| (*arg).to_owned()).collect(),
        }
    }

    #[test]
    fn parses_name_command_and_comma_separated_args() {
        assert_eq!(
            parse(&["velum=/Users/me/.cargo/bin/velum,mcp,--verbose"]).unwrap(),
            vec![server(
                "velum",
                "/Users/me/.cargo/bin/velum",
                &["mcp", "--verbose"]
            )]
        );
    }

    #[test]
    fn a_command_without_args_has_no_args() {
        assert_eq!(
            parse(&["velum=velum"]).unwrap(),
            vec![server("velum", "velum", &[])]
        );
    }

    #[test]
    fn only_the_first_equals_sign_splits_the_name() {
        assert_eq!(
            parse(&["velum=/bin/velum,--opt=1"]).unwrap(),
            vec![server("velum", "/bin/velum", &["--opt=1"])]
        );
    }

    #[test]
    fn the_flag_repeats_and_keeps_its_order() {
        assert_eq!(
            parse(&["a=one", "b=two,x"]).unwrap(),
            vec![server("a", "one", &[]), server("b", "two", &["x"])]
        );
    }

    #[test]
    fn no_flag_means_no_servers() {
        assert!(parse(&[]).unwrap().is_empty());
    }

    #[test]
    fn a_value_without_equals_is_a_usage_error() {
        assert_eq!(
            parse(&["velum"]).unwrap_err().kind(),
            ErrorKind::ValueValidation
        );
    }

    #[test]
    fn an_empty_name_or_command_is_a_usage_error() {
        for value in ["=/bin/velum", "velum=", "velum=,mcp"] {
            assert_eq!(
                parse(&[value]).unwrap_err().kind(),
                ErrorKind::ValueValidation,
                "{value}"
            );
        }
    }

    #[test]
    fn a_name_with_other_characters_is_a_usage_error() {
        for value in ["ve lum=/bin/velum", "velum.x=/bin/velum", "velum/x=cmd"] {
            assert_eq!(
                parse(&[value]).unwrap_err().kind(),
                ErrorKind::ValueValidation,
                "{value}"
            );
        }
    }
}
