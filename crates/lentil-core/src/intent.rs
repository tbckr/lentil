//! Edit operations as intents, and their replay (spec section 10).
//!
//! An intent names a transaction by `lentil_id` and carries the value the user
//! saw, so it can be re-applied to a newer repository state: if the current
//! value already equals the new value it is done, if it equals the old value it
//! is applied, otherwise it is a conflict.

use jiff::civil::Date;
use serde::{Deserialize, Serialize};

use crate::config::Config;
use crate::snapshot::Snapshot;

/// One edit operation of spec section 10 (v0.1 subset).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "kebab-case")]
pub enum Intent {
    /// Set the flag of an active transaction to `*`.
    Confirm {
        /// `lentil_id` of the transaction.
        id: String,
    },
    /// Change the account of a posting that is not the source posting.
    SetAccount {
        /// `lentil_id` of the transaction.
        id: String,
        /// 0-based index of the posting in file order.
        posting: usize,
        /// The account the user saw.
        old: String,
        /// The account the user chose; must be open on the transaction date.
        new: String,
    },
    /// Comment out an active transaction in place.
    Dismiss {
        /// `lentil_id` of the transaction.
        id: String,
    },
    /// Append `<date> open <account>` to `accounts.beancount`.
    OpenAccount {
        /// Account to open.
        account: String,
        /// Opening date.
        date: Date,
    },
    /// Write a new import run file.
    AddImportRun {
        /// Source ID.
        source: String,
        /// Repository-relative path of the run file.
        file: String,
        /// Complete content of the run file.
        content: String,
    },
}

impl Intent {
    /// The operation name as used in commit subjects, such as `set-account`.
    #[must_use]
    pub fn op_name(&self) -> &'static str {
        match self {
            Self::Confirm { .. } => "confirm",
            Self::SetAccount { .. } => "set-account",
            Self::Dismiss { .. } => "dismiss",
            Self::OpenAccount { .. } => "open-account",
            Self::AddImportRun { .. } => "add-import-run",
        }
    }

    /// The transaction the intent addresses, if any.
    #[must_use]
    pub fn transaction_id(&self) -> Option<&str> {
        match self {
            Self::Confirm { id } | Self::SetAccount { id, .. } | Self::Dismiss { id } => Some(id),
            Self::OpenAccount { .. } | Self::AddImportRun { .. } => None,
        }
    }
}

/// An intent stored on the device until it is pushed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingIntent {
    /// Unique key on the device, used to report outcomes and remove the intent.
    pub key: String,
    /// The operation.
    pub intent: Intent,
}

/// What replaying one intent did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// The precondition held and the change was made.
    Applied,
    /// The current state already equals the intended one; nothing changed.
    AlreadyApplied,
    /// Neither old nor new value matches; the intent leaves the queue and is
    /// shown to the user (spec section 11, step 3).
    Conflict(Conflict),
}

/// Why an intent could not be applied.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Conflict {
    /// The reason, translated by the client.
    pub code: ConflictCode,
    /// The value found instead of the expected old value, if any.
    pub current: Option<String>,
}

/// Reasons an intent conflicts.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ConflictCode {
    /// No active or dismissed transaction has the `lentil_id`.
    TransactionNotFound,
    /// The transaction was dismissed in the meantime.
    TransactionDismissed,
    /// The posting's account is neither the old nor the new value.
    AccountChanged,
    /// The posting index does not exist.
    PostingNotFound,
    /// The posting is the source posting, which editors must not change.
    SourcePosting,
    /// The new account is not open on the transaction date.
    AccountNotOpen,
    /// The transaction header uses a form the core cannot edit byte-precisely.
    NotEditable,
    /// The run file already exists with different content.
    FileExists,
    /// The account name is invalid.
    InvalidAccount,
}

/// The result of replaying intents on a snapshot.
#[derive(Clone, Debug)]
pub struct ReplayResult {
    /// The snapshot with all applied intents.
    pub snapshot: Snapshot,
    /// One outcome per input intent, keyed by [`PendingIntent::key`], in input order.
    pub outcomes: Vec<(String, Outcome)>,
}

/// Applies `intents` in order to `snapshot` (spec section 10).
///
/// Every applied intent changes only the bytes of the affected element; the
/// rest of each file stays byte-identical.
#[must_use]
pub fn replay(snapshot: &Snapshot, config: &Config, intents: &[PendingIntent]) -> ReplayResult {
    let _ = (snapshot, config, intents);
    todo!("replay: implemented in task A")
}

/// Commit subject summary of applied intents, such as
/// `confirm 12, set-account 3, dismiss 1` (spec section 12).
///
/// Operations appear in the order of the spec table; intents that were already
/// applied or conflicted are not counted.
#[must_use]
pub fn summary(intents: &[PendingIntent], outcomes: &[(String, Outcome)]) -> String {
    let _ = (intents, outcomes);
    todo!("summary: implemented in task A")
}
