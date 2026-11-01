// SPDX-License-Identifier: MPL-2.0
//! Sums token classes over raw rows and hourly aggregates.

use super::Ledger;
use super::error::LedgerError;
use super::model::{Filter, Tokens, family_of};

impl Ledger {
    /// Sums tokens over the requests the filter keeps, from the hourly aggregates (by hour start)
    /// and the raw rows (by request time). The two never overlap.
    ///
    /// # Errors
    /// Fails when a state file cannot be read.
    pub fn tokens(&self, filter: &Filter) -> Result<Tokens, LedgerError> {
        let mut sum = Tokens::default();
        for h in self.hourly()? {
            if filter.keeps(h.hour, h.sub, &h.family) {
                sum.add(&h.tokens);
            }
        }
        for r in self.rows()? {
            if filter.keeps(r.t, r.agent.is_some(), family_of(&r.model)) {
                sum.add_row(r);
            }
        }
        Ok(sum)
    }
}
