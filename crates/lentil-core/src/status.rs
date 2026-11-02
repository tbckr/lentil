//! `lentil status` (PRD FR-E2): unreviewed transactions, failed balance
//! assertions and import runs without a balance check.

use crate::config::Config;
use crate::layout::PLACEHOLDERS;
use crate::ledger::{BalanceFailure, Ledger};
use crate::snapshot::Snapshot;

/// Comment that marks a run whose balance the user skipped (spec section 8).
pub const UNCHECKED_MARK: &str = "; lentil: unchecked";

/// An import run without a balance check.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UncheckedRun {
    /// The run file.
    pub file: String,
    /// The source ID.
    pub source: String,
}

/// An open transaction, summarized.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpenItem {
    /// `lentil_id`, if any.
    pub id: Option<String>,
    /// File and line of the transaction.
    pub file: String,
    /// 1-based line.
    pub line: u32,
    /// Date as `YYYY-MM-DD`.
    pub date: String,
    /// Payee and narration joined for display.
    pub text: String,
}

/// The status report.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Status {
    /// Open transactions, newest first.
    pub open: Vec<OpenItem>,
    /// Failing balance assertions.
    pub failed_assertions: Vec<BalanceFailure>,
    /// Runs marked unchecked and not covered by a later assertion.
    pub unchecked_runs: Vec<UncheckedRun>,
}

/// Computes the status report. `snapshot` gives the run files' text.
///
/// A run marked with [`UNCHECKED_MARK`] stays unchecked until a balance
/// assertion on its source's account is dated after the run's last active
/// transaction: an assertion holds at the start of its date, so only then
/// does it cover every transaction of the run (spec section 8). A run whose
/// transactions were all dismissed has nothing left to check.
#[must_use]
pub fn status(config: &Config, ledger: &Ledger, snapshot: &Snapshot) -> Status {
    let open = ledger
        .open_queue()
        .into_iter()
        .map(|t| OpenItem {
            id: t.id.clone(),
            file: t.file.clone(),
            line: t.line,
            date: t.date.to_string(),
            text: display_text(t.payee.as_deref(), &t.narration),
        })
        .collect();

    let mut unchecked_runs = Vec::new();
    for file in snapshot.glob("imports/*/*/*.beancount") {
        if PLACEHOLDERS.contains(&file) {
            continue;
        }
        let marked = snapshot
            .text(file)
            .is_some_and(|text| text.lines().any(|l| l.trim_end() == UNCHECKED_MARK));
        if !marked {
            continue;
        }
        let source = file.split('/').nth(1).unwrap_or_default();
        let Some(last) = ledger
            .transactions
            .iter()
            .filter(|t| t.file == file)
            .map(|t| t.date)
            .max()
        else {
            continue;
        };
        let covered = config.source(source).is_some_and(|s| {
            ledger
                .balance_assertions
                .iter()
                .any(|b| b.account == s.account && b.date > last)
        });
        if !covered {
            unchecked_runs.push(UncheckedRun {
                file: file.to_owned(),
                source: source.to_owned(),
            });
        }
    }

    Status {
        open,
        failed_assertions: ledger.balance_failures.clone(),
        unchecked_runs,
    }
}

/// Payee and narration as `payee | narration`, or whichever is not empty.
pub(crate) fn display_text(payee: Option<&str>, narration: &str) -> String {
    match payee.map(str::trim).filter(|p| !p.is_empty()) {
        Some(payee) if !narration.trim().is_empty() => format!("{payee} | {narration}"),
        Some(payee) => payee.to_owned(),
        None => narration.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ledger;

    const CONFIG: &str = r#"spec_version = 0
main_file = "main.beancount"
fallback_account = "Expenses:Uncategorized"
unattributed_account = "Expenses:Unattributed"

[[source]]
id = "giro"
account = "Assets:Bank:Giro"
currency = "EUR"
importer = "csv"
mapping = "sources/giro.toml"

[[source]]
id = "card"
account = "Liabilities:Card"
currency = "EUR"
importer = "csv"
mapping = "sources/card.toml"
"#;

    const MAIN: &str = r#"option "title" "Ledger"
option "operating_currency" "EUR"

include "accounts.beancount"
include "imports/*/*/*.beancount"
"#;

    const ACCOUNTS: &str = "2000-01-01 open Assets:Bank:Giro EUR
2000-01-01 open Liabilities:Card EUR
2000-01-01 open Expenses:Uncategorized
2000-01-01 open Expenses:Food
";

    const GIRO_RUN: &str = "imports/giro/2026/20261020T071502Z.beancount";
    const CARD_RUN: &str = "imports/card/2026/20261021T080000Z.beancount";

    fn txn(
        date: &str,
        flag: char,
        id: &str,
        payee: &str,
        narration: &str,
        account: &str,
    ) -> String {
        format!(
            "{date} {flag} \"{payee}\" \"{narration}\"\n  lentil_id: \"{id}\"\n  lentil_counterparty: \"X\"\n  {account}  -1.00 EUR\n  Expenses:Food\n\n"
        )
    }

    fn snapshot(giro_run: &str, card_run: &str) -> Snapshot {
        Snapshot::from_files([
            ("lentil.toml", CONFIG.to_owned()),
            ("main.beancount", MAIN.to_owned()),
            ("accounts.beancount", ACCOUNTS.to_owned()),
            (GIRO_RUN, giro_run.to_owned()),
            (CARD_RUN, card_run.to_owned()),
        ])
    }

    fn run_status(s: &Snapshot) -> Status {
        let config = Config::parse(CONFIG).unwrap();
        let ledger = ledger::load(s, "main.beancount");
        assert!(ledger.errors.is_empty(), "{:?}", ledger.errors);
        status(&config, &ledger, s)
    }

    #[test]
    fn lists_open_transactions_newest_first() {
        let giro = format!(
            "{}{}{}",
            txn(
                "2026-10-18",
                '!',
                "giro-1",
                "Corner Grocery",
                "Card payment",
                "Assets:Bank:Giro"
            ),
            txn(
                "2026-10-19",
                '*',
                "giro-2",
                "Bakery",
                "Bread",
                "Assets:Bank:Giro"
            ),
            txn("2026-10-19", '!', "giro-3", "", "Fee", "Assets:Bank:Giro"),
        );
        let card = txn("2026-10-20", '!', "card-1", "Shop", "", "Liabilities:Card");
        let report = run_status(&snapshot(&giro, &card));
        let summary: Vec<_> = report
            .open
            .iter()
            .map(|o| (o.id.as_deref().unwrap(), o.date.as_str(), o.text.as_str()))
            .collect();
        assert_eq!(
            summary,
            vec![
                ("card-1", "2026-10-20", "Shop"),
                ("giro-3", "2026-10-19", "Fee"),
                ("giro-1", "2026-10-18", "Corner Grocery | Card payment"),
            ]
        );
        assert_eq!(report.open[2].file, GIRO_RUN);
        assert_eq!(report.open[2].line, 1);
        assert!(report.failed_assertions.is_empty());
        assert!(report.unchecked_runs.is_empty());
    }

    #[test]
    fn lists_failed_assertions() {
        let giro = format!(
            "{}2026-10-20 balance Assets:Bank:Giro  -1.01 ~ 0.00 EUR\n",
            txn("2026-10-18", '!', "giro-1", "A", "B", "Assets:Bank:Giro")
        );
        let card = txn("2026-10-20", '*', "card-1", "Shop", "", "Liabilities:Card");
        let report = run_status(&snapshot(&giro, &card));
        assert_eq!(report.failed_assertions.len(), 1);
        let failure = &report.failed_assertions[0];
        assert_eq!(failure.account, "Assets:Bank:Giro");
        assert_eq!(failure.file.as_deref(), Some(GIRO_RUN));
    }

    #[test]
    fn reports_unchecked_runs_until_a_later_assertion_covers_them() {
        let giro = format!(
            "; lentil import run\n{UNCHECKED_MARK}\n\n{}",
            txn("2026-10-18", '!', "giro-1", "A", "B", "Assets:Bank:Giro")
        );
        let card = format!(
            "{UNCHECKED_MARK}\n\n{}",
            txn("2026-10-20", '!', "card-1", "Shop", "", "Liabilities:Card")
        );
        let mut s = snapshot(&giro, &card);
        let report = run_status(&s);
        assert_eq!(
            report.unchecked_runs,
            vec![
                UncheckedRun {
                    file: CARD_RUN.into(),
                    source: "card".into()
                },
                UncheckedRun {
                    file: GIRO_RUN.into(),
                    source: "giro".into()
                },
            ]
        );

        // An assertion on the run's last date does not cover it: it holds at
        // the start of that day, before the run's transactions.
        s.insert(
            "imports/giro/2026/20261101T070000Z.beancount",
            "2026-10-18 balance Assets:Bank:Giro  0.00 ~ 0.00 EUR\n",
        );
        let report = run_status(&s);
        assert_eq!(report.unchecked_runs.len(), 2);

        // An assertion on another account does not cover it either.
        s.insert(
            "imports/card/2026/20261101T070000Z.beancount",
            "2026-10-19 balance Liabilities:Card  0.00 ~ 0.00 EUR\n",
        );
        let report = run_status(&s);
        assert_eq!(report.unchecked_runs.len(), 2);

        // A later assertion on the source's account covers the run.
        s.insert(
            "imports/giro/2026/20261101T070000Z.beancount",
            "2026-10-19 balance Assets:Bank:Giro  -1.00 ~ 0.00 EUR\n",
        );
        let report = run_status(&s);
        assert_eq!(
            report.unchecked_runs,
            vec![UncheckedRun {
                file: CARD_RUN.into(),
                source: "card".into()
            }]
        );
    }

    #[test]
    fn mark_must_be_a_whole_line() {
        let giro = format!(
            "; lentil: unchecked later\n\n{}",
            txn("2026-10-18", '!', "giro-1", "A", "B", "Assets:Bank:Giro")
        );
        let card = txn("2026-10-20", '!', "card-1", "Shop", "", "Liabilities:Card");
        let report = run_status(&snapshot(&giro, &card));
        assert!(report.unchecked_runs.is_empty());
    }

    #[test]
    fn display_text_joins_payee_and_narration() {
        assert_eq!(display_text(Some("Shop"), "Bread"), "Shop | Bread");
        assert_eq!(display_text(Some("Shop"), ""), "Shop");
        assert_eq!(display_text(Some(" "), "Bread"), "Bread");
        assert_eq!(display_text(None, "Bread"), "Bread");
    }
}
