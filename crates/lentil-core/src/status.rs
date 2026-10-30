//! `lentil status` (PRD FR-E2): unreviewed transactions, failed balance
//! assertions and import runs without a balance check.

use crate::config::Config;
use crate::ledger::{BalanceFailure, Ledger};

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

/// Computes the status report. `files` gives the run files' text.
#[must_use]
pub fn status(config: &Config, ledger: &Ledger, snapshot: &crate::snapshot::Snapshot) -> Status {
    let _ = (config, ledger, snapshot);
    todo!("task C")
}
