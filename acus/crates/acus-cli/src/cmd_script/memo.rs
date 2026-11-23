// SPDX-License-Identifier: MPL-2.0
//! Remembers the results of a script's read-only calls, so the same call made twice runs once.

use acus_script::HostError;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Mutex;
#[cfg(test)]
use std::sync::atomic::{AtomicU64, Ordering};

/// Results by call name and serialized arguments. Two identical calls in flight may both run;
/// the later result replaces the earlier, and both are the same.
#[derive(Default)]
pub(super) struct Memo {
    results: Mutex<HashMap<String, Value>>,
    /// Calls that ran instead of being answered from the memo.
    #[cfg(test)]
    ran: AtomicU64,
}

impl Memo {
    /// Returns the remembered result of the call, or runs it and remembers what it returns.
    /// A failed call is not remembered.
    pub(super) fn get_or_run(
        &self,
        name: &str,
        args: &[Value],
        run: impl FnOnce() -> Result<Value, HostError>,
    ) -> Result<Value, HostError> {
        let key = format!("{name}\u{0}{}", Value::Array(args.to_vec()));
        if let Some(found) = self.lock().get(&key) {
            return Ok(found.clone());
        }
        #[cfg(test)]
        self.ran.fetch_add(1, Ordering::Relaxed);
        let value = run()?;
        self.lock().insert(key, value.clone());
        Ok(value)
    }

    /// Forgets every result: a write may have changed what the calls would return.
    pub(super) fn clear(&self) {
        self.lock().clear();
    }

    /// Returns how many calls ran instead of being answered from the memo.
    #[cfg(test)]
    pub(super) fn ran(&self) -> u64 {
        self.ran.load(Ordering::Relaxed)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Value>> {
        self.results
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests {
    use super::Memo;
    use acus_script::HostError;
    use serde_json::json;

    #[test]
    fn an_identical_call_is_answered_from_the_memo_until_cleared() {
        let memo = Memo::default();
        let args = [json!("fn main"), json!({"glob": "*.rs"})];
        for _ in 0..2 {
            let got = memo.get_or_run("find", &args, || Ok(json!(7))).unwrap();
            assert_eq!(got, json!(7));
        }
        assert_eq!(memo.ran(), 1);
        memo.get_or_run("find", &[json!("other")], || Ok(json!(8)))
            .unwrap();
        memo.get_or_run("show", &args, || Ok(json!(9))).unwrap();
        assert_eq!(memo.ran(), 3);
        memo.clear();
        memo.get_or_run("find", &args, || Ok(json!(7))).unwrap();
        assert_eq!(memo.ran(), 4);
    }

    #[test]
    fn a_failed_call_is_not_remembered() {
        let memo = Memo::default();
        let fail = || Err(HostError::new("no"));
        assert!(memo.get_or_run("read", &[json!("a")], fail).is_err());
        assert_eq!(
            memo.get_or_run("read", &[json!("a")], || Ok(json!("ok")))
                .unwrap(),
            json!("ok")
        );
        assert_eq!(memo.ran(), 2);
    }
}
