// SPDX-License-Identifier: MPL-2.0
//! Consecutive-failure bookkeeping for the message rules, and the switch to passthrough.
//! The engine forwards the original line of each failed message.

use std::any::Any;
use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::panic::{AssertUnwindSafe, catch_unwind};

/// What the engine should do after a failure.
#[derive(Debug, PartialEq, Eq)]
pub struct Failure {
    /// Consecutive failures of this session (or of the proxy) so far.
    pub count: u32,
    /// The scope was just switched to pure passthrough.
    pub switched: bool,
}

#[derive(Debug)]
pub struct FailSafe {
    /// Consecutive failures that switch to passthrough; 0 never switches.
    max: u32,
    /// Consecutive failures per session; `None` is messages without one.
    consecutive: HashMap<Option<String>, u32>,
    passthrough: HashSet<String>,
    all: bool,
}

impl FailSafe {
    /// Forgets the failures and passthrough state of a closed session.
    pub fn forget(&mut self, session: &str) {
        self.consecutive.remove(&Some(session.to_owned()));
        self.passthrough.remove(session);
    }

    pub fn new(max: u32) -> Self {
        Self {
            max,
            consecutive: HashMap::new(),
            passthrough: HashSet::new(),
            all: false,
        }
    }

    /// Tells whether any scope is in passthrough, so callers can skip the work of
    /// finding a message's session when nothing is switched.
    pub fn any_passthrough(&self) -> bool {
        self.all || !self.passthrough.is_empty()
    }

    pub fn is_passthrough(&self, session: Option<&str>) -> bool {
        self.all || session.is_some_and(|s| self.passthrough.contains(s))
    }

    /// Tells whether some scope has failures that a success would reset.
    pub fn has_failures(&self) -> bool {
        !self.consecutive.is_empty()
    }

    pub fn success(&mut self, session: Option<&str>) {
        self.consecutive.remove(&session.map(str::to_owned));
    }

    pub fn failure(&mut self, session: Option<&str>) -> Failure {
        let count = self
            .consecutive
            .entry(session.map(str::to_owned))
            .or_insert(0);
        *count += 1;
        let count = *count;
        let switched = self.max > 0 && count >= self.max && !self.is_passthrough(session);
        if switched {
            match session {
                Some(s) => {
                    self.passthrough.insert(s.to_owned());
                }
                None => self.all = true,
            }
        }
        Failure { count, switched }
    }
}

thread_local! {
    /// Set while a rule runs under [`catch_panic`]: the panic hook leaves
    /// those panics to the fail-safe's own note.
    static CATCHING: Cell<bool> = const { Cell::new(false) };
}

/// Runs the rules under `catch_unwind`. [`is_caught`] is true while the panic hook
/// runs for a panic that is caught here.
pub fn catch_panic<R>(f: impl FnOnce() -> R) -> std::thread::Result<R> {
    CATCHING.set(true);
    let outcome = catch_unwind(AssertUnwindSafe(f));
    CATCHING.set(false);
    outcome
}

/// Tells whether the panic now starting is one [`catch_panic`] catches.
pub fn is_caught() -> bool {
    CATCHING.get()
}

/// Returns the text of a caught panic.
pub fn panic_text(payload: &(dyn Any + Send)) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|s| (*s).to_owned())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "unknown panic".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn switches_a_session_after_max_consecutive_failures() {
        let mut g = FailSafe::new(3);
        assert!(!g.failure(Some("s1")).switched);
        assert!(!g.failure(Some("s1")).switched);
        let f = g.failure(Some("s1"));
        assert!(f.switched && f.count == 3);
        assert!(g.is_passthrough(Some("s1")));
        assert!(!g.is_passthrough(Some("s2")));
        assert!(!g.is_passthrough(None));
    }

    #[test]
    fn a_success_resets_the_count() {
        let mut g = FailSafe::new(2);
        g.failure(Some("s1"));
        g.success(Some("s1"));
        assert!(!g.failure(Some("s1")).switched);
        assert!(!g.is_passthrough(Some("s1")));
    }

    #[test]
    fn sessionless_failures_switch_everything() {
        let mut g = FailSafe::new(2);
        g.failure(None);
        assert!(g.failure(None).switched);
        assert!(g.is_passthrough(Some("any")));
    }

    #[test]
    fn zero_never_switches() {
        let mut g = FailSafe::new(0);
        for _ in 0..50 {
            assert!(!g.failure(Some("s1")).switched);
        }
        assert!(!g.any_passthrough());
    }

    #[test]
    fn panic_text_reads_str_and_string_payloads() {
        let a = std::panic::catch_unwind(|| panic!("plain")).unwrap_err();
        assert_eq!(panic_text(a.as_ref()), "plain");
        let n = 3;
        let b = std::panic::catch_unwind(|| panic!("fmt {n}")).unwrap_err();
        assert_eq!(panic_text(b.as_ref()), "fmt 3");
    }
}
