//! Rendering run files in the layout of spec section 8.

use jiff::Timestamp;
use rust_decimal::Decimal;

use super::plan::{BalanceOutcome, PlannedTxn};
use crate::ledger::{META_COUNTERPARTY, META_ID, META_PURPOSE, META_REF, META_TYPE};
use crate::status::UNCHECKED_MARK;

/// `imports/<source-id>/<YYYY>/<YYYYMMDDTHHMMSSZ>.beancount` for a run started at `started` (UTC).
#[must_use]
pub fn run_path(source_id: &str, started: Timestamp) -> String {
    format!(
        "imports/{source_id}/{}/{}.beancount",
        started.strftime("%Y"),
        started.strftime("%Y%m%dT%H%M%SZ")
    )
}

/// The run file content: header comments, transactions with flag `!`, and the
/// balance assertion with `~ 0.00` or the `; lentil: unchecked` comment.
///
/// Transactions are written in the given order; the planner puts the opening
/// balance first. Source amounts are aligned in one column per file: they
/// start one space after the longest counter account, at least two spaces
/// after the source account, and are right-aligned. Runs without an assertion
/// ([`BalanceOutcome::Unchecked`], [`BalanceOutcome::Suspect`],
/// [`BalanceOutcome::NeedsInput`]) are marked unchecked; with
/// [`BalanceOutcome::AlreadyExists`] the existing assertion covers the run.
#[must_use]
pub fn render_run(
    source_id: &str,
    account: &str,
    started: Timestamp,
    client: &str,
    transactions: &[PlannedTxn],
    balance: &BalanceOutcome,
) -> String {
    let counter_width = transactions
        .iter()
        .map(|t| t.counter_account.chars().count())
        .max()
        .unwrap_or(0);
    let account_column = (counter_width + 1).max(account.chars().count() + 2);
    let number_width = transactions
        .iter()
        .map(|t| number(t.amount).len())
        .max()
        .unwrap_or(0);

    let mut blocks: Vec<String> = transactions
        .iter()
        .map(|t| {
            let mut lines = vec![match &t.payee {
                Some(payee) => format!(
                    "{} ! \"{}\" \"{}\"",
                    t.date,
                    escape(payee),
                    escape(&t.narration)
                ),
                None => format!("{} ! \"{}\"", t.date, escape(&t.narration)),
            }];
            lines.push(format!("  {META_ID}: \"{}\"", escape(&t.id)));
            for (key, value) in [
                (META_REF, &t.reference),
                (META_COUNTERPARTY, &t.counterparty),
                (META_PURPOSE, &t.purpose),
                (META_TYPE, &t.booking_type),
            ] {
                if let Some(value) = value {
                    lines.push(format!("  {key}: \"{}\"", escape(value)));
                }
            }
            lines.push(format!(
                "  {account:<account_column$}{:>number_width$} {}",
                number(t.amount),
                t.currency
            ));
            lines.push(format!("  {}", t.counter_account));
            lines.join("\n")
        })
        .collect();
    match balance {
        BalanceOutcome::Assertion {
            date,
            amount,
            currency,
        } => blocks.push(format!(
            "{date} balance {account}  {} ~ {} {currency}",
            number(*amount),
            zero_tolerance(*amount)
        )),
        BalanceOutcome::NeedsInput | BalanceOutcome::Unchecked | BalanceOutcome::Suspect => {
            blocks.push(UNCHECKED_MARK.to_owned());
        }
        BalanceOutcome::AlreadyExists { .. } => {}
    }

    let mut content = format!(
        "; lentil import run\n; source: {source_id}, started: {}, client: {client}\n",
        started.strftime("%Y-%m-%dT%H:%M:%SZ")
    );
    if !blocks.is_empty() {
        content.push('\n');
        content.push_str(&blocks.join("\n\n"));
        content.push('\n');
    }
    content
}

/// An amount as written: the scale the bank reported, no negative zero.
fn number(amount: Decimal) -> String {
    let mut amount = amount;
    if amount.is_zero() {
        amount.set_sign_positive(true);
    }
    amount.to_string()
}

/// `0.00`, with as many decimal places as `amount` has, at least two.
fn zero_tolerance(amount: Decimal) -> String {
    let places = usize::try_from(amount.scale()).unwrap_or(2).max(2);
    format!("0.{}", "0".repeat(places))
}

/// Escapes a Beancount string literal's content (`\` and `"`).
#[must_use]
pub fn escape(text: &str) -> String {
    text.replace('\\', "\\\\").replace('"', "\\\"")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::import::SuggestionKind;
    use jiff::civil::Date;

    fn started() -> Timestamp {
        "2026-10-20T07:15:02.750Z".parse().unwrap()
    }

    #[allow(clippy::too_many_arguments)]
    fn txn(
        date: Date,
        payee: Option<&str>,
        narration: &str,
        id: &str,
        reference: Option<&str>,
        [counterparty, purpose, booking_type]: [Option<&str>; 3],
        amount: Decimal,
        counter: &str,
    ) -> PlannedTxn {
        PlannedTxn {
            id: id.into(),
            date,
            payee: payee.map(Into::into),
            narration: narration.into(),
            counterparty: counterparty.map(Into::into),
            purpose: purpose.map(Into::into),
            booking_type: booking_type.map(Into::into),
            reference: reference.map(Into::into),
            amount,
            currency: "EUR".into(),
            counter_account: counter.into(),
            suggestion: SuggestionKind::Fallback,
        }
    }

    fn spec_transactions() -> Vec<PlannedTxn> {
        vec![
            txn(
                Date::constant(2026, 10, 18),
                Some("Corner Grocery"),
                "Card payment",
                "giro-3f9a1c2e7b4d",
                None,
                [
                    Some("CORNER GROCERY 4711"),
                    Some("Card payment"),
                    Some("Debit card"),
                ],
                Decimal::new(-4217, 2),
                "Expenses:Food:Groceries",
            ),
            txn(
                Date::constant(2026, 10, 19),
                Some("City Utilities"),
                "Electricity, monthly instalment",
                "giro-8c01d4e9a2f7",
                Some("2026101912345"),
                [
                    Some("CITY UTILITIES"),
                    Some("Electricity, monthly instalment"),
                    Some("Direct debit"),
                ],
                Decimal::new(-6400, 2),
                "Expenses:Uncategorized",
            ),
        ]
    }

    #[test]
    fn run_path_uses_the_utc_start_time() {
        assert_eq!(
            run_path("giro", started()),
            "imports/giro/2026/20261020T071502Z.beancount"
        );
        let new_year: Timestamp = "2026-12-31T23:30:00-01:00".parse().unwrap();
        assert_eq!(
            run_path("card", new_year),
            "imports/card/2027/20270101T003000Z.beancount"
        );
    }

    #[test]
    fn renders_the_spec_example_exactly() {
        let content = render_run(
            "giro",
            "Assets:Bank:Giro",
            started(),
            "lentil-cli/0.1.0",
            &spec_transactions(),
            &BalanceOutcome::Assertion {
                date: Date::constant(2026, 10, 20),
                amount: Decimal::new(123_456, 2),
                currency: "EUR".into(),
            },
        );
        let expected = r#"; lentil import run
; source: giro, started: 2026-10-20T07:15:02Z, client: lentil-cli/0.1.0

2026-10-18 ! "Corner Grocery" "Card payment"
  lentil_id: "giro-3f9a1c2e7b4d"
  lentil_counterparty: "CORNER GROCERY 4711"
  lentil_purpose: "Card payment"
  lentil_type: "Debit card"
  Assets:Bank:Giro        -42.17 EUR
  Expenses:Food:Groceries

2026-10-19 ! "City Utilities" "Electricity, monthly instalment"
  lentil_id: "giro-8c01d4e9a2f7"
  lentil_ref: "2026101912345"
  lentil_counterparty: "CITY UTILITIES"
  lentil_purpose: "Electricity, monthly instalment"
  lentil_type: "Direct debit"
  Assets:Bank:Giro        -64.00 EUR
  Expenses:Uncategorized

2026-10-20 balance Assets:Bank:Giro  1234.56 ~ 0.00 EUR
"#;
        assert_eq!(content, expected);
    }

    #[test]
    fn unchecked_runs_end_with_the_mark() {
        for balance in [
            BalanceOutcome::Unchecked,
            BalanceOutcome::Suspect,
            BalanceOutcome::NeedsInput,
        ] {
            let content = render_run(
                "giro",
                "Assets:Bank:Giro",
                started(),
                "c",
                &spec_transactions()[..1],
                &balance,
            );
            assert!(
                content.ends_with("  Expenses:Food:Groceries\n\n; lentil: unchecked\n"),
                "{content}"
            );
        }
        let content = render_run(
            "giro",
            "Assets:Bank:Giro",
            started(),
            "c",
            &spec_transactions()[..1],
            &BalanceOutcome::AlreadyExists {
                date: Date::constant(2026, 10, 20),
                difference: None,
            },
        );
        assert!(
            content.ends_with("  Expenses:Food:Groceries\n"),
            "{content}"
        );
    }

    #[test]
    fn amounts_align_right_and_payee_is_optional() {
        let transactions = vec![
            txn(
                Date::constant(2026, 10, 1),
                None,
                "Interest",
                "x-1",
                None,
                [None, Some("Interest"), None],
                Decimal::new(5, 2),
                "Income:Interest",
            ),
            txn(
                Date::constant(2026, 10, 2),
                Some("Big \"Shop\""),
                "Path C:\\x",
                "x-2",
                None,
                [Some("Big \"Shop\""), Some("Path C:\\x"), Some("A\\B \"C\"")],
                Decimal::new(-123_456, 2),
                "Expenses:X",
            ),
        ];
        let content = render_run(
            "x",
            "Assets:Bank:Savings",
            started(),
            "c",
            &transactions,
            &BalanceOutcome::Unchecked,
        );
        let expected = r#"; lentil import run
; source: x, started: 2026-10-20T07:15:02Z, client: c

2026-10-01 ! "Interest"
  lentil_id: "x-1"
  lentil_purpose: "Interest"
  Assets:Bank:Savings      0.05 EUR
  Income:Interest

2026-10-02 ! "Big \"Shop\"" "Path C:\\x"
  lentil_id: "x-2"
  lentil_counterparty: "Big \"Shop\""
  lentil_purpose: "Path C:\\x"
  lentil_type: "A\\B \"C\""
  Assets:Bank:Savings  -1234.56 EUR
  Expenses:X

; lentil: unchecked
"#;
        assert_eq!(content, expected);
    }

    #[test]
    fn tolerance_has_the_amount_places_and_at_least_two() {
        assert_eq!(zero_tolerance(Decimal::new(123_456, 2)), "0.00");
        assert_eq!(zero_tolerance(Decimal::new(1234, 0)), "0.00");
        assert_eq!(zero_tolerance(Decimal::new(1_234_567, 3)), "0.000");
        let content = render_run(
            "giro",
            "Assets:Bank:Giro",
            started(),
            "c",
            &[],
            &BalanceOutcome::Assertion {
                date: Date::constant(2026, 10, 20),
                amount: Decimal::new(-5, 0),
                currency: "EUR".into(),
            },
        );
        assert_eq!(
            content,
            "; lentil import run\n; source: giro, started: 2026-10-20T07:15:02Z, client: c\n\n2026-10-20 balance Assets:Bank:Giro  -5 ~ 0.00 EUR\n"
        );
    }

    #[test]
    fn zero_is_never_negative() {
        let mut zero = Decimal::new(0, 2);
        zero.set_sign_negative(true);
        assert_eq!(number(zero), "0.00");
    }

    #[test]
    fn escape_backslash_and_quote() {
        assert_eq!(escape(r#"a\b"c"#), r#"a\\b\"c"#);
    }
}
