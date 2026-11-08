//! Differential tests: rustledger, through `ledger::load`, against the
//! reference implementation, Python Beancount's `bean-check` (PRD FR-B4).
//!
//! Each case builds a synthetic repository from `init_files` plus hand-written
//! run files, writes it to a temporary directory and runs
//! `uv run bean-check main.beancount` from the repository root, which uses the
//! Python Beancount pinned in `uv.lock`. Both sides must agree on whether the
//! ledger is valid and, where both report locations, on the set of
//! `(file, line)` of the errors. A divergence fails the test.
//!
//! Set `LENTIL_SKIP_DIFFERENTIAL` to skip these tests where uv or the pinned
//! Python is not available.

// Test code may panic on unexpected input, like the unit tests (clippy.toml).
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use lentil_core::finding::FindingCode;
use lentil_core::import::Original;
use lentil_core::layout::{InitOptions, init_files};
use lentil_core::learn::Learner;
use lentil_core::ledger;
use lentil_core::snapshot::Snapshot;
use lentil_core::validate::{Context, Role, validate};

const SKIP_VAR: &str = "LENTIL_SKIP_DIFFERENTIAL";
const MAIN: &str = "main.beancount";

const CONFIG: &str = r#"spec_version = 0
main_file = "main.beancount"
fallback_account = "Expenses:Uncategorized"
unattributed_account = "Expenses:Unattributed"
price_currencies = ["USD"]

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

[[snapshot]]
account = "Assets:PaymentService"
currency = "EUR"
"#;

const ACCOUNTS_EXTRA: &str = "
2026-01-01 open Assets:Bank:Giro EUR
2026-01-01 open Liabilities:Card EUR
2026-01-01 open Assets:PaymentService EUR
2026-01-01 open Assets:Cash EUR
2026-01-01 open Expenses:Food:Groceries
2026-01-01 open Expenses:Utilities
2026-01-01 open Expenses:Books
2026-01-01 open Expenses:Travel
2026-01-01 open Income:Salary
2026-01-01 commodity USD
";

const GIRO_RUN: &str = "imports/giro/2026/20261020T071502Z.beancount";
const GIRO_RUN_TEXT: &str = r#"; lentil import run
; source: giro, started: 2026-10-20T07:15:02Z, client: lentil-cli/0.1.0

2026-10-01 ! "Employer" "Salary October"
  lentil_id: "giro-0a1b2c3d4e5f"
  lentil_counterparty: "EMPLOYER"
  lentil_purpose: "SALARY 10/2026"
  Assets:Bank:Giro        2500.00 EUR
  Income:Salary

2026-10-18 ! "Corner Grocery" "Card payment"
  lentil_id: "giro-3f9a1c2e7b4d"
  lentil_counterparty: "CORNER GROCERY 4711"
  lentil_purpose: "CARD PAYMENT"
  Assets:Bank:Giro        -42.17 EUR
  Expenses:Food:Groceries

2026-10-19 ! "City Utilities" "Electricity, monthly instalment"
  lentil_id: "giro-8c01d4e9a2f7"
  lentil_ref: "2026101912345"
  lentil_counterparty: "CITY UTILITIES"
  lentil_purpose: "ELECTRICITY INSTALMENT 10/2026"
  Assets:Bank:Giro        -64.00 EUR
  Expenses:Uncategorized

; 2026-10-19 ! "Card Company" "Card statement"
;   lentil_id: "giro-77aa88bb99cc"
;   lentil_counterparty: "CARD COMPANY"
;   lentil_purpose: "STATEMENT 09/2026"
;   Assets:Bank:Giro        -20.00 EUR
;   Expenses:Uncategorized

2026-10-20 balance Assets:Bank:Giro  2393.83 ~ 0.00 EUR
"#;

const CARD_RUN: &str = "imports/card/2026/20261021T080000Z.beancount";
const CARD_RUN_TEXT: &str = r#"; lentil import run
; source: card, started: 2026-10-21T08:00:00Z, client: lentil-cli/0.1.0
; lentil: unchecked

2026-10-10 ! "Bookshop" "Books"
  lentil_id: "card-111111111111"
  lentil_counterparty: "BOOKSHOP 12 MAIN STREET"
  Liabilities:Card        -20.00 EUR
  Expenses:Books

2026-10-12 ! "Hotel" "Two nights"
  lentil_id: "card-222222222222"
  lentil_counterparty: "HOTEL CENTRAL"
  Liabilities:Card        -200.00 EUR
  Expenses:Travel         216.00 USD @@ 200.00 EUR
"#;

const MANUAL: &str = "manual/k7p2q9xd/2026-10.beancount";
const MANUAL_TEXT: &str = r#"2026-10-05 * "Market" "Vegetables" #market
  lentil_id: "k7p2q9xd-01jb2m3n4p5q6r7s8t9v0w1x2y"
  Assets:Cash             -5.00 EUR
  Expenses:Food:Groceries

2026-10-30 pad Assets:PaymentService Equity:OpeningBalances
2026-10-31 balance Assets:PaymentService  80.00 ~ 0.00 EUR
"#;

const PRICES: &str = "prices/2026/20261020.beancount";
const PRICES_TEXT: &str = "2026-10-20 price USD 0.9260 EUR\n";

/// A valid repository with two sources, several transactions, a dismissed
/// transaction, a price annotation, a manual entry, a balance snapshot with a
/// pad and passing balance assertions.
fn valid_repository() -> Snapshot {
    let mut files = init_files(&InitOptions {
        title: "Synthetic Ledger".into(),
        currency: "EUR".into(),
    });
    files.insert("lentil.toml", CONFIG);
    let accounts = format!(
        "{}{ACCOUNTS_EXTRA}",
        files.text("accounts.beancount").unwrap()
    );
    files.insert("accounts.beancount", accounts);
    files.insert("sources/giro.toml", "[file]\nheader = [\"Date\"]\n");
    files.insert("sources/card.toml", "[file]\nheader = [\"Date\"]\n");
    files.insert(GIRO_RUN, GIRO_RUN_TEXT);
    files.insert(CARD_RUN, CARD_RUN_TEXT);
    files.insert(MANUAL, MANUAL_TEXT);
    files.insert(PRICES, PRICES_TEXT);
    files
}

fn with(mut files: Snapshot, path: &str, from: &str, to: &str) -> Snapshot {
    let text = files.text(path).unwrap();
    assert!(text.contains(from), "{path} lacks {from:?}");
    let replaced = text.replacen(from, to, 1);
    files.insert(path, replaced);
    files
}

fn with_file(mut files: Snapshot, path: &str, text: &str) -> Snapshot {
    files.insert(path, text);
    files
}

/// What one implementation reports.
#[derive(Debug)]
struct Report {
    valid: bool,
    located: BTreeSet<(String, u32)>,
    unlocated: usize,
    messages: Vec<String>,
}

/// rustledger as Lentil uses it: `ledger::load`, with parse errors located
/// the way `validate` locates them, because the loader reports them per file.
fn rustledger(files: &Snapshot) -> Report {
    let ledger = ledger::load(files, MAIN);
    let mut report = Report {
        valid: true,
        located: BTreeSet::new(),
        unlocated: 0,
        messages: Vec::new(),
    };
    let is_parse_error = |e: &&ledger::CoreError| e.message.starts_with("parse errors in ");
    let parse_errors: Vec<(Option<String>, Option<u32>, String)> =
        if ledger.errors.iter().any(|e| is_parse_error(&e)) {
            let context = Context {
                role: Role::Admin,
                device: None,
                before: None,
            };
            validate(files, &context)
                .findings
                .into_iter()
                .filter(|f| f.code == FindingCode::ParseError)
                .map(|f| (f.file, f.line, format!("parse: {:?}", f.args)))
                .collect()
        } else {
            Vec::new()
        };
    let errors = ledger
        .errors
        .iter()
        .filter(|e| e.is_error && !is_parse_error(e));
    for (file, line, message) in errors
        .map(|e| (e.file.clone(), e.line, format!("{}: {}", e.code, e.message)))
        .chain(parse_errors)
        .chain(ledger.balance_failures.iter().map(|f| {
            (
                f.file.clone(),
                f.line,
                format!("balance {} {} {}", f.account, f.asserted, f.difference),
            )
        }))
    {
        report.valid = false;
        report.messages.push(message);
        match (file, line) {
            (Some(file), Some(line)) if line > 0 => {
                report.located.insert((file, line));
            }
            _ => report.unlocated += 1,
        }
    }
    report
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn python(files: &Snapshot) -> Report {
    let dir = tempfile::tempdir().unwrap();
    for (path, content) in files.files() {
        let target = dir.path().join(path);
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::write(target, content).unwrap();
    }
    let output = Command::new("uv")
        .args(["run", "--frozen", "--project"])
        .arg(workspace_root())
        .args(["bean-check", "--no-cache", "--json", MAIN])
        .current_dir(dir.path())
        .output()
        .expect("uv must be on PATH (set LENTIL_SKIP_DIFFERENTIAL to skip)");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let json: serde_json::Value = serde_json::from_str(&stdout).unwrap_or_else(|e| {
        panic!(
            "bean-check did not print JSON ({e}); status {}, stdout {stdout}, stderr {}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        )
    });
    let roots = [dir.path().canonicalize().unwrap(), dir.path().to_path_buf()];
    let mut report = Report {
        valid: true,
        located: BTreeSet::new(),
        unlocated: 0,
        messages: Vec::new(),
    };
    for error in json["errors"].as_array().unwrap() {
        report.valid = false;
        report
            .messages
            .push(error["message"].as_str().unwrap_or_default().to_owned());
        let file = error["filename"].as_str().and_then(|f| {
            roots
                .iter()
                .find_map(|root| Path::new(f).strip_prefix(root).ok())
                .map(|p| p.to_string_lossy().into_owned())
        });
        let line = error["lineno"].as_u64().and_then(|l| u32::try_from(l).ok());
        match (file, line) {
            (Some(file), Some(line)) if line > 0 => {
                report.located.insert((file, line));
            }
            _ => report.unlocated += 1,
        }
    }
    assert_eq!(
        report.valid,
        output.status.success(),
        "bean-check exit status and errors disagree: {report:?}"
    );
    report
}

fn skip() -> bool {
    if std::env::var_os(SKIP_VAR).is_some() {
        eprintln!("differential test skipped: {SKIP_VAR} is set");
        return true;
    }
    false
}

/// Runs both implementations and fails on any divergence, or if the case
/// does not have the validity it was written for.
fn compare(name: &str, files: &Snapshot, expect_valid: bool) {
    if skip() {
        return;
    }
    let rust = rustledger(files);
    let py = python(files);
    let context = format!("{name}:\n  rustledger: {rust:?}\n  Python Beancount: {py:?}");
    if std::env::var_os("LENTIL_DIFFERENTIAL_VERBOSE").is_some() {
        eprintln!("{context}");
    }
    assert_eq!(rust.valid, py.valid, "validity diverges in {context}");
    assert_eq!(rust.valid, expect_valid, "unexpected validity in {context}");
    // Compare locations where both report them. An error one side reports
    // without a location may be any of the other side's located errors.
    match (rust.unlocated, py.unlocated) {
        (0, 0) => assert_eq!(rust.located, py.located, "locations diverge in {context}"),
        (_, 0) => assert!(
            rust.located.is_subset(&py.located),
            "locations diverge in {context}"
        ),
        (0, _) => assert!(
            py.located.is_subset(&rust.located),
            "locations diverge in {context}"
        ),
        _ => {}
    }
}

#[test]
fn init_files_are_valid() {
    let files = init_files(&InitOptions {
        title: "Ledger".into(),
        currency: "EUR".into(),
    });
    compare("init files", &files, true);
}

#[test]
fn valid_repository_is_valid() {
    compare("valid repository", &valid_repository(), true);
}

#[test]
fn unbalanced_transaction() {
    let files = with(
        valid_repository(),
        MANUAL,
        "  Expenses:Food:Groceries\n",
        "  Expenses:Food:Groceries  4.00 EUR\n",
    );
    compare("unbalanced transaction", &files, false);
}

#[test]
fn account_not_open() {
    let files = with(
        valid_repository(),
        GIRO_RUN,
        "  Expenses:Food:Groceries\n",
        "  Expenses:Food:Restaurant\n",
    );
    compare("account not open", &files, false);
}

#[test]
fn account_closed() {
    let files = with_file(
        valid_repository(),
        "accounts.beancount",
        &format!(
            "{}2026-09-30 close Expenses:Utilities\n",
            valid_repository().text("accounts.beancount").unwrap()
        ),
    );
    let files = with(
        files,
        GIRO_RUN,
        "  Expenses:Uncategorized\n",
        "  Expenses:Utilities\n",
    );
    compare("account closed", &files, false);
}

#[test]
fn balance_assertion_off_by_one_cent_with_zero_tolerance() {
    let files = with(
        valid_repository(),
        GIRO_RUN,
        "2393.83 ~ 0.00",
        "2393.84 ~ 0.00",
    );
    compare("assertion off by 0.01 with ~ 0.00", &files, false);
}

#[test]
fn balance_assertion_off_by_one_cent_with_default_tolerance_passes() {
    // Why spec section 8 requires `~ 0.00`: the default tolerance hides a
    // one-cent gap in both implementations.
    let files = with(valid_repository(), GIRO_RUN, "2393.83 ~ 0.00", "2393.84");
    compare("assertion off by 0.01 without tolerance", &files, true);
}

#[test]
fn passing_balance_assertion() {
    let files = with(valid_repository(), CARD_RUN, "; lentil: unchecked\n", "");
    let text = format!(
        "{}\n2026-10-21 balance Liabilities:Card  -220.00 ~ 0.00 EUR\n",
        files.text(CARD_RUN).unwrap()
    );
    let files = with_file(files, CARD_RUN, &text);
    compare("passing assertion", &files, true);
}

#[test]
fn duplicate_open() {
    let text = format!(
        "{}2026-02-01 open Expenses:Books\n",
        valid_repository().text("accounts.beancount").unwrap()
    );
    let files = with_file(valid_repository(), "accounts.beancount", &text);
    compare("duplicate open", &files, false);
}

#[test]
fn parse_error() {
    let files = with(
        valid_repository(),
        CARD_RUN,
        "2026-10-12 ! \"Hotel\" \"Two nights\"",
        "2026-10-12 ! \"Hotel\" \"Two nights",
    );
    compare("parse error", &files, false);
}

#[test]
fn parse_error_in_posting() {
    let files = with(
        valid_repository(),
        MANUAL,
        "  Assets:Cash             -5.00 EUR\n",
        "  Assets:Cash             -5.00.0 EUR\n",
    );
    compare("parse error in a posting", &files, false);
}

#[test]
fn posting_before_open_date() {
    let files = with(
        valid_repository(),
        "accounts.beancount",
        "2026-01-01 open Expenses:Books\n",
        "2026-10-11 open Expenses:Books\n",
    );
    compare("posting before the open date", &files, false);
}

#[test]
fn unused_pad() {
    // Spec section 9: an unused pad is an error in both implementations.
    let files = with(
        valid_repository(),
        MANUAL,
        "2026-10-31 balance Assets:PaymentService  80.00 ~ 0.00 EUR\n",
        "2026-10-31 balance Assets:PaymentService  0.00 ~ 0.00 EUR\n",
    );
    compare("unused pad", &files, false);
}

#[test]
fn empty_include_glob() {
    let mut files = valid_repository();
    files.remove("prices/keep/keep.beancount");
    files.remove(PRICES);
    compare("include glob without matches", &files, false);
}

/// Counter accounts of the synthetic large ledgers; all but the fallback
/// account are opened.
const COUNTERS: [&str; 6] = [
    "Expenses:Food:Groceries",
    "Expenses:Utilities",
    "Expenses:Books",
    "Expenses:Travel",
    "Income:Salary",
    "Expenses:Uncategorized",
];
/// Transactions of the synthetic large ledgers.
const TOTAL: usize = 50_000;
/// Months of the synthetic large ledgers: ten years.
const MONTHS: usize = 120;

/// A synthetic ledger covering ten years with 50,000 transactions (PRD
/// performance NFR): three sources, one run file per source and month, each
/// closed by a passing balance assertion.
fn large_repository() -> (Snapshot, usize) {
    const SOURCES: [(&str, &str); 3] = [
        ("giro", "Assets:Bank:Giro"),
        ("savings", "Assets:Bank:Savings"),
        ("card", "Liabilities:Card"),
    ];
    let mut files = init_files(&InitOptions {
        title: "Large".into(),
        currency: "EUR".into(),
    });
    let mut accounts = files.text("accounts.beancount").unwrap().to_owned();
    for (_, account) in SOURCES {
        writeln!(accounts, "2016-01-01 open {account} EUR").unwrap();
    }
    for account in &COUNTERS[..5] {
        writeln!(accounts, "2016-01-01 open {account}").unwrap();
    }
    files.insert("accounts.beancount", accounts);

    let runs = MONTHS * SOURCES.len();
    let mut written = 0;
    let mut balances = [0i64; 3];
    for month in 0..MONTHS {
        let year = 2016 + month / 12;
        let mon = month % 12 + 1;
        for (s, (id, account)) in SOURCES.iter().enumerate() {
            let run = month * SOURCES.len() + s;
            let count = (TOTAL - written) / (runs - run);
            let mut text = format!("; lentil import run\n; source: {id}\n\n");
            for i in 0..count {
                let day = i % 28 + 1;
                let cents = i64::try_from((i * 7919 + run * 31) % 20_000).unwrap() - 9_000;
                balances[s] += cents;
                let counter = COUNTERS[(i + run) % COUNTERS.len()];
                write!(
                    text,
                    "{year}-{mon:02}-{day:02} ! \"Payee {i}\" \"Narration {run}\"\n  lentil_id: \"{id}-{run:06}{i:06}\"\n  lentil_counterparty: \"PAYEE {i}\"\n  lentil_purpose: \"RUN {run}\"\n  {account}  {}.{:02} EUR\n  {counter}\n\n",
                    if cents < 0 { format!("-{}", -cents / 100) } else { (cents / 100).to_string() },
                    (cents % 100).abs()
                )
                .unwrap();
            }
            written += count;
            let (next_year, next_mon) = if mon == 12 {
                (year + 1, 1)
            } else {
                (year, mon + 1)
            };
            let balance = balances[s];
            writeln!(
                text,
                "{next_year}-{next_mon:02}-01 balance {account}  {}{}.{:02} ~ 0.00 EUR",
                if balance < 0 { "-" } else { "" },
                balance.abs() / 100,
                balance.abs() % 100
            )
            .unwrap();
            files.insert(
                format!("imports/{id}/{year}/{year}{mon:02}28T120000Z.beancount"),
                text,
            );
        }
    }
    (files, written)
}

#[test]
#[ignore = "performance measurement; run with --ignored --nocapture"]
fn large_ledger_load_time() {
    let (files, count) = large_repository();
    assert_eq!(count, 50_000);
    let start = Instant::now();
    let ledger = ledger::load(&files, MAIN);
    let elapsed = start.elapsed();
    assert!(
        ledger.errors.is_empty(),
        "{:?}",
        &ledger.errors[..ledger.errors.len().min(5)]
    );
    assert!(
        ledger.balance_failures.is_empty(),
        "{:?}",
        &ledger.balance_failures[..1]
    );
    assert_eq!(ledger.transactions.len(), 50_000);
    println!(
        "ledger::load of 10 years, {} transactions in {} files: {elapsed:?}",
        ledger.transactions.len(),
        ledger.files.len()
    );

    let mut files = files;
    files.insert("lentil.toml", CONFIG);
    let start = Instant::now();
    let context = Context {
        role: Role::Admin,
        device: None,
        before: None,
    };
    let validation = validate(&files, &context);
    let elapsed = start.elapsed();
    assert!(!validation.is_blocking(), "{:?}", &validation.findings[..1]);
    println!("validate of the same ledger: {elapsed:?}");
}

/// `n` in base 26 with the digits `a` to `z`, at least two of them.
fn letters(mut n: usize) -> String {
    let mut digits = Vec::new();
    while digits.len() < 2 || n > 0 {
        digits.push(char::from(b'a' + u8::try_from(n % 26).unwrap()));
        n /= 26;
    }
    digits.iter().rev().collect()
}

/// One source with 50,000 reviewed transactions over ten years, 2,000
/// counterparties and recurring words in the purpose, one run file per month.
fn learner_repository() -> Snapshot {
    let mut files = init_files(&InitOptions {
        title: "Learner".into(),
        currency: "EUR".into(),
    });
    let mut accounts = files.text("accounts.beancount").unwrap().to_owned();
    writeln!(accounts, "2016-01-01 open Assets:Bank:Giro EUR").unwrap();
    for account in &COUNTERS[..5] {
        writeln!(accounts, "2016-01-01 open {account}").unwrap();
    }
    files.insert("accounts.beancount", accounts);
    let per_month = TOTAL / MONTHS + 1;
    for month in 0..MONTHS {
        let year = 2016 + month / 12;
        let mon = month % 12 + 1;
        let mut text = String::from("; lentil import run\n; source: giro\n\n");
        for i in (month * per_month..TOTAL).take(per_month) {
            let day = i % 28 + 1;
            let counterparty = format!("MERCHANT {}", letters(i % 2_000));
            let purpose = format!("PURCHASE {} {}", letters(i % 97), letters(i % 13));
            let counter = COUNTERS[(i % 2_000) % 5];
            write!(
                text,
                "{year}-{mon:02}-{day:02} * \"{counterparty}\" \"{purpose}\"\n  lentil_id: \"giro-{i:012}\"\n  lentil_counterparty: \"{counterparty}\"\n  lentil_purpose: \"{purpose}\"\n  Assets:Bank:Giro  -1.00 EUR\n  {counter}\n\n"
            )
            .unwrap();
        }
        files.insert(
            format!("imports/giro/{year}/{year}{mon:02}28T120000Z.beancount"),
            text,
        );
    }
    files
}

#[test]
#[ignore = "performance measurement; run with --ignored --nocapture"]
fn learner_time() {
    let ledger = ledger::load(&learner_repository(), MAIN);
    assert!(
        ledger.errors.is_empty(),
        "{:?}",
        &ledger.errors[..ledger.errors.len().min(5)]
    );
    assert_eq!(ledger.transactions.len(), TOTAL);
    let texts: Vec<Original<'_>> = ledger
        .transactions
        .iter()
        .step_by(TOTAL / 1_000)
        .map(Original::from)
        .collect();
    assert_eq!(texts.len(), 1_000);

    let start = Instant::now();
    let learner = Learner::train(&ledger, "Assets:Bank:Giro", "Expenses:Uncategorized", &[]);
    let suggested = texts
        .iter()
        .filter(|text| learner.suggest(text).is_some())
        .count();
    let elapsed = start.elapsed();
    println!(
        "Learner::train on {TOTAL} reviewed transactions and suggest for {} texts ({suggested} suggested): {elapsed:?}",
        texts.len()
    );
    assert!(elapsed < Duration::from_millis(500));
}
