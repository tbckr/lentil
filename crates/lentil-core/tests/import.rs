//! End-to-end import of the fictional exports under `testdata/exports/`:
//! mapping, records, plan and the exact run file, which must load cleanly.

// Helpers outside `#[test]` functions are test code too (clippy.toml).
#![allow(clippy::unwrap_used)]

use std::collections::BTreeSet;

use jiff::Timestamp;
use jiff::civil::Date;
use lentil_core::config::Config;
use lentil_core::id;
use lentil_core::import::{
    BalanceOutcome, ImportPlan, ImportRequest, Mapping, Original, Record, Rules, StatedBalance,
    Statement, SuggestionKind, UserBalance, plan_import, read_statement,
};
use lentil_core::ledger::{self, Ledger};
use lentil_core::reapply::{self, ReapplyFields, ReapplyOptions};
use lentil_core::snapshot::Snapshot;
use rust_decimal::Decimal;

const GIRO_EXPORT: &[u8] = include_bytes!("../../../testdata/exports/giro-windows1252.csv");
const GIRO_MAPPING: &str = include_str!("../../../testdata/mappings/giro-windows1252.toml");
const CARD_EXPORT: &[u8] = include_bytes!("../../../testdata/exports/card-iso885915.csv");
const CARD_MAPPING: &str = include_str!("../../../testdata/mappings/card-iso885915.toml");
const WALLET_EXPORT: &[u8] = include_bytes!("../../../testdata/exports/wallet-utf8.csv");
const WALLET_MAPPING: &str = include_str!("../../../testdata/mappings/wallet-utf8.toml");

const CONFIG: &str = r#"
spec_version = 0
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

[[source]]
id = "wallet"
account = "Assets:Wallet"
currency = "EUR"
importer = "csv"
mapping = "sources/wallet.toml"
"#;

const MAIN: &str = r#"option "title" "Ledger"
option "operating_currency" "EUR"

include "accounts.beancount"
include "imports/*/*/*.beancount"
"#;

const ACCOUNTS: &str = "2020-01-01 open Assets:Bank:Giro EUR
2020-01-01 open Liabilities:Card EUR
2020-01-01 open Assets:Wallet EUR
2020-01-01 open Equity:OpeningBalances
2020-01-01 open Expenses:Uncategorized
2020-01-01 open Expenses:Unattributed
2020-01-01 open Expenses:Food:Groceries
2020-01-01 open Income:Salary
";

const RULES: &str = r#"
[[rule]]
match = { source = "giro", counterparty = "(?i)eckladen", type = "^Kartenzahlung$" }
payee = "Eckladen"
account = "Expenses:Food:Groceries"

[[rule]]
match = { counterparty = "(?i)^beispiel gmbh$", purpose = "(?i)^gehalt" }
account = "Income:Salary"
"#;

struct Repo {
    snapshot: Snapshot,
    config: Config,
    rules: Rules,
    ledger: Ledger,
}

impl Repo {
    fn new(extra: &[(&str, &str)]) -> Self {
        let mut snapshot = Snapshot::from_files([
            ("main.beancount", MAIN),
            ("accounts.beancount", ACCOUNTS),
            ("imports/keep/keep/keep.beancount", ""),
        ]);
        for (path, content) in extra {
            snapshot.insert(*path, *content);
        }
        let ledger = ledger::load(&snapshot, "main.beancount");
        assert!(ledger.errors.is_empty(), "{:?}", ledger.errors);
        Self {
            snapshot,
            config: Config::parse(CONFIG).unwrap(),
            rules: Rules::parse(RULES).unwrap(),
            ledger,
        }
    }

    fn plan(
        &self,
        source: &str,
        statement: &Statement,
        started: &str,
        run_date: &str,
        balance: UserBalance,
        accepted: &BTreeSet<String>,
    ) -> ImportPlan {
        plan_import(&ImportRequest {
            config: &self.config,
            source: self.config.source(source).unwrap(),
            rules: &self.rules,
            ledger: &self.ledger,
            statement,
            started: started.parse::<Timestamp>().unwrap(),
            run_date: date(run_date),
            balance,
            accepted_duplicates: accepted,
            client: "lentil-cli/0.1.0",
        })
    }

    /// Adds the run file and checks that the ledger loads without errors,
    /// that every balance assertion holds, and that re-applying the rules
    /// changes nothing, because they match the written run as they matched
    /// the export.
    fn assert_loads(&self, plan: &ImportPlan) {
        let run = plan.run_file.as_ref().unwrap();
        let mut snapshot = self.snapshot.clone();
        snapshot.insert(run.path.clone(), run.content.clone());
        let ledger = ledger::load(&snapshot, "main.beancount");
        assert!(ledger.files.contains(&run.path));
        assert!(ledger.errors.is_empty(), "{:?}", ledger.errors);
        assert!(
            ledger.balance_failures.is_empty(),
            "{:?}",
            ledger.balance_failures
        );
        let source = &self.config.run_source(&run.path).unwrap().id;
        for planned in &plan.transactions {
            let written = ledger.find(&planned.id).unwrap();
            let found = self.rules.suggest(source, &Original::from(written));
            assert_eq!(
                found.payee.or(planned.counterparty.as_deref()),
                planned.payee.as_deref()
            );
            if planned.suggestion == SuggestionKind::Rule {
                assert_eq!(found.account, Some(planned.counter_account.as_str()));
            }
        }
        let all = ReapplyOptions {
            reviewed: true,
            fields: ReapplyFields::Both,
        };
        assert_eq!(
            reapply::plan(&ledger, &self.config, &self.rules, &all),
            Vec::new()
        );
    }
}

fn date(text: &str) -> Date {
    text.parse().unwrap()
}

fn dec(text: &str) -> Decimal {
    Decimal::from_str_exact(text).unwrap()
}

#[allow(clippy::too_many_arguments)]
fn record(
    day: &str,
    amount: &str,
    currency: &str,
    counterparty: Option<&str>,
    purpose: Option<&str>,
    reference: Option<&str>,
    balance_after: Option<&str>,
    line: usize,
) -> Record {
    Record {
        booking_date: date(day),
        amount: dec(amount),
        currency: currency.into(),
        counterparty: counterparty.map(Into::into),
        purpose: purpose.map(Into::into),
        booking_type: None,
        reference: reference.map(Into::into),
        balance_after: balance_after.map(dec),
        line,
    }
}

/// `record` with the booking type the bank gives.
fn typed(mut record: Record, booking_type: &str) -> Record {
    record.booking_type = Some(booking_type.into());
    record
}

#[test]
fn giro_windows1252_with_stated_balance_on_the_run_date() {
    let mapping = Mapping::parse(GIRO_MAPPING).unwrap();
    let statement = read_statement(GIRO_EXPORT, &mapping, "EUR").unwrap();
    assert_eq!(
        statement,
        Statement {
            records: vec![
                typed(
                    record(
                        "2026-10-20",
                        "-64.00",
                        "EUR",
                        Some("Stadtwerke Musterstadt"),
                        Some("Abschlag Strom 10/2026"),
                        None,
                        None,
                        7,
                    ),
                    "SEPA-Lastschrift",
                ),
                typed(
                    record(
                        "2026-10-19",
                        "-42.17",
                        "EUR",
                        Some("Eckladen Müller"),
                        Some("Kartenzahlung  4711"),
                        None,
                        None,
                        8,
                    ),
                    "Kartenzahlung",
                ),
                typed(
                    record(
                        "2026-10-19",
                        "-42.17",
                        "EUR",
                        Some("Eckladen Müller"),
                        Some("Kartenzahlung  4711"),
                        None,
                        None,
                        9,
                    ),
                    "Kartenzahlung",
                ),
                typed(
                    record(
                        "2026-10-15",
                        "2450.00",
                        "EUR",
                        Some("Beispiel GmbH"),
                        Some("Gehalt Oktober 2026 \"Projekt Süd\""),
                        None,
                        None,
                        10,
                    ),
                    "Gutschrift  Überweisung",
                ),
                record(
                    "2026-10-01",
                    "-1150.00",
                    "EUR",
                    Some("Wohnbau Beispiel eG"),
                    Some("Miete Oktober"),
                    None,
                    None,
                    11,
                ),
            ],
            stated_balance: Some(StatedBalance {
                amount: dec("1234.56"),
                date: Some(date("2026-10-20")),
            }),
            skipped_rows: 1,
        }
    );

    // First run: the stated balance refers to the run date, so the run-date
    // booking is subtracted, and an opening balance explains the rest.
    let repo = Repo::new(&[]);
    let plan = repo.plan(
        "giro",
        &statement,
        "2026-10-20T07:15:02Z",
        "2026-10-20",
        UserBalance::NotAsked,
        &BTreeSet::new(),
    );
    assert_eq!(
        (
            plan.skipped_existing,
            plan.skipped_run_date,
            plan.skipped_rows
        ),
        (0, 1, 1)
    );
    assert!(plan.possible_duplicates.is_empty());
    let suggestions: Vec<_> = plan.transactions.iter().map(|t| t.suggestion).collect();
    assert_eq!(
        suggestions,
        vec![
            SuggestionKind::Fallback,
            SuggestionKind::Rule,
            SuggestionKind::Rule,
            SuggestionKind::Rule
        ]
    );
    assert_eq!(
        plan.balance,
        BalanceOutcome::Assertion {
            date: date("2026-10-20"),
            amount: dec("1298.56"),
            currency: "EUR".into(),
        }
    );
    let run = plan.run_file.as_ref().unwrap();
    assert_eq!(run.path, "imports/giro/2026/20261020T071502Z.beancount");
    assert_eq!(
        run.content,
        r#"; lentil import run
; source: giro, started: 2026-10-20T07:15:02Z, client: lentil-cli/0.1.0

2026-10-01 ! "Opening balance"
  lentil_id: "giro-aa658d06d641"
  Assets:Bank:Giro           82.90 EUR
  Equity:OpeningBalances

2026-10-01 ! "Wohnbau Beispiel eG" "Miete Oktober"
  lentil_id: "giro-aee6867ae035"
  lentil_counterparty: "Wohnbau Beispiel eG"
  lentil_purpose: "Miete Oktober"
  Assets:Bank:Giro        -1150.00 EUR
  Expenses:Uncategorized

2026-10-15 ! "Beispiel GmbH" "Gehalt Oktober 2026 \"Projekt Süd\""
  lentil_id: "giro-ff83424121d6"
  lentil_counterparty: "Beispiel GmbH"
  lentil_purpose: "Gehalt Oktober 2026 \"Projekt Süd\""
  lentil_type: "Gutschrift Überweisung"
  Assets:Bank:Giro         2450.00 EUR
  Income:Salary

2026-10-19 ! "Eckladen" "Kartenzahlung 4711"
  lentil_id: "giro-5d97a3acf95e"
  lentil_counterparty: "Eckladen Müller"
  lentil_purpose: "Kartenzahlung 4711"
  lentil_type: "Kartenzahlung"
  Assets:Bank:Giro          -42.17 EUR
  Expenses:Food:Groceries

2026-10-19 ! "Eckladen" "Kartenzahlung 4711"
  lentil_id: "giro-ebbf18d5061b"
  lentil_counterparty: "Eckladen Müller"
  lentil_purpose: "Kartenzahlung 4711"
  lentil_type: "Kartenzahlung"
  Assets:Bank:Giro          -42.17 EUR
  Expenses:Food:Groceries

2026-10-20 balance Assets:Bank:Giro  1298.56 ~ 0.00 EUR
"#
    );
    repo.assert_loads(&plan);

    // Importing the same export again writes nothing: every ID exists, and
    // the assertion for the run date exists with the same amount.
    let repo = Repo::new(&[(run.path.as_str(), run.content.as_str())]);
    let again = repo.plan(
        "giro",
        &statement,
        "2026-10-20T09:00:00Z",
        "2026-10-20",
        UserBalance::NotAsked,
        &BTreeSet::new(),
    );
    assert_eq!(again.skipped_existing, 4);
    assert!(again.transactions.is_empty() && again.opening.is_none());
    assert_eq!(
        again.balance,
        BalanceOutcome::AlreadyExists {
            date: date("2026-10-20"),
            difference: None
        }
    );
    assert_eq!(again.run_file, None);
}

#[test]
fn card_iso885915_with_running_balance_newest_first() {
    let mapping = Mapping::parse(CARD_MAPPING).unwrap();
    let statement = read_statement(CARD_EXPORT, &mapping, "EUR").unwrap();
    assert_eq!(
        statement,
        Statement {
            records: vec![
                record(
                    "2026-10-19",
                    "-18.40",
                    "EUR",
                    Some("Café Lumière, Lyon"),
                    None,
                    None,
                    Some("-1268.40"),
                    4,
                ),
                // The debit is negative in the file and still a debit.
                record(
                    "2026-10-17",
                    "-1250.00",
                    "EUR",
                    Some("Librairie Œuvre"),
                    None,
                    None,
                    Some("-1250.00"),
                    5,
                ),
                record(
                    "2026-10-16",
                    "500.00",
                    "EUR",
                    Some("Card Repayment"),
                    None,
                    None,
                    Some("0.00"),
                    6,
                ),
                record(
                    "2026-10-02",
                    "-500.00",
                    "EUR",
                    Some("Train Tickets"),
                    None,
                    None,
                    Some("-500.00"),
                    8,
                ),
            ],
            stated_balance: None,
            skipped_rows: 1,
        }
    );

    // The running balance fits newest first; the assertion takes the balance
    // after the last booking before the run date. It is the first run, but
    // the records explain the whole balance, so no opening balance is needed.
    let repo = Repo::new(&[]);
    let plan = repo.plan(
        "card",
        &statement,
        "2026-10-21T18:30:00Z",
        "2026-10-21",
        UserBalance::NotAsked,
        &BTreeSet::new(),
    );
    assert_eq!(plan.opening, None);
    let run = plan.run_file.as_ref().unwrap();
    assert_eq!(run.path, "imports/card/2026/20261021T183000Z.beancount");
    assert_eq!(
        run.content,
        r#"; lentil import run
; source: card, started: 2026-10-21T18:30:00Z, client: lentil-cli/0.1.0

2026-10-02 ! "Train Tickets" ""
  lentil_id: "card-89042207e3e4"
  lentil_counterparty: "Train Tickets"
  Liabilities:Card        -500.00 EUR
  Expenses:Uncategorized

2026-10-16 ! "Card Repayment" ""
  lentil_id: "card-458742060aab"
  lentil_counterparty: "Card Repayment"
  Liabilities:Card         500.00 EUR
  Expenses:Uncategorized

2026-10-17 ! "Librairie Œuvre" ""
  lentil_id: "card-53648cd5da81"
  lentil_counterparty: "Librairie Œuvre"
  Liabilities:Card       -1250.00 EUR
  Expenses:Uncategorized

2026-10-19 ! "Café Lumière, Lyon" ""
  lentil_id: "card-46319256750e"
  lentil_counterparty: "Café Lumière, Lyon"
  Liabilities:Card         -18.40 EUR
  Expenses:Uncategorized

2026-10-20 balance Liabilities:Card  -1268.40 ~ 0.00 EUR
"#
    );
    repo.assert_loads(&plan);

    // Read as Latin-1, the export decodes differently and the IDs change.
    let latin1 = Mapping::parse(&CARD_MAPPING.replace("iso-8859-15", "iso-8859-1")).unwrap();
    let records = read_statement(CARD_EXPORT, &latin1, "EUR").unwrap().records;
    assert_eq!(records[1].counterparty.as_deref(), Some("Librairie ¼uvre"));
}

#[test]
fn wallet_utf8_with_references_and_a_user_balance() {
    let mapping = Mapping::parse(WALLET_MAPPING).unwrap();
    let statement = read_statement(WALLET_EXPORT, &mapping, "EUR").unwrap();
    assert_eq!(
        statement,
        Statement {
            records: vec![
                record(
                    "2026-10-20",
                    "-2.80",
                    "EUR",
                    Some("Night Bus Musterstadt"),
                    None,
                    Some("TX-0004"),
                    None,
                    2,
                ),
                record(
                    "2026-10-18",
                    "-23.50",
                    "EUR",
                    Some("Green Grocer Musterstadt"),
                    Some("Weekly  veg box"),
                    Some("TX-0003"),
                    None,
                    3,
                ),
                record(
                    "2026-10-18",
                    "-3.20",
                    "EUR",
                    Some("Coffee Corner"),
                    None,
                    Some("TX-0002"),
                    None,
                    4,
                ),
                record(
                    "2026-10-12",
                    "100.00",
                    "EUR",
                    Some("Top-up"),
                    Some("from giro"),
                    Some("TX-0001"),
                    None,
                    5,
                ),
            ],
            stated_balance: None,
            skipped_rows: 0,
        }
    );

    // An earlier run imported TX-0001, and the coffee under an ID from before
    // the mapping had a reference column.
    let top_up = id::hashed_id("wallet", "wallet|TX-0001");
    assert_eq!(top_up, "wallet-2d73aafa3882");
    let existing = format!(
        "; lentil import run
; source: wallet, started: 2026-10-13T08:00:00Z, client: lentil-cli/0.1.0

2026-10-12 ! \"Top-up\" \"from giro\"
  lentil_id: \"{top_up}\"
  lentil_ref: \"TX-0001\"
  lentil_counterparty: \"Top-up\"
  lentil_purpose: \"from giro\"
  Assets:Wallet           100.00 EUR
  Expenses:Uncategorized

2026-10-18 * \"Coffee Corner\"
  lentil_id: \"wallet-0123456789ab\"
  lentil_counterparty: \"COFFEE CORNER\"
  Assets:Wallet  -3.20 EUR
  Expenses:Food:Groceries
"
    );
    let existing_path = "imports/wallet/2026/20261018T090000Z.beancount";
    let repo = Repo::new(&[(existing_path, existing.as_str())]);
    let none = BTreeSet::new();

    // The export has no balance: the CLI must ask first.
    let plan = repo.plan(
        "wallet",
        &statement,
        "2026-10-20T07:15:02Z",
        "2026-10-20",
        UserBalance::NotAsked,
        &none,
    );
    assert_eq!(plan.balance, BalanceOutcome::NeedsInput);
    assert_eq!(plan.run_file, None);
    assert_eq!((plan.skipped_existing, plan.skipped_run_date), (1, 1));
    assert_eq!(plan.possible_duplicates.len(), 1);
    let duplicate = &plan.possible_duplicates[0];
    assert_eq!(duplicate.new_id, "wallet-65d8b298e334");
    assert_eq!(duplicate.line, 4);
    assert_eq!(duplicate.existing_id, "wallet-0123456789ab");
    assert_eq!(duplicate.existing_file, existing_path);
    assert_eq!(duplicate.existing_line, 12);

    // The user enters the balance for yesterday; the possible duplicate
    // stays out of the run.
    let plan = repo.plan(
        "wallet",
        &statement,
        "2026-10-20T07:15:02Z",
        "2026-10-20",
        UserBalance::Given {
            amount: dec("73.30"),
            date: date("2026-10-19"),
        },
        &none,
    );
    let run = plan.run_file.as_ref().unwrap();
    assert_eq!(run.path, "imports/wallet/2026/20261020T071502Z.beancount");
    assert_eq!(
        run.content,
        r#"; lentil import run
; source: wallet, started: 2026-10-20T07:15:02Z, client: lentil-cli/0.1.0

2026-10-18 ! "Green Grocer Musterstadt" "Weekly veg box"
  lentil_id: "wallet-c321c4732412"
  lentil_ref: "TX-0003"
  lentil_counterparty: "Green Grocer Musterstadt"
  lentil_purpose: "Weekly veg box"
  Assets:Wallet          -23.50 EUR
  Expenses:Uncategorized

2026-10-20 balance Assets:Wallet  73.30 ~ 0.00 EUR
"#
    );
    repo.assert_loads(&plan);

    // Accepted, the coffee is written, with the account learned from the
    // reviewed transaction's original text.
    let accepted = BTreeSet::from(["wallet-65d8b298e334".to_owned()]);
    let plan = repo.plan(
        "wallet",
        &statement,
        "2026-10-20T07:15:02Z",
        "2026-10-20",
        UserBalance::Skipped,
        &accepted,
    );
    assert!(plan.possible_duplicates.is_empty());
    let coffee = &plan.transactions[1];
    assert_eq!(coffee.id, "wallet-65d8b298e334");
    assert_eq!(coffee.reference.as_deref(), Some("TX-0002"));
    assert_eq!(coffee.counter_account, "Expenses:Food:Groceries");
    assert_eq!(coffee.suggestion, SuggestionKind::Learned);
    assert!(
        plan.run_file
            .as_ref()
            .unwrap()
            .content
            .ends_with("\n\n; lentil: unchecked\n")
    );
    repo.assert_loads(&plan);
}
