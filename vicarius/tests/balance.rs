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

#[test]
fn parses_the_documented_shape() {
    let parsed = parse(DOCUMENTED.as_bytes()).unwrap();
    assert_eq!(parsed, balance(true, &[("USD", 20.0)]));
}

#[test]
fn parses_a_cny_only_balance() {
    let body = answer(true, r#"{"currency": "CNY", "total_balance": "150.50"}"#);
    assert_eq!(
        parse(body.as_bytes()).unwrap(),
        balance(true, &[("CNY", 150.5)])
    );
}

#[test]
fn keeps_is_available_false_from_the_answer() {
    let body = answer(false, r#"{"currency": "USD", "total_balance": "0.00"}"#);
    assert_eq!(
        parse(body.as_bytes()).unwrap(),
        balance(false, &[("USD", 0.0)])
    );
}

#[test]
fn an_empty_list_of_balances_parses_to_no_totals() {
    let body = answer(true, "");
    assert_eq!(parse(body.as_bytes()).unwrap(), balance(true, &[]));
}

#[test]
fn a_non_numeric_amount_is_an_error() {
    let body = answer(true, r#"{"currency": "USD", "total_balance": "abc"}"#);
    assert!(matches!(
        parse(body.as_bytes()),
        Err(BalanceError::Amount { .. })
    ));
}

#[test]
fn a_non_finite_amount_is_an_error() {
    for value in ["inf", "NaN"] {
        let body = answer(
            true,
            &format!(r#"{{"currency": "USD", "total_balance": "{value}"}}"#),
        );
        assert!(matches!(
            parse(body.as_bytes()),
            Err(BalanceError::Amount { .. })
        ));
    }
}

#[test]
fn an_unavailable_account_is_unavailable_whatever_its_balance() {
    let decided = decide(&balance(false, &[("USD", 20.0)]), 1.0);
    assert_eq!(decided, Availability::Unavailable);
}

#[test]
fn a_usd_balance_at_the_minimum_is_available() {
    let decided = decide(&balance(true, &[("USD", 1.0)]), 1.0);
    assert_eq!(
        decided,
        Availability::Available {
            amount: 1.0,
            currency: "USD".to_owned(),
        }
    );
}

#[test]
fn a_usd_balance_below_the_minimum_is_low() {
    let decided = decide(&balance(true, &[("USD", 0.99)]), 1.0);
    assert_eq!(
        decided,
        Availability::Low {
            amount: 0.99,
            currency: "USD".to_owned(),
            min: 1.0,
        }
    );
}

#[test]
fn a_cny_only_balance_is_available_in_cny() {
    let decided = decide(&balance(true, &[("CNY", 150.5)]), 1.0);
    assert_eq!(
        decided,
        Availability::Available {
            amount: 150.5,
            currency: "CNY".to_owned(),
        }
    );
}

#[test]
fn a_zero_cny_balance_is_unavailable() {
    let decided = decide(&balance(true, &[("CNY", 0.0)]), 1.0);
    assert_eq!(decided, Availability::Unavailable);
}

#[test]
fn no_totals_is_unavailable() {
    assert_eq!(decide(&balance(true, &[]), 1.0), Availability::Unavailable);
}
