// SPDX-License-Identifier: MPL-2.0
//! Reads the DeepSeek balance answer and decides whether DeepSeek is available.

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// The DeepSeek balance as the `/user/balance` answer gives it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Balance {
    /// Whether DeepSeek accepts requests for this account.
    pub is_available: bool,
    /// One total per currency the account holds, in the order DeepSeek lists them.
    pub totals: Vec<Total>,
}

/// The total balance in one currency.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Total {
    /// The currency code, `USD` or `CNY`.
    pub currency: String,
    /// The total in that currency.
    pub amount: f64,
}

/// Why a balance answer cannot be read.
#[derive(Debug, Error)]
pub enum BalanceError {
    /// The body is not the balance JSON.
    #[error("the balance answer is not the expected JSON: {0}")]
    Json(#[from] serde_json::Error),
    /// A total is not a finite decimal number.
    #[error("the {currency} balance `{value}` is not a finite amount")]
    Amount { currency: String, value: String },
}

/// Whether DeepSeek can take work now, and with which balance.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Availability {
    /// DeepSeek refuses requests, or the account holds nothing usable.
    Unavailable,
    /// DeepSeek can take work with this balance.
    Available { amount: f64, currency: String },
    /// The USD balance is below the minimum that keeps the derived agents.
    Low {
        amount: f64,
        currency: String,
        min: f64,
    },
}

#[derive(Deserialize)]
struct Answer {
    is_available: bool,
    #[serde(default)]
    balance_infos: Vec<Info>,
}

#[derive(Deserialize)]
struct Info {
    currency: String,
    total_balance: String,
}

/// Parses the body of a `GET /user/balance` answer.
///
/// # Errors
/// Fails when the body is not the balance JSON, or when a total is not a finite number.
pub fn parse(body: &[u8]) -> Result<Balance, BalanceError> {
    let answer: Answer = serde_json::from_slice(body)?;
    let totals = answer
        .balance_infos
        .into_iter()
        .map(total)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Balance {
        is_available: answer.is_available,
        totals,
    })
}

fn total(info: Info) -> Result<Total, BalanceError> {
    match info.total_balance.parse::<f64>() {
        Ok(amount) if amount.is_finite() => Ok(Total {
            currency: info.currency,
            amount,
        }),
        _ => Err(BalanceError::Amount {
            currency: info.currency,
            value: info.total_balance,
        }),
    }
}

/// Decides whether DeepSeek is available for `balance`, against a USD minimum `min_usd`.
///
/// A USD total is compared with `min_usd`. An account without a USD total but with a total in
/// another currency is available when that total is above zero, and reports its own currency.
/// No exchange rate is applied, so `min_usd` does not apply to that case.
pub fn decide(balance: &Balance, min_usd: f64) -> Availability {
    if !balance.is_available {
        return Availability::Unavailable;
    }
    match balance.totals.iter().find(|total| total.currency == "USD") {
        Some(usd) if usd.amount >= min_usd => Availability::Available {
            amount: usd.amount,
            currency: usd.currency.clone(),
        },
        Some(usd) => Availability::Low {
            amount: usd.amount,
            currency: usd.currency.clone(),
            min: min_usd,
        },
        None => balance
            .totals
            .iter()
            .find(|total| total.amount > 0.0)
            .map_or(Availability::Unavailable, |total| Availability::Available {
                amount: total.amount,
                currency: total.currency.clone(),
            }),
    }
}
