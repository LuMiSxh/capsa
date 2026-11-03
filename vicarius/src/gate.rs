// SPDX-License-Identifier: MPL-2.0
//! Holds whether the DeepSeek route is open, shared by the proxy and the balance refresh.

use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{PoisonError, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};

/// Names why the DeepSeek route is closed.
#[derive(Clone, Debug, PartialEq)]
pub enum Closed {
    /// No DeepSeek API key is configured.
    NoKey,
    /// The balance is below the minimum, in USD.
    LowBalance { minimum: f64 },
    /// DeepSeek answered 402.
    PaymentRequired,
    /// DeepSeek answered 401.
    KeyRejected,
    /// DeepSeek reports the account as unusable.
    Unavailable,
    /// The balance has not been read yet.
    NotChecked,
}

impl Closed {
    /// Returns the stable identifier of the reason, used in the status JSON.
    pub fn code(&self) -> &'static str {
        match self {
            Closed::NoKey => "no_key",
            Closed::LowBalance { .. } => "low_balance",
            Closed::PaymentRequired => "payment_required",
            Closed::KeyRejected => "key_rejected",
            Closed::Unavailable => "unavailable",
            Closed::NotChecked => "not_checked",
        }
    }
}

impl fmt::Display for Closed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Closed::NoKey => f.write_str("no API key configured"),
            Closed::LowBalance { minimum } => write!(f, "balance below ${minimum}"),
            Closed::PaymentRequired => f.write_str("payment required"),
            Closed::KeyRejected => f.write_str("key rejected"),
            Closed::Unavailable => f.write_str("account has no usable balance"),
            Closed::NotChecked => f.write_str("balance not checked yet"),
        }
    }
}

/// A copy of the gate at one moment.
#[derive(Clone, Debug, PartialEq)]
pub struct Snapshot {
    /// `None` when the route is open.
    pub closed: Option<Closed>,
    /// The last known balance.
    pub balance: Option<f64>,
    /// The currency of `balance`, such as `USD`.
    pub currency: Option<String>,
    /// Unix seconds of the last change made by the refresh or by the proxy.
    pub checked_at: Option<u64>,
}

impl Snapshot {
    /// Returns whether the DeepSeek route accepts requests.
    pub fn is_open(&self) -> bool {
        self.closed.is_none()
    }
}

/// Shares the state of the DeepSeek route between threads. Readers take a short read lock.
#[derive(Debug)]
pub struct Gate {
    state: RwLock<Snapshot>,
    /// Counts the closes for a payment or key reason; written only under the write lock.
    generation: AtomicU64,
}

impl Gate {
    /// Creates a gate that is closed: with `NoKey` when there is no key, else `NotChecked`.
    pub fn new(has_key: bool) -> Self {
        let closed = if has_key {
            Closed::NotChecked
        } else {
            Closed::NoKey
        };
        Gate {
            state: RwLock::new(Snapshot {
                closed: Some(closed),
                balance: None,
                currency: None,
                checked_at: None,
            }),
            generation: AtomicU64::new(0),
        }
    }

    /// Returns the number of closes for a payment or key reason so far.
    ///
    /// A balance read that starts now passes it to [`Gate::open_if_unchanged`].
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }

    /// Opens the route and records the balance read at `at`, in Unix seconds.
    pub fn open(&self, amount: f64, currency: &str, at: u64) {
        let mut state = self.write();
        Self::set_open(&mut state, amount, currency, at);
    }

    /// Opens the route like [`Gate::open`] unless a payment or key close happened since
    /// `generation` was read. Then it records the balance only. Returns whether it opened.
    pub fn open_if_unchanged(&self, generation: u64, amount: f64, currency: &str, at: u64) -> bool {
        let mut state = self.write();
        if self.generation() == generation {
            Self::set_open(&mut state, amount, currency, at);
            true
        } else {
            state.balance = Some(amount);
            state.currency = Some(currency.to_owned());
            false
        }
    }

    fn set_open(state: &mut Snapshot, amount: f64, currency: &str, at: u64) {
        state.closed = None;
        state.balance = Some(amount);
        state.currency = Some(currency.to_owned());
        state.checked_at = Some(at);
    }

    /// Closes the route for `reason` at `at`. The last known balance stays.
    pub fn close(&self, reason: Closed, at: u64) {
        let mut state = self.write();
        if matches!(reason, Closed::PaymentRequired | Closed::KeyRejected) {
            self.generation.fetch_add(1, Ordering::Release);
        }
        state.closed = Some(reason);
        state.checked_at = Some(at);
    }

    /// Records a balance read at `at` without changing whether the route is open.
    pub fn note_balance(&self, amount: f64, currency: &str, at: u64) {
        let mut state = self.write();
        state.balance = Some(amount);
        state.currency = Some(currency.to_owned());
        state.checked_at = Some(at);
    }

    /// Returns the reason the route is closed, or `None` when it is open.
    pub fn closed(&self) -> Option<Closed> {
        self.state
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .closed
            .clone()
    }

    /// Returns a copy of the whole state.
    pub fn snapshot(&self) -> Snapshot {
        self.state
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn write(&self) -> std::sync::RwLockWriteGuard<'_, Snapshot> {
        self.state.write().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Returns the current time in Unix seconds.
pub fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starts_closed_as_not_checked_or_no_key() {
        assert_eq!(Gate::new(true).closed(), Some(Closed::NotChecked));
        assert_eq!(Gate::new(false).closed(), Some(Closed::NoKey));
    }

    #[test]
    fn open_records_the_balance_and_close_keeps_it() {
        let gate = Gate::new(true);
        gate.open(4.5, "USD", 10);
        let open = gate.snapshot();
        assert!(open.is_open());
        assert_eq!((open.balance, open.checked_at), (Some(4.5), Some(10)));
        gate.close(Closed::PaymentRequired, 20);
        let closed = gate.snapshot();
        assert_eq!(closed.closed, Some(Closed::PaymentRequired));
        assert_eq!((closed.balance, closed.checked_at), (Some(4.5), Some(20)));
    }

    #[test]
    fn a_payment_close_after_the_read_keeps_a_stale_open_out() {
        let gate = Gate::new(true);
        let seen = gate.generation();
        gate.close(Closed::PaymentRequired, 5);
        assert!(!gate.open_if_unchanged(seen, 4.0, "USD", 6));
        let snap = gate.snapshot();
        assert_eq!(snap.closed, Some(Closed::PaymentRequired));
        assert_eq!((snap.balance, snap.checked_at), (Some(4.0), Some(5)));
        assert!(gate.open_if_unchanged(gate.generation(), 4.0, "USD", 7));
        assert!(gate.snapshot().is_open());
    }

    #[test]
    fn closes_for_other_reasons_keep_the_generation() {
        let gate = Gate::new(true);
        let seen = gate.generation();
        gate.close(Closed::LowBalance { minimum: 1.0 }, 5);
        assert!(gate.open_if_unchanged(seen, 4.0, "USD", 6));
    }

    #[test]
    fn reasons_read_as_the_refusal_message_needs() {
        assert_eq!(
            Closed::LowBalance { minimum: 1.0 }.to_string(),
            "balance below $1"
        );
        assert_eq!(Closed::NoKey.to_string(), "no API key configured");
        assert_eq!(Closed::KeyRejected.to_string(), "key rejected");
    }
}
