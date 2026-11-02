//! The import pipeline (spec sections 8, 15 and 16).
//!
//! An adapter turns a bank export into a [`Statement`] of [`Record`]s.
//! Everything after that works on records only: IDs, duplicate checks,
//! suggestions, the balance assertion and the run file.

pub mod csv;
pub mod inputs;
pub mod mapping;
pub mod plan;
pub mod render;
pub mod rules;

use jiff::civil::Date;
use rust_decimal::Decimal;

pub use csv::read_statement;
pub use inputs::{InputProblem, check_inputs};
pub use mapping::{Mapping, MappingError};
pub use plan::{
    BalanceOutcome, ImportPlan, ImportRequest, PlannedTxn, PossibleDuplicate, RunFile,
    SuggestionKind, UserBalance, opening_id, plan_import,
};
pub use rules::{Learned, Original, RuleMatch, Rules, RulesError};

/// One booked transaction from a bank export (spec section 15).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Record {
    /// Date the bank booked the transaction.
    pub booking_date: Date,
    /// Signed amount, negative when money leaves the account.
    pub amount: Decimal,
    /// From a column, otherwise the source's currency.
    pub currency: String,
    /// The other party, as the bank writes it.
    pub counterparty: Option<String>,
    /// Purpose, as the bank writes it.
    pub purpose: Option<String>,
    /// Booking type, such as direct debit or card payment, as the bank writes it.
    pub booking_type: Option<String>,
    /// The bank's unique reference.
    pub reference: Option<String>,
    /// Running balance after this booking.
    pub balance_after: Option<Decimal>,
    /// 1-based line in the export, for error messages.
    pub line: usize,
}

/// A balance an export states once, with the date it refers to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StatedBalance {
    /// The balance.
    pub amount: Decimal,
    /// The date it refers to; `None` means the run date (spec section 15).
    pub date: Option<Date>,
}

/// The records of one export, in the order the file lists them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Statement {
    /// Records in file order.
    pub records: Vec<Record>,
    /// Balance from `from = "text"`, if the mapping uses it.
    pub stated_balance: Option<StatedBalance>,
    /// Rows skipped by `[[skip]]` entries.
    pub skipped_rows: usize,
}

/// Why an export cannot be imported. Codes, not text: each client translates.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ImportError {
    /// The file is not valid in the mapping's encoding.
    #[error("export is not valid {0}")]
    Encoding(String),
    /// No row contains every cell of `header`.
    #[error("header row not found")]
    HeaderNotFound,
    /// A column named by the mapping is missing from the header.
    #[error("column {0:?} not found")]
    ColumnNotFound(String),
    /// A row neither parses nor matches a `[[skip]]` entry.
    #[error("line {line}: cannot parse {field}: {value:?}")]
    RowInvalid {
        /// 1-based line in the export.
        line: usize,
        /// Which field failed.
        field: String,
        /// The cell content.
        value: String,
    },
    /// The CSV structure is broken (quotes, field counts).
    #[error("line {line}: malformed CSV")]
    Malformed {
        /// 1-based line in the export.
        line: usize,
    },
    /// `from = "text"` found no balance line.
    #[error("balance text not found")]
    BalanceNotFound,
    /// Two rows of one export carry the same reference, so the reference
    /// column cannot identify transactions (spec section 5).
    #[error("line {line}: reference {reference:?} repeats line {first_line}")]
    DuplicateReference {
        /// 1-based line of the repeated reference.
        line: usize,
        /// 1-based line where the reference first appears.
        first_line: usize,
        /// The reference.
        reference: String,
    },
}
