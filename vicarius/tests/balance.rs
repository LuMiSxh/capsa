// SPDX-License-Identifier: MPL-2.0

use vicarius::balance::{Availability, Balance, BalanceError, Total, decide, parse};

const DOCUMENTED: &str = r#"{"is_available": true, "balance_infos": [{"currency": "USD", "total_balance": "20.00", "granted_balance": "0.00", "topped_up_balance": "20.00"}]}"#;

fn balance(is_available: bool, totals: &[(&str, f64)]) -> Balance {
    Balance {
        is_available,
        totals: totals
            .iter()
            .map(|&(currency, amount)| Total {
                currency: currency.to_owned(),
                amount,
            })
            .collect(),
    }
}

fn answer(is_available: bool, infos: &str) -> String {
    format!(r#"{{"is_available": {is_available}, "balance_infos": [{infos}]}}"#)
}

type DecideCase<'a> = (&'a str, bool, &'a [(&'a str, f64)], Availability);

#[test]
fn parses_every_answer_shape() {
    let cases = [
        (
            "the documented shape",
            DOCUMENTED.to_owned(),
            balance(true, &[("USD", 20.0)]),
        ),
        (
            "a CNY only balance",
            answer(true, r#"{"currency": "CNY", "total_balance": "150.50"}"#),
            balance(true, &[("CNY", 150.5)]),
        ),
        (
            "is_available false",
            answer(false, r#"{"currency": "USD", "total_balance": "0.00"}"#),
            balance(false, &[("USD", 0.0)]),
        ),
        (
            "an empty list of balances",
            answer(true, ""),
            balance(true, &[]),
        ),
    ];
    for (name, body, expected) in cases {
        assert_eq!(parse(body.as_bytes()).unwrap(), expected, "{name}");
    }
}

#[test]
fn a_total_that_is_not_a_finite_number_is_an_error() {
    for value in ["abc", "inf", "NaN"] {
        let body = answer(
            true,
            &format!(r#"{{"currency": "USD", "total_balance": "{value}"}}"#),
        );
        assert!(
            matches!(parse(body.as_bytes()), Err(BalanceError::Amount { .. })),
            "{value}"
        );
    }
}

#[test]
fn decides_availability_from_the_totals() {
    let cases: [DecideCase; 6] = [
        (
            "an unavailable account is unavailable whatever its balance",
            false,
            &[("USD", 20.0)],
            Availability::Unavailable,
        ),
        (
            "a USD balance at the minimum is available",
            true,
            &[("USD", 1.0)],
            Availability::Available {
                amount: 1.0,
                currency: "USD".to_owned(),
            },
        ),
        (
            "a USD balance below the minimum is low",
            true,
            &[("USD", 0.99)],
            Availability::Low {
                amount: 0.99,
                currency: "USD".to_owned(),
                min: 1.0,
            },
        ),
        (
            "a CNY only balance is available in CNY",
            true,
            &[("CNY", 150.5)],
            Availability::Available {
                amount: 150.5,
                currency: "CNY".to_owned(),
            },
        ),
        (
            "a zero CNY balance is unavailable",
            true,
            &[("CNY", 0.0)],
            Availability::Unavailable,
        ),
        (
            "no totals is unavailable",
            true,
            &[],
            Availability::Unavailable,
        ),
    ];
    for (name, is_available, totals, expected) in cases {
        assert_eq!(
            decide(&balance(is_available, totals), 1.0),
            expected,
            "{name}"
        );
    }
}
