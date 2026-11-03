//! Planning one import run (spec section 8): IDs, duplicates, suggestions,
//! balance assertion and the run file. Pure: the CLI calls it again with the
//! user's answers until nothing is left to ask.

use std::collections::BTreeSet;

use jiff::civil::Date;
use jiff::{SignedDuration, Timestamp};
use rust_decimal::Decimal;

use super::render::{render_run, run_path};
use super::{Learned, Original, Record, Rules, Statement};
use crate::config::{Config, SourceConfig};
use crate::id;
use crate::layout::OPENING_BALANCES_ACCOUNT;
use crate::ledger::{Ledger, Txn};

/// Everything an import run needs.
#[derive(Clone, Copy, Debug)]
pub struct ImportRequest<'a> {
    /// The repository configuration.
    pub config: &'a Config,
    /// The source being imported.
    pub source: &'a SourceConfig,
    /// Rules from `rules.toml`.
    pub rules: &'a Rules,
    /// The current ledger, for IDs, duplicates, assertions and learned suggestions.
    pub ledger: &'a Ledger,
    /// The export's records.
    pub statement: &'a Statement,
    /// Start of the run in UTC; names the run file.
    pub started: Timestamp,
    /// The run date in the user's time zone. Records on or after it are not
    /// imported yet, because the bank may still book on it (spec section 18,
    /// partial days, option A).
    pub run_date: Date,
    /// The user's answer to the balance prompt.
    pub balance: UserBalance,
    /// IDs of possible duplicates the user confirmed as new transactions.
    pub accepted_duplicates: &'a BTreeSet<String>,
    /// `Lentil-Client` value, such as `lentil-cli/0.1.0`, for the run header.
    pub client: &'a str,
}

/// The user's answer to "balance shown in online banking" (PRD FR-C6).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UserBalance {
    /// Not asked (yet).
    NotAsked,
    /// The user skipped; the run is marked `; lentil: unchecked`.
    Skipped,
    /// The balance and the date it refers to (end of that day).
    Given {
        /// The balance.
        amount: Decimal,
        /// The day the balance refers to.
        date: Date,
    },
}

/// Where a planned transaction's counter account came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SuggestionKind {
    /// A rule in `rules.toml`.
    Rule,
    /// Learned from reviewed transactions.
    Learned,
    /// `fallback_account`.
    Fallback,
    /// The opening balance transaction.
    Opening,
}

/// One transaction the run will write.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlannedTxn {
    /// `lentil_id`.
    pub id: String,
    /// Booking date.
    pub date: Date,
    /// Payee: a rule's payee, otherwise the normalized counterparty, if any.
    pub payee: Option<String>,
    /// Narration: the normalized purpose, or empty.
    pub narration: String,
    /// `lentil_counterparty`: the normalized counterparty, if any.
    pub counterparty: Option<String>,
    /// `lentil_purpose`: the normalized purpose, if any.
    pub purpose: Option<String>,
    /// `lentil_type`: the normalized booking type, if any.
    pub booking_type: Option<String>,
    /// `lentil_ref`, if the bank provides one.
    pub reference: Option<String>,
    /// Amount on the source account.
    pub amount: Decimal,
    /// Currency.
    pub currency: String,
    /// Suggested counter account.
    pub counter_account: String,
    /// Where the suggestion came from.
    pub suggestion: SuggestionKind,
    /// The tags the rules add, in the order the evaluation adds them (spec
    /// section 16); none for the opening transaction.
    pub tags: Vec<String>,
}

/// A new record that may duplicate an existing transaction whose ID the
/// export no longer reproduces (PRD FR-C7).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PossibleDuplicate {
    /// The new record's ID.
    pub new_id: String,
    /// The new record's line in the export.
    pub line: usize,
    /// The new record's original text: counterparty and purpose.
    pub desc: String,
    /// Booking date shared by both.
    pub date: Date,
    /// Amount shared by both.
    pub amount: Decimal,
    /// Currency shared by both.
    pub currency: String,
    /// The existing transaction's ID.
    pub existing_id: String,
    /// The existing transaction's original text: counterparty and purpose.
    pub existing_desc: Option<String>,
    /// The existing transaction's file.
    pub existing_file: String,
    /// The existing transaction's line.
    pub existing_line: u32,
}

/// What happens with the run's balance assertion.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BalanceOutcome {
    /// The run writes `<date> balance <account> <amount> ~ 0.00 <currency>`.
    Assertion {
        /// Assertion date; it holds at the start of this day.
        date: Date,
        /// Asserted balance.
        amount: Decimal,
        /// Currency.
        currency: String,
    },
    /// The export has no balance and the user was not asked yet.
    NeedsInput,
    /// The user skipped; the run gets `; lentil: unchecked`.
    Unchecked,
    /// `from = "column"`: neither row order gives a consistent running
    /// balance; no assertion, mapping reported as suspect.
    Suspect,
    /// An assertion for the account and date exists already; none is written.
    AlreadyExists {
        /// The assertion date.
        date: Date,
        /// `existing - computed` if they differ.
        difference: Option<Decimal>,
    },
}

/// A run file to write.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunFile {
    /// `imports/<source-id>/<YYYY>/<YYYYMMDDTHHMMSSZ>.beancount`.
    pub path: String,
    /// Complete content.
    pub content: String,
}

/// The result of planning a run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImportPlan {
    /// Transactions to write, in booking order, then export order.
    pub transactions: Vec<PlannedTxn>,
    /// Opening balance transaction on a source's first run, if a balance is known.
    pub opening: Option<PlannedTxn>,
    /// Records whose ID exists already, active or dismissed.
    pub skipped_existing: usize,
    /// Records on or after the run date, left for a later run.
    pub skipped_run_date: usize,
    /// Rows skipped by the mapping's `[[skip]]` entries.
    pub skipped_rows: usize,
    /// Possible duplicates not (yet) accepted; they are not written.
    pub possible_duplicates: Vec<PossibleDuplicate>,
    /// The balance assertion decision.
    pub balance: BalanceOutcome,
    /// The run file, or `None` if the run has nothing new (spec section 8).
    pub run_file: Option<RunFile>,
}

/// Narration of the opening balance transaction.
pub const OPENING_BALANCE_TEXT: &str = "Opening balance";

/// The `lentil_id` of a source's opening balance transaction:
/// `<source-id>-` + 12 hex characters of SHA-256(`<source-id>|opening`).
#[must_use]
pub fn opening_id(source_id: &str) -> String {
    id::hashed_id(source_id, &format!("{source_id}|opening"))
}

/// Plans an import run.
///
/// Records whose ID exists already (active or dismissed) are skipped, records
/// on or after the run date wait for a later run, and possible duplicates are
/// held back until the user accepts them. The run file is `None` if nothing
/// new would be written, and also while the balance still needs the user's
/// input ([`BalanceOutcome::NeedsInput`]).
#[must_use]
pub fn plan_import(request: &ImportRequest<'_>) -> ImportPlan {
    let ImportRequest {
        config,
        source,
        ledger,
        statement,
        run_date,
        ..
    } = *request;
    let records = &statement.records;
    let ids = id::import_ids(&source.id, records);
    let existing = ledger.all_ids();
    let opening_id = opening_id(&source.id);
    let orphans = orphans(ledger, source, records, &ids, &opening_id);
    let learned = Learned::from_ledger(ledger, &source.account, &config.fallback_account);

    let mut transactions = Vec::new();
    let mut possible_duplicates = Vec::new();
    let mut skipped_existing = 0;
    let mut skipped_run_date = 0;
    for (record, id) in records.iter().zip(&ids) {
        if existing.contains(id.as_str()) {
            skipped_existing += 1;
            continue;
        }
        if record.booking_date >= run_date {
            skipped_run_date += 1;
            continue;
        }
        let desc = id::description(record);
        if !request.accepted_duplicates.contains(id)
            && let Some(orphan) = orphans.iter().find(|o| o.matches(record))
        {
            possible_duplicates.push(PossibleDuplicate {
                new_id: id.clone(),
                line: record.line,
                desc,
                date: record.booking_date,
                amount: record.amount,
                currency: record.currency.clone(),
                existing_id: orphan.id.to_owned(),
                existing_desc: orphan.txn.original_text(),
                existing_file: orphan.txn.file.clone(),
                existing_line: orphan.txn.line,
            });
            continue;
        }
        let counterparty = original(record.counterparty.as_deref());
        let purpose = original(record.purpose.as_deref());
        let booking_type = original(record.booking_type.as_deref());
        let fields = Original::new(
            counterparty.as_deref(),
            purpose.as_deref(),
            booking_type.as_deref(),
        );
        let found = request.rules.suggest(&source.id, &fields);
        let tags = request
            .rules
            .tags(&source.id, &fields, &[], |_| false)
            .into_iter()
            .map(str::to_owned)
            .collect();
        let payee = found
            .payee
            .map(str::to_owned)
            .or_else(|| counterparty.clone());
        let narration = purpose.clone().unwrap_or_default();
        let (counter_account, suggestion) =
            suggest(request, &learned, record, found.account, &desc);
        transactions.push(PlannedTxn {
            id: id.clone(),
            date: record.booking_date,
            payee,
            narration,
            counterparty,
            purpose,
            booking_type,
            reference: record.reference.clone(),
            amount: record.amount,
            currency: record.currency.clone(),
            counter_account,
            suggestion,
            tags,
        });
    }
    // Stable: same-day transactions keep the order the bank lists them in.
    transactions.sort_by_key(|t| t.date);

    let balance = plan_balance(request);
    let opening = plan_opening(request, &transactions, &balance, &opening_id);

    let has_new = !transactions.is_empty()
        || opening.is_some()
        || matches!(balance, BalanceOutcome::Assertion { .. });
    let run_file = (has_new && balance != BalanceOutcome::NeedsInput).then(|| {
        let all: Vec<PlannedTxn> = opening.iter().chain(&transactions).cloned().collect();
        let started = free_start(request);
        RunFile {
            path: run_path(&source.id, started),
            content: render_run(
                &source.id,
                &source.account,
                started,
                request.client,
                &all,
                &balance,
            ),
        }
    });

    ImportPlan {
        transactions,
        opening,
        skipped_existing,
        skipped_run_date,
        skipped_rows: statement.skipped_rows,
        possible_duplicates,
        balance,
        run_file,
    }
}

/// An existing transaction of the source inside the export's date range whose
/// ID the export does not reproduce (spec section 8).
///
/// Only a record on the transaction's own date can match it, and every record
/// date lies within the export's range, so the range test reduces to "on a
/// date the export contains".
struct Orphan<'l> {
    id: &'l str,
    txn: &'l Txn,
    amount: Decimal,
    currency: &'l str,
}

impl Orphan<'_> {
    fn matches(&self, record: &Record) -> bool {
        self.txn.date == record.booking_date
            && self.amount == record.amount
            && self.currency == record.currency
    }
}

fn orphans<'l>(
    ledger: &'l Ledger,
    source: &SourceConfig,
    records: &[Record],
    ids: &[String],
    opening_id: &str,
) -> Vec<Orphan<'l>> {
    let dates: BTreeSet<Date> = records.iter().map(|r| r.booking_date).collect();
    let reproduced: BTreeSet<&str> = ids.iter().map(String::as_str).collect();
    ledger
        .transactions
        .iter()
        .filter_map(|txn| {
            let id = txn.id.as_deref()?;
            if !id::is_import_id_of(&source.id, id)
                || id == opening_id
                || reproduced.contains(id)
                || !dates.contains(&txn.date)
            {
                return None;
            }
            let (amount, currency) = txn
                .postings
                .iter()
                .find(|p| p.account == source.account)?
                .units
                .as_ref()?;
            Some(Orphan {
                id,
                txn,
                amount: *amount,
                currency,
            })
        })
        .collect()
}

/// An original field as the run writes it: normalized, `None` if empty.
fn original(text: Option<&str>) -> Option<String> {
    Some(id::normalize_text(text?)).filter(|t| !t.is_empty())
}

/// The counter account: a rule's account first, then a learned account for
/// the original text `desc` that is open on the booking date and not a
/// source account, then `fallback_account`.
fn suggest(
    request: &ImportRequest<'_>,
    learned: &Learned,
    record: &Record,
    rule_account: Option<&str>,
    desc: &str,
) -> (String, SuggestionKind) {
    if let Some(account) = rule_account {
        return (account.to_owned(), SuggestionKind::Rule);
    }
    if let Some(account) = learned.suggest(desc).filter(|a| {
        request.ledger.is_open_on(a, record.booking_date) && !request.config.is_source_account(a)
    }) {
        return (account.to_owned(), SuggestionKind::Learned);
    }
    (
        request.config.fallback_account.clone(),
        SuggestionKind::Fallback,
    )
}

/// The run's balance decision (spec section 8).
fn plan_balance(request: &ImportRequest<'_>) -> BalanceOutcome {
    let ImportRequest {
        source,
        ledger,
        statement,
        run_date,
        ..
    } = *request;
    let records = &statement.records;
    let column = if records.iter().any(|r| r.balance_after.is_some()) {
        match column_balance(records, run_date) {
            Ok(found) => found,
            Err(Suspect) => return BalanceOutcome::Suspect,
        }
    } else {
        None
    };
    let found = match (column, &statement.stated_balance) {
        (Some(found), _) => Some(found),
        (None, Some(stated)) => stated_balance(stated.amount, stated.date, records, run_date),
        (None, None) => match request.balance {
            UserBalance::NotAsked => return BalanceOutcome::NeedsInput,
            UserBalance::Skipped => return BalanceOutcome::Unchecked,
            UserBalance::Given { amount, date } => {
                stated_balance(amount, Some(date), records, run_date)
            }
        },
    };
    let Some((date, amount)) = found else {
        return BalanceOutcome::Suspect;
    };
    if let Some(existing) = ledger
        .balance_assertions
        .iter()
        .find(|b| b.account == source.account && b.date == date && b.currency == source.currency)
    {
        return BalanceOutcome::AlreadyExists {
            date,
            difference: existing.amount.checked_sub(amount).filter(|d| !d.is_zero()),
        };
    }
    BalanceOutcome::Assertion {
        date,
        amount,
        currency: source.currency.clone(),
    }
}

/// The running balance does not fit the amounts in either row order.
#[derive(Debug, PartialEq, Eq)]
struct Suspect;

/// `from = "column"`: checks the running balance in file order and in
/// reverse order. From each order that fits, it takes the balance after the
/// last booking before the run date, dated the day after that booking. Both
/// orders can fit (a single row, or amounts that cancel out); then they must
/// agree. `Ok(None)` means there is no booking before the run date.
fn column_balance(records: &[Record], run_date: Date) -> Result<Option<(Date, Decimal)>, Suspect> {
    let balances: Vec<Decimal> = records
        .iter()
        .map(|r| r.balance_after)
        .collect::<Option<_>>()
        .ok_or(Suspect)?;
    let fits = |chronological: &[usize]| {
        chronological
            .windows(2)
            .all(|w| balances[w[0]].checked_add(records[w[1]].amount) == Some(balances[w[1]]))
    };
    let forward: Vec<usize> = (0..records.len()).collect();
    let reverse: Vec<usize> = (0..records.len()).rev().collect();
    let mut results = Vec::new();
    for order in [forward, reverse] {
        if !fits(&order) {
            continue;
        }
        let last = order
            .iter()
            .rev()
            .find(|&&i| records[i].booking_date < run_date);
        results.push(match last {
            Some(&i) => Some((
                records[i].booking_date.tomorrow().map_err(|_| Suspect)?,
                balances[i],
            )),
            None => None,
        });
    }
    match results.as_slice() {
        [only] => Ok(*only),
        [a, b] if a == b => Ok(*a),
        _ => Err(Suspect),
    }
}

/// A balance stated once, by the export or the user. A date before the run
/// date is the end of that day, so the assertion is dated the day after.
/// Without a date, or with the run date or later, the balance may include
/// bookings of the run date: the export's records on or after the run date
/// are subtracted and the assertion is dated on the run date. `None` if the
/// arithmetic overflows.
fn stated_balance(
    amount: Decimal,
    date: Option<Date>,
    records: &[Record],
    run_date: Date,
) -> Option<(Date, Decimal)> {
    match date {
        Some(date) if date < run_date => Some((date.tomorrow().ok()?, amount)),
        _ => {
            let later = sum(records
                .iter()
                .filter(|r| r.booking_date >= run_date)
                .map(|r| r.amount))?;
            Some((run_date, amount.checked_sub(later)?))
        }
    }
}

fn sum(amounts: impl IntoIterator<Item = Decimal>) -> Option<Decimal> {
    amounts
        .into_iter()
        .try_fold(Decimal::ZERO, Decimal::checked_add)
}

/// The run's start time, moved on by whole seconds while a run file of that
/// name exists: two runs of one source within a second would otherwise share
/// a file name (spec section 8).
fn free_start(request: &ImportRequest<'_>) -> Timestamp {
    let mut started = request.started;
    while request
        .ledger
        .files
        .contains(&run_path(&request.source.id, started))
    {
        match started.checked_add(SignedDuration::from_secs(1)) {
            Ok(next) => started = next,
            Err(_) => break,
        }
    }
    started
}

/// The opening balance transaction (spec section 8): written by the first run
/// that writes an assertion for the source, unless the ledger already holds
/// it, active or dismissed. It books the part of the asserted balance that the
/// source's transactions before the assertion, in the ledger and in this run,
/// do not explain, and is dated on the source's earliest transaction, but
/// always before the assertion. Omitted if that part is zero, or if a posting
/// on the source account has no amount to add up.
fn plan_opening(
    request: &ImportRequest<'_>,
    transactions: &[PlannedTxn],
    balance: &BalanceOutcome,
    opening_id: &str,
) -> Option<PlannedTxn> {
    let BalanceOutcome::Assertion {
        date: assertion_date,
        amount,
        currency,
    } = balance
    else {
        return None;
    };
    let ImportRequest { source, ledger, .. } = *request;
    let asserted_before = ledger
        .balance_assertions
        .iter()
        .any(|b| b.account == source.account);
    let exists = ledger.find(opening_id).is_some() || ledger.dismissed_ids.contains(opening_id);
    if asserted_before || exists {
        return None;
    }
    let mut booked: Vec<(Date, Decimal)> = Vec::new();
    for txn in &ledger.transactions {
        for posting in txn.postings.iter().filter(|p| p.account == source.account) {
            let (number, posting_currency) = posting.units.as_ref()?;
            if posting_currency == currency {
                booked.push((txn.date, *number));
            }
        }
    }
    booked.extend(transactions.iter().map(|t| (t.date, t.amount)));
    let explained = sum(booked
        .iter()
        .filter(|(date, _)| date < assertion_date)
        .map(|(_, number)| *number))?;
    let opening = amount.checked_sub(explained)?;
    if opening.is_zero() {
        return None;
    }
    let latest = assertion_date.yesterday().ok()?;
    let date = booked
        .iter()
        .map(|(date, _)| *date)
        .min()
        .map_or(latest, |d| d.min(latest));
    Some(PlannedTxn {
        id: opening_id.to_owned(),
        date,
        payee: None,
        narration: OPENING_BALANCE_TEXT.to_owned(),
        counterparty: None,
        purpose: None,
        booking_type: None,
        reference: None,
        amount: opening,
        currency: currency.clone(),
        counter_account: OPENING_BALANCES_ACCOUNT.to_owned(),
        suggestion: SuggestionKind::Opening,
        tags: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::import::StatedBalance;
    use crate::ledger;
    use crate::snapshot::Snapshot;

    const MAIN: &str = "option \"title\" \"Ledger\"\noption \"operating_currency\" \"EUR\"\n\ninclude \"accounts.beancount\"\ninclude \"imports/*/*/*.beancount\"\n";

    const ACCOUNTS: &str = "2020-01-01 open Assets:Bank:Giro EUR
2020-01-01 open Assets:Bank:Card EUR
2020-01-01 open Equity:OpeningBalances
2020-01-01 open Expenses:Uncategorized
2020-01-01 open Expenses:Food:Groceries
2020-01-01 open Expenses:Home
2020-01-01 open Expenses:Old
2026-06-01 close Expenses:Old
";

    const CONFIG: &str = r#"
spec_version = 0
main_file = "main.beancount"
fallback_account = "Expenses:Uncategorized"
unattributed_account = "Expenses:Uncategorized"

[[source]]
id = "giro"
account = "Assets:Bank:Giro"
currency = "EUR"
importer = "csv"
mapping = "sources/giro.toml"

[[source]]
id = "card"
account = "Assets:Bank:Card"
currency = "EUR"
importer = "csv"
mapping = "sources/card.toml"
"#;

    const EXISTING_FILE: &str = "imports/giro/2026/20261001T000000Z.beancount";
    const RUN_FILE: &str = "imports/giro/2026/20261020T071502Z.beancount";

    struct Fixture {
        snapshot: Snapshot,
        config: Config,
        ledger: Ledger,
        rules: Rules,
        accepted: BTreeSet<String>,
    }

    impl Fixture {
        fn new(existing: &str) -> Self {
            let snapshot = Snapshot::from_files([
                ("main.beancount", MAIN),
                ("accounts.beancount", ACCOUNTS),
                (EXISTING_FILE, existing),
            ]);
            let ledger = ledger::load(&snapshot, "main.beancount");
            assert!(ledger.errors.is_empty(), "{:?}", ledger.errors);
            Self {
                snapshot,
                config: Config::parse(CONFIG).unwrap(),
                ledger,
                rules: Rules::default(),
                accepted: BTreeSet::new(),
            }
        }

        fn plan(&self, statement: &Statement, run_date: &str, balance: UserBalance) -> ImportPlan {
            plan_import(&ImportRequest {
                config: &self.config,
                source: self.config.source("giro").unwrap(),
                rules: &self.rules,
                ledger: &self.ledger,
                statement,
                started: "2026-10-20T07:15:02Z".parse().unwrap(),
                run_date: date(run_date),
                balance,
                accepted_duplicates: &self.accepted,
                client: "lentil-test/0",
            })
        }

        /// Loads the ledger with the run file and checks it is error-free and
        /// every assertion holds.
        fn assert_loads(&self, plan: &ImportPlan) {
            let run = plan.run_file.as_ref().unwrap();
            assert_eq!(run.path, RUN_FILE);
            let mut snapshot = self.snapshot.clone();
            snapshot.insert(run.path.clone(), run.content.clone());
            let ledger = ledger::load(&snapshot, "main.beancount");
            assert!(
                ledger.errors.is_empty(),
                "{:?}\n{}",
                ledger.errors,
                run.content
            );
            assert!(
                ledger.balance_failures.is_empty(),
                "{:?}\n{}",
                ledger.balance_failures,
                run.content
            );
        }
    }

    fn date(text: &str) -> Date {
        text.parse().unwrap()
    }

    fn dec(text: &str) -> Decimal {
        Decimal::from_str_exact(text).unwrap()
    }

    fn rec(day: &str, amount: &str, counterparty: Option<&str>, purpose: Option<&str>) -> Record {
        Record {
            booking_date: date(day),
            amount: dec(amount),
            currency: "EUR".into(),
            counterparty: counterparty.map(Into::into),
            purpose: purpose.map(Into::into),
            booking_type: None,
            reference: None,
            balance_after: None,
            line: 0,
        }
    }

    fn with_balance(mut record: Record, balance: &str) -> Record {
        record.balance_after = Some(dec(balance));
        record
    }

    fn statement(records: Vec<Record>) -> Statement {
        let records = records
            .into_iter()
            .enumerate()
            .map(|(i, mut r)| {
                r.line = i + 2;
                r
            })
            .collect();
        Statement {
            records,
            stated_balance: None,
            skipped_rows: 0,
        }
    }

    fn stated(mut statement: Statement, amount: &str, day: Option<&str>) -> Statement {
        statement.stated_balance = Some(StatedBalance {
            amount: dec(amount),
            date: day.map(date),
        });
        statement
    }

    fn txn(day: &str, flag: char, id: &str, desc: &str, amount: &str, counter: &str) -> String {
        format!(
            "{day} {flag} \"x\"\n  lentil_id: \"{id}\"\n  lentil_counterparty: \"{desc}\"\n  Assets:Bank:Giro  {amount} EUR\n  {counter}\n\n"
        )
    }

    /// A ledger with one older giro transaction of 101.00 and an assertion
    /// after it, so no opening balance is planned and the balance before the
    /// tests' records is 101.00.
    fn history() -> String {
        format!(
            "{}2026-01-02 balance Assets:Bank:Giro  101.00 ~ 0.00 EUR\n",
            unchecked_history()
        )
    }

    /// The same transaction without an assertion, as an unchecked run leaves it.
    fn unchecked_history() -> String {
        txn(
            "2026-01-01",
            '*',
            "giro-000000000000",
            "OLD",
            "101.00",
            "Expenses:Home",
        )
    }

    fn assertion(day: &str, amount: &str) -> BalanceOutcome {
        BalanceOutcome::Assertion {
            date: date(day),
            amount: dec(amount),
            currency: "EUR".into(),
        }
    }

    fn ids(plan: &ImportPlan) -> Vec<&str> {
        plan.transactions.iter().map(|t| t.id.as_str()).collect()
    }

    #[test]
    fn skips_existing_active_and_dismissed_ids() {
        let records = vec![
            rec("2026-10-15", "-1.00", None, Some("A")),
            rec("2026-10-16", "-2.00", None, Some("B")),
            rec("2026-10-17", "-3.00", None, Some("C")),
        ];
        let id_a = id::import_id("giro", &records[0], 0);
        let id_b = id::import_id("giro", &records[1], 0);
        let dismissed = txn("2026-10-16", '!', &id_b, "B", "-2.00", "Expenses:Home")
            .lines()
            .map(|l| format!(";   {l}\n"))
            .collect::<String>();
        let existing = format!(
            "{}{}{dismissed}",
            history(),
            txn("2026-10-15", '!', &id_a, "A", "-1.00", "Expenses:Home")
        );
        let f = Fixture::new(&existing);
        let plan = f.plan(
            &statement(records.clone()),
            "2026-10-20",
            UserBalance::Skipped,
        );
        assert_eq!(plan.skipped_existing, 2);
        assert_eq!(ids(&plan), vec![id::import_id("giro", &records[2], 0)]);
        assert!(plan.possible_duplicates.is_empty());
        f.assert_loads(&plan);
    }

    #[test]
    fn run_date_and_later_records_wait_for_a_later_run() {
        let f = Fixture::new(&history());
        let records = vec![
            rec("2026-10-19", "-1.00", None, Some("A")),
            rec("2026-10-20", "-2.00", None, Some("B")),
            rec("2026-10-21", "-3.00", None, Some("C")),
        ];
        let plan = f.plan(&statement(records), "2026-10-20", UserBalance::Skipped);
        assert_eq!(plan.skipped_run_date, 2);
        assert_eq!(plan.transactions.len(), 1);
        assert_eq!(plan.transactions[0].date, date("2026-10-19"));
    }

    #[test]
    fn passes_through_skipped_rows() {
        let f = Fixture::new(&history());
        let mut s = statement(vec![rec("2026-10-19", "-1.00", None, Some("A"))]);
        s.skipped_rows = 3;
        assert_eq!(
            f.plan(&s, "2026-10-20", UserBalance::Skipped).skipped_rows,
            3
        );
    }

    #[test]
    fn payee_narration_and_original_fields_are_normalized() {
        let f = Fixture::new(&history());
        let mut typed = rec(
            "2026-10-18",
            "-42.17",
            Some("  Corner   Grocery "),
            Some(" 4711\tCARD "),
        );
        typed.booking_type = Some(" Debit  card ".into());
        let mut blank = rec("2026-10-18", "-1.00", Some(" "), None);
        blank.booking_type = Some("\t".into());
        let records = vec![typed, blank];
        let plan = f.plan(&statement(records), "2026-10-20", UserBalance::Skipped);
        let [a, b] = plan.transactions.as_slice() else {
            panic!()
        };
        assert_eq!(a.payee.as_deref(), Some("Corner Grocery"));
        assert_eq!(a.narration, "4711 CARD");
        assert_eq!(
            (
                a.counterparty.as_deref(),
                a.purpose.as_deref(),
                a.booking_type.as_deref()
            ),
            (
                Some("Corner Grocery"),
                Some("4711 CARD"),
                Some("Debit card")
            )
        );
        assert_eq!((b.payee.as_deref(), b.narration.as_str()), (None, ""));
        assert_eq!(
            (&b.counterparty, &b.purpose, &b.booking_type),
            (&None, &None, &None)
        );
    }

    #[test]
    fn a_reviewed_run_teaches_the_wording_it_was_imported_with() {
        // Quotes, backslashes and runs of whitespace pass through the run
        // file, so the written transaction learns under the record's key.
        let (counterparty, purpose) = (Some(" Café \"Zum  Eck\" "), Some("C:\\Karte\t4711"));
        let mut record = rec("2026-10-18", "-4.20", counterparty, purpose);
        record.booking_type = Some(" Karten\\zahlung  \"girocard\" ".into());
        let first = Fixture::new(&history());
        let plan = first.plan(
            &statement(vec![record.clone()]),
            "2026-10-20",
            UserBalance::Skipped,
        );
        let planned = plan.transactions[0].clone();
        let reviewed = plan
            .run_file
            .unwrap()
            .content
            .replacen(" ! ", " * ", 1)
            .replace("Expenses:Uncategorized", "Expenses:Food:Groceries");
        let second = Fixture::new(&format!("{}{reviewed}", history()));
        let written = second
            .ledger
            .transactions
            .iter()
            .find(|t| t.date == date("2026-10-18"))
            .unwrap();
        assert_eq!(written.original_text(), Some(id::description(&record)));
        // Rules see the written transaction as the importer saw the record.
        assert_eq!(
            Original::from(written),
            Original::new(
                planned.counterparty.as_deref(),
                planned.purpose.as_deref(),
                planned.booking_type.as_deref(),
            )
        );
        assert_eq!(
            Original::from(written).booking_type,
            "Karten\\zahlung \"girocard\""
        );

        let again = rec("2026-10-19", "-5.10", counterparty, purpose);
        let plan = second.plan(&statement(vec![again]), "2026-10-20", UserBalance::Skipped);
        let [t] = plan.transactions.as_slice() else {
            panic!()
        };
        assert_eq!(
            (t.counter_account.as_str(), t.suggestion),
            ("Expenses:Food:Groceries", SuggestionKind::Learned)
        );
    }

    #[test]
    fn orders_by_booking_date_then_export_order() {
        let f = Fixture::new(&history());
        let records = vec![
            rec("2026-10-19", "-1.00", None, Some("A")),
            rec("2026-10-18", "-2.00", None, Some("B")),
            rec("2026-10-18", "-3.00", None, Some("C")),
            rec("2026-10-17", "-4.00", None, Some("D")),
        ];
        let plan = f.plan(&statement(records), "2026-10-20", UserBalance::Skipped);
        let order: Vec<_> = plan
            .transactions
            .iter()
            .map(|t| t.narration.as_str())
            .collect();
        assert_eq!(order, vec!["D", "B", "C", "A"]);
    }

    #[test]
    fn suggestion_prefers_rules_then_learned_then_fallback() {
        let existing = [
            history(),
            txn(
                "2026-05-01",
                '*',
                "giro-111111111111",
                "BAKERY SUNSHINE 1",
                "-2.00",
                "Expenses:Food:Groceries",
            ),
            txn(
                "2026-05-01",
                '*',
                "giro-222222222222",
                "OLD SHOP 1",
                "-2.00",
                "Expenses:Old",
            ),
            txn(
                "2026-05-02",
                '!',
                "giro-333333333333",
                "REFUND",
                "2.00",
                "Expenses:Old",
            ),
        ]
        .concat();
        let mut f = Fixture::new(&existing);
        let records = vec![
            rec("2026-10-18", "-1.00", Some("BAKERY SUNSHINE"), Some("77")),
            rec("2026-10-18", "-1.00", Some("CITY UTILITIES"), None),
            rec("2026-10-18", "-1.00", Some("OLD SHOP"), Some("2")),
            rec("2026-10-18", "-1.00", Some("UNKNOWN"), None),
        ];
        let s = statement(records);
        let kinds = |plan: &ImportPlan| -> Vec<(String, SuggestionKind)> {
            plan.transactions
                .iter()
                .map(|t| (t.counter_account.clone(), t.suggestion))
                .collect()
        };
        let plan = f.plan(&s, "2026-10-20", UserBalance::Skipped);
        assert_eq!(
            kinds(&plan),
            vec![
                ("Expenses:Food:Groceries".into(), SuggestionKind::Learned),
                ("Expenses:Uncategorized".into(), SuggestionKind::Fallback),
                // Learned, but closed on the booking date.
                ("Expenses:Uncategorized".into(), SuggestionKind::Fallback),
                ("Expenses:Uncategorized".into(), SuggestionKind::Fallback),
            ]
        );
        f.rules = Rules::parse(
            "[[rule]]\nmatch = { counterparty = \"(?i)bakery|utilities\" }\naccount = \"Expenses:Home\"\n",
        )
        .unwrap();
        let plan = f.plan(&s, "2026-10-20", UserBalance::Skipped);
        assert_eq!(
            kinds(&plan)[0],
            ("Expenses:Home".into(), SuggestionKind::Rule)
        );
        assert_eq!(
            kinds(&plan)[1],
            ("Expenses:Home".into(), SuggestionKind::Rule)
        );
        f.assert_loads(&plan);
    }

    #[test]
    fn rules_set_the_payee_from_the_normalized_original_fields() {
        let existing = [
            history(),
            "2026-05-01 * \"Corner Grocery\" \"Card payment\"\n  lentil_id: \"giro-111111111111\"\n  lentil_counterparty: \"CORNER GROCERY 4711\"\n  lentil_purpose: \"Card payment\"\n  Assets:Bank:Giro  -2.00 EUR\n  Expenses:Food:Groceries\n\n".to_owned(),
        ]
        .concat();
        let mut f = Fixture::new(&existing);
        f.rules = Rules::parse(
            r#"
[[rule]]
match = { counterparty = "^CORNER GROCERY 4711$", type = "^Debit card$" }
payee = "Corner Grocery"

[[rule]]
match = { counterparty = "^CITY UTILITIES$" }
payee = "City Utilities"
account = "Expenses:Home"
"#,
        )
        .unwrap();
        let mut grocery = rec(
            "2026-10-18",
            "-1.00",
            Some(" CORNER  GROCERY 4711 "),
            Some("Card payment"),
        );
        grocery.booking_type = Some("Debit\tcard".into());
        let utilities = rec("2026-10-18", "-2.00", Some("CITY UTILITIES"), None);
        let untyped = rec(
            "2026-10-18",
            "-3.00",
            Some("CORNER GROCERY 4711"),
            Some("Card payment"),
        );
        let s = statement(vec![grocery, utilities, untyped]);
        let plan = f.plan(&s, "2026-10-20", UserBalance::Skipped);
        let written: Vec<_> = plan
            .transactions
            .iter()
            .map(|t| {
                (
                    t.payee.as_deref(),
                    t.counterparty.as_deref(),
                    t.counter_account.as_str(),
                    t.suggestion,
                )
            })
            .collect();
        assert_eq!(
            written,
            vec![
                // A payee-only rule leaves the account to the learned suggestion.
                (
                    Some("Corner Grocery"),
                    Some("CORNER GROCERY 4711"),
                    "Expenses:Food:Groceries",
                    SuggestionKind::Learned
                ),
                (
                    Some("City Utilities"),
                    Some("CITY UTILITIES"),
                    "Expenses:Home",
                    SuggestionKind::Rule
                ),
                // No type: the first rule does not match, the payee stays the counterparty.
                (
                    Some("CORNER GROCERY 4711"),
                    Some("CORNER GROCERY 4711"),
                    "Expenses:Food:Groceries",
                    SuggestionKind::Learned
                ),
            ]
        );
        let content = &plan.run_file.as_ref().unwrap().content;
        assert!(
            content.contains("2026-10-18 ! \"Corner Grocery\" \"Card payment\"\n"),
            "{content}"
        );
        f.assert_loads(&plan);
    }

    fn duplicate_fixture() -> Fixture {
        let existing = [
            history(),
            txn("2026-10-18", '!', "giro-aaaaaaaaaaaa", "CORNER GROCERY OLD WORDING", "-42.17", "Expenses:Home"),
            "2026-10-16 ! \"x\"\n  lentil_id: \"card-bbbbbbbbbbbb\"\n  lentil_counterparty: \"CARD\"\n  Assets:Bank:Card  -7.00 EUR\n  Expenses:Home\n\n".to_owned(),
        ]
        .concat();
        Fixture::new(&existing)
    }

    #[test]
    fn possible_duplicate_is_listed_and_not_written() {
        let f = duplicate_fixture();
        let records = vec![
            rec("2026-10-15", "-10.00", None, Some("A")),
            rec(
                "2026-10-18",
                "-42.170",
                Some("CORNER GROCERY"),
                Some("NEW WORDING"),
            ),
            rec("2026-10-19", "-1.00", None, Some("B")),
        ];
        let new_id = id::import_id("giro", &records[1], 0);
        let plan = f.plan(&statement(records), "2026-10-20", UserBalance::Skipped);
        assert_eq!(plan.transactions.len(), 2);
        assert!(!ids(&plan).contains(&new_id.as_str()));
        assert_eq!(
            plan.possible_duplicates,
            vec![PossibleDuplicate {
                new_id,
                line: 3,
                desc: "CORNER GROCERY NEW WORDING".into(),
                date: date("2026-10-18"),
                amount: dec("-42.170"),
                currency: "EUR".into(),
                existing_id: "giro-aaaaaaaaaaaa".into(),
                existing_desc: Some("CORNER GROCERY OLD WORDING".into()),
                existing_file: EXISTING_FILE.into(),
                existing_line: 8,
            }]
        );
        assert!(!plan.run_file.unwrap().content.contains("NEW WORDING"));
    }

    #[test]
    fn accepted_duplicate_is_written() {
        let mut f = duplicate_fixture();
        let records = vec![
            rec("2026-10-15", "-10.00", None, Some("A")),
            rec(
                "2026-10-18",
                "-42.17",
                Some("CORNER GROCERY"),
                Some("NEW WORDING"),
            ),
        ];
        f.accepted.insert(id::import_id("giro", &records[1], 0));
        let plan = f.plan(&statement(records), "2026-10-20", UserBalance::Skipped);
        assert!(plan.possible_duplicates.is_empty());
        assert_eq!(plan.transactions.len(), 2);
        f.assert_loads(&plan);
    }

    #[test]
    fn no_duplicate_for_other_date_amount_currency_or_source() {
        let f = duplicate_fixture();
        let mut usd = rec("2026-10-18", "-42.17", None, Some("usd"));
        usd.currency = "USD".into();
        for record in [
            rec("2026-10-17", "-42.17", None, Some("other date")),
            rec("2026-10-18", "-42.18", None, Some("other amount")),
            usd,
            // Same date and amount as a card transaction: another source.
            rec("2026-10-16", "-7.00", None, Some("card")),
        ] {
            let records = vec![rec("2026-10-15", "-10.00", None, Some("A")), record];
            let plan = f.plan(&statement(records), "2026-10-20", UserBalance::Skipped);
            assert!(
                plan.possible_duplicates.is_empty(),
                "{:?}",
                plan.possible_duplicates
            );
            assert_eq!(plan.transactions.len(), 2);
        }
    }

    #[test]
    fn reproduced_id_is_no_orphan() {
        let records = vec![
            rec("2026-10-15", "-5.00", None, Some("A")),
            rec("2026-10-15", "-5.00", None, Some("A")),
        ];
        let first = id::import_id("giro", &records[0], 0);
        let existing = format!(
            "{}{}",
            history(),
            txn("2026-10-15", '!', &first, "A", "-5.00", "Expenses:Home")
        );
        let f = Fixture::new(&existing);
        let plan = f.plan(
            &statement(records.clone()),
            "2026-10-20",
            UserBalance::Skipped,
        );
        assert_eq!(plan.skipped_existing, 1);
        assert!(plan.possible_duplicates.is_empty());
        assert_eq!(ids(&plan), vec![id::import_id("giro", &records[1], 1)]);
    }

    #[test]
    fn column_balance_in_file_order() {
        let f = Fixture::new(&history());
        let s = statement(vec![
            with_balance(rec("2026-10-15", "-10.00", None, Some("A")), "91.00"),
            with_balance(rec("2026-10-18", "-5.00", None, Some("B")), "86.00"),
            with_balance(rec("2026-10-20", "-1.00", None, Some("C")), "85.00"),
        ]);
        let plan = f.plan(&s, "2026-10-20", UserBalance::NotAsked);
        assert_eq!(plan.balance, assertion("2026-10-19", "86.00"));
        assert!(plan.opening.is_none());
        f.assert_loads(&plan);
    }

    #[test]
    fn column_balance_newest_first() {
        let f = Fixture::new(&history());
        let s = statement(vec![
            with_balance(rec("2026-10-20", "-1.00", None, Some("C")), "85.00"),
            with_balance(rec("2026-10-18", "-5.00", None, Some("B")), "86.00"),
            with_balance(rec("2026-10-15", "-10.00", None, Some("A")), "91.00"),
        ]);
        let plan = f.plan(&s, "2026-10-20", UserBalance::NotAsked);
        assert_eq!(plan.balance, assertion("2026-10-19", "86.00"));
        let order: Vec<_> = plan
            .transactions
            .iter()
            .map(|t| t.narration.as_str())
            .collect();
        assert_eq!(order, vec!["A", "B"]);
        f.assert_loads(&plan);
    }

    #[test]
    fn column_balance_that_fits_neither_order_is_suspect() {
        let f = Fixture::new(&history());
        let s = statement(vec![
            with_balance(rec("2026-10-15", "-10.00", None, Some("A")), "91.00"),
            with_balance(rec("2026-10-18", "-5.00", None, Some("B")), "86.01"),
        ]);
        let plan = f.plan(&s, "2026-10-20", UserBalance::NotAsked);
        assert_eq!(plan.balance, BalanceOutcome::Suspect);
        let content = plan.run_file.unwrap().content;
        assert!(content.ends_with("\n; lentil: unchecked\n"), "{content}");
        assert!(!content.contains(" balance "));
    }

    #[test]
    fn column_balance_with_ambiguous_orders_is_suspect() {
        let f = Fixture::new(&history());
        let s = statement(vec![
            with_balance(rec("2026-10-18", "10.00", None, Some("A")), "110.00"),
            with_balance(rec("2026-10-18", "-10.00", None, Some("B")), "100.00"),
        ]);
        assert_eq!(
            f.plan(&s, "2026-10-20", UserBalance::NotAsked).balance,
            BalanceOutcome::Suspect
        );
        let single = statement(vec![with_balance(
            rec("2026-10-18", "10.00", None, None),
            "110.00",
        )]);
        assert_eq!(
            f.plan(&single, "2026-10-20", UserBalance::NotAsked).balance,
            assertion("2026-10-19", "110.00")
        );
    }

    #[test]
    fn column_balance_missing_on_a_row_is_suspect() {
        let f = Fixture::new(&history());
        let s = statement(vec![
            with_balance(rec("2026-10-15", "-10.00", None, Some("A")), "91.00"),
            rec("2026-10-18", "-5.00", None, Some("B")),
        ]);
        assert_eq!(
            f.plan(&s, "2026-10-20", UserBalance::NotAsked).balance,
            BalanceOutcome::Suspect
        );
    }

    #[test]
    fn column_balance_without_booking_before_run_date_asks_the_user() {
        let f = Fixture::new(&history());
        let s = statement(vec![with_balance(
            rec("2026-10-20", "-1.00", None, None),
            "85.00",
        )]);
        let plan = f.plan(&s, "2026-10-20", UserBalance::NotAsked);
        assert_eq!(plan.balance, BalanceOutcome::NeedsInput);
        assert_eq!(plan.run_file, None);
        let plan = f.plan(
            &s,
            "2026-10-20",
            UserBalance::Given {
                amount: dec("85.00"),
                date: date("2026-10-20"),
            },
        );
        assert_eq!(plan.balance, assertion("2026-10-20", "86.00"));
    }

    #[test]
    fn stated_balance_before_run_date_is_end_of_day() {
        let f = Fixture::new(&history());
        let s = stated(
            statement(vec![
                rec("2026-10-15", "-10.00", None, Some("A")),
                rec("2026-10-18", "-5.00", None, Some("B")),
                rec("2026-10-20", "-1.00", None, Some("C")),
            ]),
            "86.00",
            Some("2026-10-18"),
        );
        let plan = f.plan(&s, "2026-10-20", UserBalance::NotAsked);
        assert_eq!(plan.balance, assertion("2026-10-19", "86.00"));
        f.assert_loads(&plan);
    }

    #[test]
    fn stated_balance_of_the_run_date_subtracts_its_bookings() {
        let f = Fixture::new(&history());
        let records = statement(vec![
            rec("2026-10-18", "-5.00", None, Some("B")),
            rec("2026-10-20", "-1.00", None, Some("C")),
            rec("2026-10-20", "-2.50", None, Some("D")),
        ]);
        for day in [None, Some("2026-10-20"), Some("2026-10-21")] {
            let s = stated(records.clone(), "92.50", day);
            let plan = f.plan(&s, "2026-10-20", UserBalance::NotAsked);
            assert_eq!(plan.balance, assertion("2026-10-20", "96.00"), "{day:?}");
            f.assert_loads(&plan);
        }
    }

    #[test]
    fn user_balance_when_the_export_has_none() {
        let f = Fixture::new(&history());
        let s = statement(vec![
            rec("2026-10-18", "-5.00", None, Some("B")),
            rec("2026-10-20", "-1.00", None, Some("C")),
        ]);
        let plan = f.plan(&s, "2026-10-20", UserBalance::NotAsked);
        assert_eq!(plan.balance, BalanceOutcome::NeedsInput);
        assert_eq!(plan.run_file, None);
        assert_eq!(plan.transactions.len(), 1);

        let plan = f.plan(&s, "2026-10-20", UserBalance::Skipped);
        assert_eq!(plan.balance, BalanceOutcome::Unchecked);
        assert!(
            plan.run_file
                .as_ref()
                .unwrap()
                .content
                .ends_with("\n\n; lentil: unchecked\n")
        );
        f.assert_loads(&plan);

        let given = |amount: &str, day: &str| UserBalance::Given {
            amount: dec(amount),
            date: date(day),
        };
        let plan = f.plan(&s, "2026-10-20", given("95.00", "2026-10-20"));
        assert_eq!(plan.balance, assertion("2026-10-20", "96.00"));
        f.assert_loads(&plan);
        let plan = f.plan(&s, "2026-10-20", given("96.00", "2026-10-19"));
        assert_eq!(plan.balance, assertion("2026-10-20", "96.00"));
        let plan = f.plan(&s, "2026-10-20", given("96.00", "2026-10-18"));
        assert_eq!(plan.balance, assertion("2026-10-19", "96.00"));
        f.assert_loads(&plan);
    }

    #[test]
    fn existing_assertion_on_the_same_date_is_not_repeated() {
        let existing = format!(
            "{}2026-10-19 balance Assets:Bank:Giro  1.00 ~ 0.00 EUR\n",
            history()
        );
        let f = Fixture::new(&existing);
        let s = statement(vec![rec("2026-10-18", "-5.00", None, Some("B"))]);
        let given = |amount: &str| UserBalance::Given {
            amount: dec(amount),
            date: date("2026-10-18"),
        };
        let plan = f.plan(&s, "2026-10-20", given("1.00"));
        assert_eq!(
            plan.balance,
            BalanceOutcome::AlreadyExists {
                date: date("2026-10-19"),
                difference: None
            }
        );
        let plan = f.plan(&s, "2026-10-20", given("-4.00"));
        assert_eq!(
            plan.balance,
            BalanceOutcome::AlreadyExists {
                date: date("2026-10-19"),
                difference: Some(dec("5.00"))
            }
        );
        let content = plan.run_file.unwrap().content;
        assert!(
            !content.contains(" balance ") && !content.contains("unchecked"),
            "{content}"
        );
        // An assertion in another currency is another assertion.
        let mut usd = Fixture::new(&history());
        let mut existing = f.ledger.balance_assertions[0].clone();
        existing.currency = "USD".into();
        usd.ledger.balance_assertions.push(existing);
        assert_eq!(
            usd.plan(&s, "2026-10-20", given("96.00")).balance,
            assertion("2026-10-19", "96.00")
        );
        // Another date is a new assertion.
        let plan = f.plan(
            &s,
            "2026-10-20",
            UserBalance::Given {
                amount: dec("-4.00"),
                date: date("2026-10-20"),
            },
        );
        assert_eq!(plan.balance, assertion("2026-10-20", "-4.00"));
    }

    /// A card transaction: the ledger has history, but none on the giro account.
    const CARD_ONLY: &str = "2026-09-01 * \"x\"\n  lentil_id: \"card-cccccccccccc\"\n  lentil_counterparty: \"CARD\"\n  Assets:Bank:Card  -7.00 EUR\n  Expenses:Home\n";

    #[test]
    fn first_run_books_an_opening_balance() {
        let f = Fixture::new(CARD_ONLY);
        let s = stated(
            statement(vec![
                rec("2026-10-15", "-10.00", None, Some("A")),
                rec("2026-10-18", "-5.00", None, Some("B")),
                rec("2026-10-20", "-1.00", None, Some("C")),
            ]),
            "100.00",
            Some("2026-10-19"),
        );
        let plan = f.plan(&s, "2026-10-20", UserBalance::NotAsked);
        assert_eq!(plan.balance, assertion("2026-10-20", "100.00"));
        assert_eq!(
            plan.opening,
            Some(PlannedTxn {
                id: opening_id("giro"),
                date: date("2026-10-15"),
                payee: None,
                narration: "Opening balance".into(),
                counterparty: None,
                purpose: None,
                booking_type: None,
                reference: None,
                amount: dec("115.00"),
                currency: "EUR".into(),
                counter_account: OPENING_BALANCES_ACCOUNT.into(),
                suggestion: SuggestionKind::Opening,
                tags: Vec::new(),
            })
        );
        let content = &plan.run_file.as_ref().unwrap().content;
        let opening_at = content.find("Opening balance").unwrap();
        let first_record_at = content.find("\"A\"").unwrap();
        assert!(opening_at < first_record_at, "{content}");
        f.assert_loads(&plan);
    }

    #[test]
    fn opening_gets_no_tags() {
        let mut f = Fixture::new(CARD_ONLY);
        f.rules = Rules::parse("[[rule]]\nmatch = { counterparty = \"^$\" }\nadd_tags = [\"x\"]\n")
            .unwrap();
        let s = stated(
            statement(vec![
                rec("2026-10-15", "-10.00", None, Some("A")),
                rec("2026-10-18", "-5.00", None, Some("B")),
                rec("2026-10-20", "-1.00", None, Some("C")),
            ]),
            "100.00",
            Some("2026-10-19"),
        );
        let plan = f.plan(&s, "2026-10-20", UserBalance::NotAsked);
        assert_eq!(plan.opening.as_ref().unwrap().tags, Vec::<String>::new());
        assert!(!plan.transactions.is_empty());
        for t in &plan.transactions {
            assert_eq!(t.tags, vec!["x".to_owned()], "{}", t.narration);
        }
        f.assert_loads(&plan);
    }

    #[test]
    fn opening_counts_only_records_before_the_assertion() {
        let f = Fixture::new("");
        let s = stated(
            statement(vec![
                rec("2026-10-15", "-10.00", None, Some("A")),
                rec("2026-10-18", "-5.00", None, Some("B")),
            ]),
            "100.00",
            Some("2026-10-17"),
        );
        let plan = f.plan(&s, "2026-10-20", UserBalance::NotAsked);
        assert_eq!(plan.balance, assertion("2026-10-18", "100.00"));
        let opening = plan.opening.as_ref().unwrap();
        assert_eq!(
            (opening.date, opening.amount),
            (date("2026-10-15"), dec("110.00"))
        );
        f.assert_loads(&plan);
    }

    #[test]
    fn opening_id_hashes_source_and_opening() {
        assert_eq!(opening_id("giro"), id::hashed_id("giro", "giro|opening"));
        assert!(opening_id("giro").starts_with("giro-"));
        assert_eq!(opening_id("giro").len(), "giro-".len() + 12);
    }

    #[test]
    fn opening_precedes_an_assertion_before_all_records() {
        let f = Fixture::new("");
        let s = stated(
            statement(vec![rec("2026-10-15", "-10.00", None, Some("A"))]),
            "100.00",
            Some("2026-10-01"),
        );
        let plan = f.plan(&s, "2026-10-20", UserBalance::NotAsked);
        assert_eq!(plan.balance, assertion("2026-10-02", "100.00"));
        let opening = plan.opening.as_ref().unwrap();
        assert_eq!(
            (opening.date, opening.amount),
            (date("2026-10-01"), dec("100.00"))
        );
        f.assert_loads(&plan);
    }

    #[test]
    fn opening_without_planned_records_is_dated_before_the_assertion() {
        let f = Fixture::new("");
        let s = stated(
            statement(vec![rec("2026-10-20", "-1.00", None, None)]),
            "99.00",
            None,
        );
        let plan = f.plan(&s, "2026-10-20", UserBalance::NotAsked);
        assert!(plan.transactions.is_empty());
        let opening = plan.opening.as_ref().unwrap();
        assert_eq!(
            (opening.date, opening.amount),
            (date("2026-10-19"), dec("100.00"))
        );
        f.assert_loads(&plan);
    }

    #[test]
    fn a_second_run_within_one_second_takes_the_next_free_name() {
        let mut f = Fixture::new("");
        f.snapshot.insert(RUN_FILE, "; lentil import run\n");
        f.ledger = ledger::load(&f.snapshot, "main.beancount");
        let s = statement(vec![rec("2026-10-15", "-10.00", None, Some("A"))]);
        let plan = f.plan(&s, "2026-10-20", UserBalance::Skipped);
        let run = plan.run_file.unwrap();
        assert_eq!(run.path, "imports/giro/2026/20261020T071503Z.beancount");
        assert!(
            run.content.contains("started: 2026-10-20T07:15:03Z"),
            "{}",
            run.content
        );
    }

    #[test]
    fn no_opening_when_zero_or_unchecked() {
        let records = vec![rec("2026-10-15", "-10.00", None, Some("A"))];
        let f = Fixture::new("");
        let zero = stated(statement(records.clone()), "-10.00", Some("2026-10-19"));
        let plan = f.plan(&zero, "2026-10-20", UserBalance::NotAsked);
        assert!(matches!(plan.balance, BalanceOutcome::Assertion { .. }));
        assert_eq!(plan.opening, None);
        f.assert_loads(&plan);

        let plan = f.plan(&statement(records), "2026-10-20", UserBalance::Skipped);
        assert_eq!(plan.opening, None);
    }

    #[test]
    fn first_checked_run_after_unchecked_runs_books_the_opening() {
        // The first run skipped the balance; the opening covers its records too.
        let f = Fixture::new(&unchecked_history());
        let s = stated(
            statement(vec![rec("2026-10-15", "-10.00", None, Some("A"))]),
            "50.00",
            Some("2026-10-19"),
        );
        let plan = f.plan(&s, "2026-10-20", UserBalance::NotAsked);
        let opening = plan.opening.as_ref().unwrap();
        assert_eq!(
            (opening.date, opening.amount),
            (date("2026-01-01"), dec("-41.00"))
        );
        f.assert_loads(&plan);
    }

    #[test]
    fn no_opening_after_an_earlier_assertion_or_an_existing_opening() {
        let s = stated(
            statement(vec![rec("2026-10-15", "-10.00", None, Some("A"))]),
            "50.00",
            Some("2026-10-19"),
        );
        let f = Fixture::new(&history());
        assert_eq!(
            f.plan(&s, "2026-10-20", UserBalance::NotAsked).opening,
            None
        );

        let first = Fixture::new("").plan(&s, "2026-10-20", UserBalance::NotAsked);
        let opening = first.opening.as_ref().unwrap();
        let block = txn(
            "2026-10-15",
            '*',
            &opening.id,
            OPENING_BALANCE_TEXT,
            "60.00",
            OPENING_BALANCES_ACCOUNT,
        );
        let dismissed: String = block.lines().map(|l| format!("; {l}\n")).collect();
        for existing in [block, dismissed] {
            let f = Fixture::new(&existing);
            assert_eq!(
                f.plan(&s, "2026-10-20", UserBalance::NotAsked).opening,
                None
            );
        }
    }

    #[test]
    fn opening_is_not_an_orphan_for_duplicate_detection() {
        let records = vec![
            rec("2026-10-15", "-10.00", None, Some("A")),
            rec("2026-10-18", "-5.00", None, Some("B")),
        ];
        let s = stated(statement(records.clone()), "100.00", Some("2026-10-19"));
        let first = Fixture::new("").plan(&s, "2026-10-20", UserBalance::NotAsked);
        let run = first.run_file.unwrap();
        // The second run's export has a record equal to the opening in date and amount.
        let f = Fixture::new(&run.content);
        let again = statement(vec![
            records[0].clone(),
            rec("2026-10-15", "115.00", None, Some("Refund")),
            records[1].clone(),
        ]);
        let plan = f.plan(&again, "2026-10-20", UserBalance::Skipped);
        assert!(plan.possible_duplicates.is_empty());
        assert_eq!(plan.skipped_existing, 2);
        assert_eq!(plan.transactions.len(), 1);
    }

    #[test]
    fn nothing_new_writes_no_file() {
        let records = vec![rec("2026-10-18", "-5.00", None, Some("B"))];
        let first = Fixture::new(&history()).plan(
            &statement(records.clone()),
            "2026-10-20",
            UserBalance::Skipped,
        );
        let f = Fixture::new(&format!("{}{}", history(), first.run_file.unwrap().content));
        let plan = f.plan(
            &statement(records.clone()),
            "2026-10-20",
            UserBalance::Skipped,
        );
        assert_eq!(plan.skipped_existing, 1);
        assert_eq!(plan.balance, BalanceOutcome::Unchecked);
        assert_eq!(plan.run_file, None);
        let plan = f.plan(&statement(Vec::new()), "2026-10-20", UserBalance::Skipped);
        assert_eq!(plan.run_file, None);
        // A new assertion alone is new.
        let plan = f.plan(
            &statement(records),
            "2026-10-20",
            UserBalance::Given {
                amount: dec("96.00"),
                date: date("2026-10-19"),
            },
        );
        assert_eq!(plan.balance, assertion("2026-10-20", "96.00"));
        let run = plan.run_file.as_ref().unwrap();
        assert!(
            run.content
                .ends_with("\n\n2026-10-20 balance Assets:Bank:Giro  96.00 ~ 0.00 EUR\n")
        );
        f.assert_loads(&plan);
    }
}
