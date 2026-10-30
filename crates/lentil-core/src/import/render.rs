//! Rendering run files in the layout of spec section 8.

use jiff::Timestamp;

use super::plan::{BalanceOutcome, PlannedTxn};

/// `imports/<source-id>/<YYYY>/<YYYYMMDDTHHMMSSZ>.beancount` for a run started at `started` (UTC).
#[must_use]
pub fn run_path(source_id: &str, started: Timestamp) -> String {
    let _ = (source_id, started);
    todo!("task B")
}

/// The run file content: header comments, transactions with flag `!`, and the
/// balance assertion with `~ 0.00` or the `; lentil: unchecked` comment.
#[must_use]
pub fn render_run(
    source_id: &str,
    account: &str,
    started: Timestamp,
    client: &str,
    transactions: &[PlannedTxn],
    balance: &BalanceOutcome,
) -> String {
    let _ = (source_id, account, started, client, transactions, balance);
    todo!("task B")
}

/// Escapes a Beancount string literal's content (`\` and `"`).
#[must_use]
pub fn escape(text: &str) -> String {
    text.replace('\\', "\\\\").replace('"', "\\\"")
}
