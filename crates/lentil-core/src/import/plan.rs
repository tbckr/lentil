//! Planning one import run (spec section 8): IDs, duplicates, suggestions,
//! balance assertion and the run file. Pure: the CLI calls it again with the
//! user's answers until nothing is left to ask.

use std::collections::BTreeSet;

use jiff::Timestamp;
use jiff::civil::Date;
use rust_decimal::Decimal;

use super::{Rules, Statement};
use crate::config::{Config, SourceConfig};
use crate::ledger::Ledger;

/// Counter account of the opening balance transaction of a source's first run.
pub const OPENING_BALANCE_ACCOUNT: &str = "Equity:Opening-Balances";

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
    /// Payee: the normalized counterparty, if any.
    pub payee: Option<String>,
    /// Narration: the normalized purpose, or empty.
    pub narration: String,
    /// `lentil_desc`.
    pub desc: String,
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
}

/// A new record that may duplicate an existing transaction whose ID the
/// export no longer reproduces (PRD FR-C7).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PossibleDuplicate {
    /// The new record's ID.
    pub new_id: String,
    /// The new record's line in the export.
    pub line: usize,
    /// The new record's description.
    pub desc: String,
    /// Booking date shared by both.
    pub date: Date,
    /// Amount shared by both.
    pub amount: Decimal,
    /// Currency shared by both.
    pub currency: String,
    /// The existing transaction's ID.
    pub existing_id: String,
    /// The existing transaction's description.
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

/// Plans an import run.
#[must_use]
pub fn plan_import(request: &ImportRequest<'_>) -> ImportPlan {
    let _ = request;
    todo!("task B")
}
