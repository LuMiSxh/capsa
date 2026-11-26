// SPDX-License-Identifier: MPL-2.0
//! `vicarius hook`: the Claude Code hook. It dispatches on the input's `hook_event_name` and, on
//! `SessionStart`, prints the DeepSeek agent context line when the gate is open and an agent file
//! is ready. Every other event, every invalid input and every failure print nothing, and it always
//! exits 0.

use std::io::{IsTerminal, Read, Write};
use std::thread;
use std::time::{Duration, Instant};

use capsa_core::exit::Outcome;
use serde::Deserialize;
use serde_json::json;
use vicarius::config::Config;
use vicarius::gate::Closed;

use crate::status_client::{self, ServiceStatus};

/// The time the hook waits for the service to accept the connection and answer.
const TIMEOUT: Duration = Duration::from_millis(50);
/// The time between two status reads while the service is still starting its first sync.
const POLL: Duration = Duration::from_millis(100);
/// The total time the hook waits for a starting service before it stays silent.
const DEADLINE: Duration = Duration::from_secs(2);
/// The most hook input read; a larger one is not a hook input.
const MAX_INPUT: u64 = 1 << 20;

/// The fields of the hook input that vicarius reads.
#[derive(Deserialize)]
struct Input {
    hook_event_name: Option<String>,
}

/// Runs the hook. Nothing it finds is an error, so the result is always `Found`.
pub fn run() -> Outcome {
    if let Some(line) = read_input().as_deref().and_then(context_line) {
        let out = json!({
            "hookSpecificOutput": {"hookEventName": "SessionStart", "additionalContext": line}
        });
        let _ = writeln!(std::io::stdout(), "{out}");
    }
    Outcome::Found
}

/// Reads stdin up to the cap; `None` for a terminal, a larger input or text that is not UTF-8.
fn read_input() -> Option<String> {
    let stdin = std::io::stdin();
    if stdin.is_terminal() {
        return None;
    }
    let mut buf = Vec::new();
    stdin
        .lock()
        .take(MAX_INPUT + 1)
        .read_to_end(&mut buf)
        .ok()?;
    if buf.len() as u64 > MAX_INPUT {
        return None;
    }
    String::from_utf8(buf).ok()
}

/// Returns the time the hook waits for one request. Debug builds, which the tests use, read
/// `VICARIUS_HOOK_TIMEOUT_MS` so a loaded run can wait for a healthy answer; a release build has no
/// override and keeps `TIMEOUT`.
fn request_timeout() -> Duration {
    #[cfg(debug_assertions)]
    if let Some(ms) = std::env::var("VICARIUS_HOOK_TIMEOUT_MS")
        .ok()
        .and_then(|ms| ms.parse::<u64>().ok())
    {
        return Duration::from_millis(ms);
    }
    TIMEOUT
}

/// Returns the context line for a `SessionStart` input; `None` for another event or any failure.
///
/// A service that answers but has not finished its first sync is polled until it has, or until
/// `DEADLINE` passes.
fn context_line(input: &str) -> Option<String> {
    let input: Input = serde_json::from_str(input).ok()?;
    if input.hook_event_name.as_deref() != Some("SessionStart") {
        return None;
    }
    let config = Config::load_from(&Config::path()).ok()?;
    let clock = SystemClock(Instant::now());
    let status = settled_status(
        || status_client::fetch(config.port, request_timeout()).map(|fetched| fetched.status),
        &clock,
        POLL,
        DEADLINE,
    )?;
    agents_line(&config, &status)
}

/// Reads the status until the service is no longer starting. Gives up with `None` once `deadline`
/// has passed on `clock`, and at once when the service does not answer.
fn settled_status(
    mut read: impl FnMut() -> Option<ServiceStatus>,
    clock: &impl Clock,
    poll: Duration,
    deadline: Duration,
) -> Option<ServiceStatus> {
    loop {
        let status = read()?;
        if !is_starting(&status) {
            return Some(status);
        }
        let elapsed = clock.elapsed();
        if elapsed >= deadline {
            return None;
        }
        clock.pause(poll.min(deadline - elapsed));
    }
}

/// Returns whether the service has yet to read its balance or sync its agent files. A gate closed
/// for any other reason does not change while the hook waits, so it is not starting.
fn is_starting(status: &ServiceStatus) -> bool {
    match status.gate.reason.as_deref() {
        Some(reason) => reason == Closed::NotChecked.code(),
        None => status.gate.open && status.agents.is_none(),
    }
}

/// Measures the time a wait has taken and pauses it.
trait Clock {
    /// Returns the time since the wait began.
    fn elapsed(&self) -> Duration;
    /// Waits for `span`.
    fn pause(&self, span: Duration);
}

/// Measures the real time since the hook started and sleeps on it.
struct SystemClock(Instant);

impl Clock for SystemClock {
    fn elapsed(&self) -> Duration {
        self.0.elapsed()
    }

    fn pause(&self, span: Duration) {
        thread::sleep(span);
    }
}

/// Builds the context line from the status and the configured models, or `None` when the gate is
/// closed or no agent file is ready.
fn agents_line(config: &Config, status: &ServiceStatus) -> Option<String> {
    if !status.gate.open {
        return None;
    }
    let entries: Vec<String> = status
        .agents
        .as_ref()?
        .entries
        .iter()
        .filter(|entry| matches!(entry.outcome_name(), "written" | "unchanged"))
        .filter_map(|entry| {
            let agent = &entry.agent;
            let model = &config
                .agents
                .iter()
                .find(|a| &a.from == agent && !a.suspended)?
                .model;
            Some(format!("{agent}-ds ({model}, variant of {agent})"))
        })
        .collect();
    if entries.is_empty() {
        return None;
    }
    let balance = status.gate.balance.map_or_else(String::new, |amount| {
        let money = status_client::money(amount, status.gate.currency.as_deref());
        format!(" (balance {money})")
    });
    Some(format!(
        "vicarius: DeepSeek agents available{balance}: {}. Use a healthy mix with their Claude source agents: a -ds agent for well-scoped tasks with a clear check, the Claude agent for work that needs judgement or after a -ds result needed rework; report the DeepSeek cost afterwards (vicarius status) and do not pass a model parameter when spawning one.",
        entries.join(", ")
    ))
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::time::Duration;

    use serde_json::json;
    use vicarius::config::{Agent, Config, Effort};

    use super::{Clock, DEADLINE, POLL, agents_line, settled_status};
    use crate::status_client::{AgentStatus, AgentsStatus, GateStatus, ServiceStatus};

    fn agent(from: &str, suspended: bool) -> Agent {
        Agent {
            from: from.to_owned(),
            model: "deepseek-flash".to_owned(),
            effort: Effort::Keep,
            suspended,
        }
    }

    fn written(agent: &str) -> AgentStatus {
        AgentStatus {
            agent: agent.to_owned(),
            file: format!("{agent}-ds.md"),
            outcome: json!("written"),
        }
    }

    #[test]
    fn a_suspended_agent_is_not_advertised_from_a_stale_report() {
        let config = Config {
            agents: vec![agent("worker", false), agent("reviewer", true)],
            ..Config::default()
        };
        let status = ServiceStatus {
            gate: GateStatus {
                open: true,
                ..GateStatus::default()
            },
            agents: Some(AgentsStatus {
                entries: vec![written("worker"), written("reviewer")],
            }),
            ..ServiceStatus::default()
        };

        let line = agents_line(&config, &status).unwrap();

        assert!(line.contains("worker-ds (deepseek-flash, variant of worker)"));
        assert!(!line.contains("reviewer-ds"));
    }

    /// Moves only when a wait pauses on it, so the tests take no wall-clock time.
    #[derive(Default)]
    struct FakeClock(Cell<Duration>);

    impl Clock for FakeClock {
        fn elapsed(&self) -> Duration {
            self.0.get()
        }

        fn pause(&self, span: Duration) {
            self.0.set(self.0.get() + span);
        }
    }

    fn senior_config() -> Config {
        Config {
            agents: vec![agent("senior-worker", false)],
            ..Config::default()
        }
    }

    fn starting() -> ServiceStatus {
        closed_for("not_checked")
    }

    fn closed_for(code: &str) -> ServiceStatus {
        ServiceStatus {
            gate: GateStatus {
                reason: Some(code.to_owned()),
                ..GateStatus::default()
            },
            ..ServiceStatus::default()
        }
    }

    fn open_without_agents() -> ServiceStatus {
        ServiceStatus {
            gate: GateStatus {
                open: true,
                ..GateStatus::default()
            },
            ..ServiceStatus::default()
        }
    }

    fn settled() -> ServiceStatus {
        ServiceStatus {
            agents: Some(AgentsStatus {
                entries: vec![written("senior-worker")],
            }),
            ..open_without_agents()
        }
    }

    #[test]
    fn waits_for_the_first_sync_before_printing_the_line() {
        let reads = Cell::new(0);
        let clock = FakeClock::default();
        let status = settled_status(
            || {
                reads.set(reads.get() + 1);
                Some(if reads.get() < 4 {
                    starting()
                } else {
                    settled()
                })
            },
            &clock,
            POLL,
            DEADLINE,
        );

        let line = status.and_then(|status| agents_line(&senior_config(), &status));

        assert!(line.is_some_and(|line| {
            line.contains("senior-worker-ds (deepseek-flash, variant of senior-worker)")
        }));
        assert_eq!(reads.get(), 4);
        assert_eq!(clock.elapsed(), POLL * 3);
    }

    #[test]
    fn waits_while_the_gate_is_open_and_the_first_sync_is_pending() {
        let reads = Cell::new(0);
        let clock = FakeClock::default();
        let status = settled_status(
            || {
                reads.set(reads.get() + 1);
                Some(if reads.get() < 3 {
                    open_without_agents()
                } else {
                    settled()
                })
            },
            &clock,
            POLL,
            DEADLINE,
        );

        assert!(status.is_some_and(|status| status.agents.is_some()));
        assert_eq!(reads.get(), 3);
    }

    #[test]
    fn stays_silent_immediately_when_the_service_does_not_answer() {
        let reads = Cell::new(0);
        let clock = FakeClock::default();
        let status = settled_status(
            || {
                reads.set(reads.get() + 1);
                None
            },
            &clock,
            POLL,
            DEADLINE,
        );

        assert!(status.is_none());
        assert_eq!(reads.get(), 1);
        assert_eq!(clock.elapsed(), Duration::ZERO);
    }

    #[test]
    fn stays_silent_immediately_when_the_gate_is_closed() {
        for code in [
            "no_key",
            "payment_required",
            "key_rejected",
            "low_balance",
            "unavailable",
        ] {
            let reads = Cell::new(0);
            let clock = FakeClock::default();
            let status = settled_status(
                || {
                    reads.set(reads.get() + 1);
                    Some(closed_for(code))
                },
                &clock,
                POLL,
                DEADLINE,
            );

            let line = status.and_then(|status| agents_line(&senior_config(), &status));

            assert!(line.is_none(), "{code}");
            assert_eq!(reads.get(), 1, "{code}");
            assert_eq!(clock.elapsed(), Duration::ZERO, "{code}");
        }
    }

    #[test]
    fn gives_up_silently_after_the_deadline_when_sync_never_finishes() {
        let reads = Cell::new(0);
        let clock = FakeClock::default();
        let status = settled_status(
            || {
                reads.set(reads.get() + 1);
                Some(starting())
            },
            &clock,
            POLL,
            DEADLINE,
        );

        assert!(status.is_none());
        assert_eq!(clock.elapsed(), DEADLINE);
        // One read at each poll from the start of the wait to the deadline.
        assert_eq!(reads.get(), 21);
    }
}
