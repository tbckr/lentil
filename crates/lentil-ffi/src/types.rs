//! FFI records and enums, and their conversions from and to the core's types.

use std::collections::HashMap;

use jiff::civil::Date;
use lentil_core::finding::{Finding, FindingCode, Severity};
use lentil_core::intent::{ConflictCode, Intent, Outcome, PendingIntent};
use lentil_core::sync::{Author, Credentials};
use lentil_core::tags::{ChoiceSource, TagChoice, TagChoices};

use crate::error::LentilError;

/// One edit operation of spec section 10, as the app records it.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum FfiIntent {
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
        posting: u32,
        /// The account the user saw.
        old: String,
        /// The account the user chose.
        new: String,
    },
    /// Comment out an active transaction in place.
    Dismiss {
        /// `lentil_id` of the transaction.
        id: String,
    },
    /// Open an account in `accounts.beancount`.
    OpenAccount {
        /// Account to open.
        account: String,
        /// Opening date as `YYYY-MM-DD`.
        date: String,
    },
    /// Add a tag to an active transaction; it never confirms it.
    AddTag {
        /// `lentil_id` of the transaction.
        id: String,
        /// The tag, without `#`.
        tag: String,
    },
    /// Remove a tag from an active transaction; it never confirms it.
    RemoveTag {
        /// `lentil_id` of the transaction.
        id: String,
        /// The tag, without `#`.
        tag: String,
    },
    /// Start a trip: an `event "trip"` in the device's month file.
    StartTrip {
        /// `lentil_id` of the event, from `new_manual_id`.
        id: String,
        /// Start date as `YYYY-MM-DD`, usually today.
        date: String,
        /// The trip's name, matching `[a-z0-9-]+` (see `is_plain_name`).
        name: String,
    },
    /// End the running trip: an empty `event "trip"` in the device's month file.
    EndTrip {
        /// `lentil_id` of the event, from `new_manual_id`.
        id: String,
        /// End date as `YYYY-MM-DD`, usually today.
        date: String,
    },
}

/// An intent stored on the device until a sync pushes it.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct FfiPendingIntent {
    /// Unique key on the device.
    pub key: String,
    /// The operation.
    pub intent: FfiIntent,
}

impl TryFrom<FfiPendingIntent> for PendingIntent {
    type Error = LentilError;

    fn try_from(pending: FfiPendingIntent) -> Result<Self, Self::Error> {
        let intent = match pending.intent {
            FfiIntent::Confirm { id } => Intent::Confirm { id },
            FfiIntent::SetAccount {
                id,
                posting,
                old,
                new,
            } => Intent::SetAccount {
                id,
                posting: usize::try_from(posting).map_err(|_| LentilError::Other {
                    detail: format!("posting index {posting} out of range"),
                })?,
                old,
                new,
            },
            FfiIntent::Dismiss { id } => Intent::Dismiss { id },
            FfiIntent::OpenAccount { account, date } => Intent::OpenAccount {
                account,
                date: parse_date(&date, &pending.key)?,
            },
            FfiIntent::AddTag { id, tag } => Intent::AddTag { id, tag },
            FfiIntent::RemoveTag { id, tag } => Intent::RemoveTag { id, tag },
            FfiIntent::StartTrip { id, date, name } => Intent::StartTrip {
                id,
                date: parse_date(&date, &pending.key)?,
                name,
            },
            FfiIntent::EndTrip { id, date } => Intent::EndTrip {
                id,
                date: parse_date(&date, &pending.key)?,
            },
        };
        Ok(Self {
            key: pending.key,
            intent,
        })
    }
}

/// Parses the `YYYY-MM-DD` date of the intent with `key`.
fn parse_date(date: &str, key: &str) -> Result<Date, LentilError> {
    date.parse::<Date>().map_err(|_| LentilError::Other {
        detail: format!("invalid date {date:?} in intent {key}"),
    })
}

impl TryFrom<Intent> for FfiIntent {
    type Error = LentilError;

    /// An intent the core computed for the app to record, such as those of
    /// a tag pick. The app never records `set-payee` or `add-import-run`.
    fn try_from(intent: Intent) -> Result<Self, Self::Error> {
        Ok(match intent {
            Intent::Confirm { id } => Self::Confirm { id },
            Intent::SetAccount {
                id,
                posting,
                old,
                new,
            } => Self::SetAccount {
                id,
                posting: u32::try_from(posting).map_err(|_| LentilError::Other {
                    detail: format!("posting index {posting} out of range"),
                })?,
                old,
                new,
            },
            Intent::Dismiss { id } => Self::Dismiss { id },
            Intent::OpenAccount { account, date } => Self::OpenAccount {
                account,
                date: date.to_string(),
            },
            Intent::AddTag { id, tag } => Self::AddTag { id, tag },
            Intent::RemoveTag { id, tag } => Self::RemoveTag { id, tag },
            Intent::StartTrip { id, date, name } => Self::StartTrip {
                id,
                date: date.to_string(),
                name,
            },
            Intent::EndTrip { id, date } => Self::EndTrip {
                id,
                date: date.to_string(),
            },
            other @ (Intent::SetPayee { .. }
            | Intent::AddImportRun { .. }
            | Intent::SuggestAccount { .. }
            | Intent::AddRule { .. }) => {
                return Err(LentilError::Other {
                    detail: format!("the app does not record {}", other.op_name()),
                });
            }
        })
    }
}

/// Converts pending intents in order, failing on the first invalid one.
pub(crate) fn pending_intents(
    pending: Vec<FfiPendingIntent>,
) -> Result<Vec<PendingIntent>, LentilError> {
    pending.into_iter().map(PendingIntent::try_from).collect()
}

/// Credentials for the remote. Never stored in the repository, and never
/// printed: `Debug` hides the secrets.
#[derive(Clone, PartialEq, Eq, uniffi::Enum)]
pub enum FfiCredentials {
    /// No explicit credentials.
    Default,
    /// HTTPS user name and password or token.
    UserPassword {
        /// User name.
        username: String,
        /// Password or token.
        password: String,
    },
    /// An SSH private key in memory.
    SshKey {
        /// SSH user, usually `git`.
        username: String,
        /// PEM or OpenSSH private key.
        private_key: String,
        /// Passphrase, if the key has one.
        passphrase: Option<String>,
    },
}

impl std::fmt::Debug for FfiCredentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Default => "FfiCredentials::Default",
            Self::UserPassword { .. } => "FfiCredentials::UserPassword(..)",
            Self::SshKey { .. } => "FfiCredentials::SshKey(..)",
        })
    }
}

impl From<FfiCredentials> for Credentials {
    fn from(credentials: FfiCredentials) -> Self {
        match credentials {
            FfiCredentials::Default => Self::Default,
            FfiCredentials::UserPassword { username, password } => {
                Self::UserPassword { username, password }
            }
            FfiCredentials::SshKey {
                username,
                private_key,
                passphrase,
            } => Self::SshKey {
                username,
                private_key,
                passphrase,
            },
        }
    }
}

/// The device's trailers: `Lentil-Client` and `Lentil-Device`. The role is
/// always editor.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct FfiIdentity {
    /// `Lentil-Client`, such as `lentil-android/0.1.0`.
    pub client: String,
    /// `Lentil-Device`, the device ID.
    pub device: String,
}

/// Commit author from the app's settings.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct FfiAuthor {
    /// Name.
    pub name: String,
    /// E-mail address.
    pub email: String,
}

impl From<FfiAuthor> for Author {
    fn from(author: FfiAuthor) -> Self {
        Self {
            name: author.name,
            email: author.email,
        }
    }
}

/// Whether a finding blocks writing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum FfiSeverity {
    /// Stops the commit.
    Blocking,
    /// Reported, never blocks.
    Warning,
}

/// A validation finding. The app translates `code` and inserts `args`.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct FfiFinding {
    /// The name of the core's finding code, such as `BalanceAssertionFailed`.
    pub code: String,
    /// Whether it blocks writing.
    pub severity: FfiSeverity,
    /// Repository-relative file, if known.
    pub file: Option<String>,
    /// 1-based line in `file`.
    pub line: Option<u32>,
    /// Named arguments of the code, such as `account`.
    pub args: HashMap<String, String>,
}

impl From<Finding> for FfiFinding {
    fn from(finding: Finding) -> Self {
        Self {
            code: finding_code_name(finding.code).to_owned(),
            severity: match finding.severity {
                Severity::Blocking => FfiSeverity::Blocking,
                Severity::Warning => FfiSeverity::Warning,
            },
            file: finding.file,
            line: finding.line,
            args: finding.args.into_iter().collect(),
        }
    }
}

/// Converts findings in order.
pub(crate) fn findings(findings: Vec<Finding>) -> Vec<FfiFinding> {
    findings.into_iter().map(FfiFinding::from).collect()
}

/// The stable name of a finding code. Spelled out, so that a new code fails
/// to compile here until it is named, and the app gets a translation for it.
#[must_use]
pub fn finding_code_name(code: FindingCode) -> &'static str {
    match code {
        FindingCode::ConfigInvalid => "ConfigInvalid",
        FindingCode::SpecVersionUnsupported => "SpecVersionUnsupported",
        FindingCode::ParseError => "ParseError",
        FindingCode::TransactionUnbalanced => "TransactionUnbalanced",
        FindingCode::AccountNotOpen => "AccountNotOpen",
        FindingCode::DuplicateLentilId => "DuplicateLentilId",
        FindingCode::OutsideWriteArea => "OutsideWriteArea",
        FindingCode::OutsideProfile => "OutsideProfile",
        FindingCode::MainFileMismatch => "MainFileMismatch",
        FindingCode::FavaFileMismatch => "FavaFileMismatch",
        FindingCode::IncludeOutsideRepository => "IncludeOutsideRepository",
        FindingCode::MissingZeroTolerance => "MissingZeroTolerance",
        FindingCode::ManualPostingOnSourceAccount => "ManualPostingOnSourceAccount",
        FindingCode::PadOnSourceAccount => "PadOnSourceAccount",
        FindingCode::ProtectedMetadataChanged => "ProtectedMetadataChanged",
        FindingCode::ProtectedTransactionChanged => "ProtectedTransactionChanged",
        FindingCode::StrayLedgerFile => "StrayLedgerFile",
        FindingCode::InvalidPath => "InvalidPath",
        FindingCode::LoadCacheCommitted => "LoadCacheCommitted",
        FindingCode::CoreError => "CoreError",
        FindingCode::BalanceAssertionFailed => "BalanceAssertionFailed",
    }
}

/// The stable name of a conflict code, spelled out like [`finding_code_name`].
#[must_use]
pub fn conflict_code_name(code: ConflictCode) -> &'static str {
    match code {
        ConflictCode::TransactionNotFound => "TransactionNotFound",
        ConflictCode::TransactionDismissed => "TransactionDismissed",
        ConflictCode::AccountChanged => "AccountChanged",
        ConflictCode::PayeeChanged => "PayeeChanged",
        ConflictCode::PostingNotFound => "PostingNotFound",
        ConflictCode::SourcePosting => "SourcePosting",
        ConflictCode::AccountNotOpen => "AccountNotOpen",
        ConflictCode::NotEditable => "NotEditable",
        ConflictCode::FileExists => "FileExists",
        ConflictCode::InvalidAccount => "InvalidAccount",
        ConflictCode::InvalidTag => "InvalidTag",
        ConflictCode::InvalidTrip => "InvalidTrip",
        ConflictCode::RulesInvalid => "RulesInvalid",
    }
}

/// What replaying one intent did.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum FfiOutcome {
    /// The change was made; remove the intent.
    Applied,
    /// The state already was the intended one; remove the intent.
    AlreadyApplied,
    /// Move the intent to the conflict list and show it.
    Conflict {
        /// The name of the core's conflict code, such as `AccountChanged`.
        code: String,
        /// The value found instead of the expected old value, if any.
        current: Option<String>,
    },
}

impl From<Outcome> for FfiOutcome {
    fn from(outcome: Outcome) -> Self {
        match outcome {
            Outcome::Applied => Self::Applied,
            Outcome::AlreadyApplied => Self::AlreadyApplied,
            Outcome::Conflict(conflict) => Self::Conflict {
                code: conflict_code_name(conflict.code).to_owned(),
                current: conflict.current,
            },
        }
    }
}

/// The outcome of one pending intent.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct IntentOutcome {
    /// The intent's key.
    pub key: String,
    /// What happened to it.
    pub outcome: FfiOutcome,
}

/// Converts outcomes in order.
pub(crate) fn outcomes(outcomes: Vec<(String, Outcome)>) -> Vec<IntentOutcome> {
    outcomes
        .into_iter()
        .map(|(key, outcome)| IntentOutcome {
            key,
            outcome: outcome.into(),
        })
        .collect()
}

/// The result of a sync.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct SyncReport {
    /// One outcome per pending intent, in input order.
    pub outcomes: Vec<IntentOutcome>,
    /// Whether a commit was pushed; `false` if nothing changed.
    pub committed: bool,
    /// Warnings of the committed state, such as failing balance assertions.
    pub warnings: Vec<FfiFinding>,
}

/// One open transaction of the review queue.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct QueueItem {
    /// `lentil_id`; without one the transaction is read-only (spec section 5).
    pub id: Option<String>,
    /// Date as `YYYY-MM-DD`.
    pub date: String,
    /// Payee, if any.
    pub payee: Option<String>,
    /// Narration.
    pub narration: String,
    /// The bank's original text: `lentil_counterparty` and `lentil_purpose`,
    /// joined by one space.
    pub desc: Option<String>,
    /// Number of the source posting, as written, such as `-12.50`.
    pub amount: Option<String>,
    /// Currency of the source posting.
    pub currency: Option<String>,
    /// The posting's account on a source of `lentil.toml`.
    pub source_account: Option<String>,
    /// The suggested counter account, if the transaction has exactly one
    /// posting besides the source posting.
    pub counter_account: Option<String>,
    /// 0-based index of the counter posting, for `SetAccount`.
    pub counter_posting: Option<u32>,
    /// Repository-relative file.
    pub file: String,
    /// 1-based line of the header.
    pub line: u32,
    /// Its tags without `#`, in file order.
    pub tags: Vec<String>,
    /// The tag of the trip its date lies in, suggested until the user
    /// accepts it through `pick_tag` (spec section 9); never written by
    /// itself.
    pub suggested_tag: Option<String>,
}

/// Where the tag picker's choices come from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum FfiChoiceSource {
    /// From `tags.toml`.
    Taxonomy,
    /// No `tags.toml`: the tags the ledger uses. The app shows a hint.
    Missing,
    /// `tags.toml` does not parse: the tags the ledger uses. The app shows a hint.
    Invalid,
}

impl From<ChoiceSource> for FfiChoiceSource {
    fn from(source: ChoiceSource) -> Self {
        match source {
            ChoiceSource::Taxonomy => Self::Taxonomy,
            ChoiceSource::Missing => Self::Missing,
            ChoiceSource::Invalid => Self::Invalid,
        }
    }
}

/// A tag the picker offers.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct FfiTagChoice {
    /// The tag, without `#`.
    pub tag: String,
    /// The transaction's tags that picking it replaces, for "replaces #…".
    pub replaces: Vec<String>,
}

impl From<TagChoice> for FfiTagChoice {
    fn from(choice: TagChoice) -> Self {
        Self {
            tag: choice.tag,
            replaces: choice.replaces,
        }
    }
}

/// What the tag picker offers for one transaction.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct FfiTagChoices {
    /// Tags the transaction does not carry yet, sorted.
    pub tags: Vec<FfiTagChoice>,
    /// Family prefixes such as `share-`: the user types the rest.
    pub families: Vec<String>,
    /// Where the choices come from.
    pub source: FfiChoiceSource,
}

impl From<TagChoices> for FfiTagChoices {
    fn from(choices: TagChoices) -> Self {
        Self {
            tags: choices.tags.into_iter().map(FfiTagChoice::from).collect(),
            families: choices.families,
            source: choices.source.into(),
        }
    }
}

/// An opened account of the ledger, for the account picker.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct AccountItem {
    /// Account name.
    pub name: String,
    /// Its open date as `YYYY-MM-DD`: a counter posting of a transaction on
    /// or after it may move to the account. `None` if no counter posting may,
    /// because the account is closed or a source account (spec section 7).
    pub open_from: Option<String>,
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use lentil_core::intent::Conflict;
    use lentil_core::tags::{ChoiceSource, TagChoice, TagChoices};

    use super::*;

    #[test]
    fn finding_keeps_code_severity_location_and_args() {
        let finding = Finding::warning(FindingCode::BalanceAssertionFailed)
            .at("imports/giro/2026/20261020T071502Z.beancount", Some(12))
            .arg("account", "Assets:Bank:Giro")
            .arg("difference", "0.01");

        let ffi = FfiFinding::from(finding);

        assert_eq!(ffi.code, "BalanceAssertionFailed");
        assert_eq!(ffi.severity, FfiSeverity::Warning);
        assert_eq!(
            ffi.file.as_deref(),
            Some("imports/giro/2026/20261020T071502Z.beancount")
        );
        assert_eq!(ffi.line, Some(12));
        assert_eq!(
            ffi.args,
            HashMap::from([
                ("account".to_owned(), "Assets:Bank:Giro".to_owned()),
                ("difference".to_owned(), "0.01".to_owned()),
            ])
        );
    }

    #[test]
    fn blocking_finding_without_location() {
        let ffi = FfiFinding::from(Finding::blocking(FindingCode::ParseError));

        assert_eq!(ffi.code, "ParseError");
        assert_eq!(ffi.severity, FfiSeverity::Blocking);
        assert_eq!(ffi.file, None);
        assert_eq!(ffi.line, None);
        assert!(ffi.args.is_empty());
    }

    #[test]
    fn code_names_are_the_variant_names() {
        // The app's translations are keyed by these names; Debug spells the
        // variant, so a renamed variant shows up here.
        for code in [
            FindingCode::ConfigInvalid,
            FindingCode::SpecVersionUnsupported,
            FindingCode::ParseError,
            FindingCode::TransactionUnbalanced,
            FindingCode::AccountNotOpen,
            FindingCode::DuplicateLentilId,
            FindingCode::OutsideWriteArea,
            FindingCode::OutsideProfile,
            FindingCode::MainFileMismatch,
            FindingCode::FavaFileMismatch,
            FindingCode::IncludeOutsideRepository,
            FindingCode::MissingZeroTolerance,
            FindingCode::ManualPostingOnSourceAccount,
            FindingCode::PadOnSourceAccount,
            FindingCode::ProtectedMetadataChanged,
            FindingCode::ProtectedTransactionChanged,
            FindingCode::StrayLedgerFile,
            FindingCode::InvalidPath,
            FindingCode::LoadCacheCommitted,
            FindingCode::CoreError,
            FindingCode::BalanceAssertionFailed,
        ] {
            assert_eq!(finding_code_name(code), format!("{code:?}"));
        }
        for code in [
            ConflictCode::TransactionNotFound,
            ConflictCode::TransactionDismissed,
            ConflictCode::AccountChanged,
            ConflictCode::PayeeChanged,
            ConflictCode::PostingNotFound,
            ConflictCode::SourcePosting,
            ConflictCode::AccountNotOpen,
            ConflictCode::NotEditable,
            ConflictCode::FileExists,
            ConflictCode::InvalidAccount,
            ConflictCode::InvalidTag,
            ConflictCode::InvalidTrip,
            ConflictCode::RulesInvalid,
        ] {
            assert_eq!(conflict_code_name(code), format!("{code:?}"));
        }
    }

    #[test]
    fn outcomes_keep_keys_order_and_conflict_details() {
        let converted = outcomes(vec![
            ("a".to_owned(), Outcome::Applied),
            ("b".to_owned(), Outcome::AlreadyApplied),
            (
                "c".to_owned(),
                Outcome::Conflict(Conflict {
                    code: ConflictCode::AccountChanged,
                    current: Some("Expenses:Food".to_owned()),
                }),
            ),
        ]);

        assert_eq!(
            converted,
            vec![
                IntentOutcome {
                    key: "a".to_owned(),
                    outcome: FfiOutcome::Applied,
                },
                IntentOutcome {
                    key: "b".to_owned(),
                    outcome: FfiOutcome::AlreadyApplied,
                },
                IntentOutcome {
                    key: "c".to_owned(),
                    outcome: FfiOutcome::Conflict {
                        code: "AccountChanged".to_owned(),
                        current: Some("Expenses:Food".to_owned()),
                    },
                },
            ]
        );
    }

    #[test]
    fn intents_convert_to_core_intents() {
        let pending = vec![
            FfiPendingIntent {
                key: "k1".to_owned(),
                intent: FfiIntent::Confirm {
                    id: "giro-3f9a1c2e7b4d".to_owned(),
                },
            },
            FfiPendingIntent {
                key: "k2".to_owned(),
                intent: FfiIntent::OpenAccount {
                    account: "Expenses:Garden".to_owned(),
                    date: "2026-01-02".to_owned(),
                },
            },
            FfiPendingIntent {
                key: "k3".to_owned(),
                intent: FfiIntent::SetAccount {
                    id: "giro-3f9a1c2e7b4d".to_owned(),
                    posting: 1,
                    old: "Expenses:Uncategorized".to_owned(),
                    new: "Expenses:Garden".to_owned(),
                },
            },
            FfiPendingIntent {
                key: "k4".to_owned(),
                intent: FfiIntent::Dismiss {
                    id: "card-0123456789ab".to_owned(),
                },
            },
            FfiPendingIntent {
                key: "k5".to_owned(),
                intent: FfiIntent::AddTag {
                    id: "giro-3f9a1c2e7b4d".to_owned(),
                    tag: "friends".to_owned(),
                },
            },
            FfiPendingIntent {
                key: "k6".to_owned(),
                intent: FfiIntent::RemoveTag {
                    id: "giro-3f9a1c2e7b4d".to_owned(),
                    tag: "recurring".to_owned(),
                },
            },
            FfiPendingIntent {
                key: "k7".to_owned(),
                intent: FfiIntent::StartTrip {
                    id: TRIP_START.to_owned(),
                    date: "2026-04-10".to_owned(),
                    name: "lisbon".to_owned(),
                },
            },
            FfiPendingIntent {
                key: "k8".to_owned(),
                intent: FfiIntent::EndTrip {
                    id: TRIP_END.to_owned(),
                    date: "2026-04-21".to_owned(),
                },
            },
        ];

        let converted = pending_intents(pending).unwrap();

        assert_eq!(
            converted,
            vec![
                PendingIntent {
                    key: "k1".to_owned(),
                    intent: Intent::Confirm {
                        id: "giro-3f9a1c2e7b4d".to_owned(),
                    },
                },
                PendingIntent {
                    key: "k2".to_owned(),
                    intent: Intent::OpenAccount {
                        account: "Expenses:Garden".to_owned(),
                        date: Date::constant(2026, 1, 2),
                    },
                },
                PendingIntent {
                    key: "k3".to_owned(),
                    intent: Intent::SetAccount {
                        id: "giro-3f9a1c2e7b4d".to_owned(),
                        posting: 1,
                        old: "Expenses:Uncategorized".to_owned(),
                        new: "Expenses:Garden".to_owned(),
                    },
                },
                PendingIntent {
                    key: "k4".to_owned(),
                    intent: Intent::Dismiss {
                        id: "card-0123456789ab".to_owned(),
                    },
                },
                PendingIntent {
                    key: "k5".to_owned(),
                    intent: Intent::AddTag {
                        id: "giro-3f9a1c2e7b4d".to_owned(),
                        tag: "friends".to_owned(),
                    },
                },
                PendingIntent {
                    key: "k6".to_owned(),
                    intent: Intent::RemoveTag {
                        id: "giro-3f9a1c2e7b4d".to_owned(),
                        tag: "recurring".to_owned(),
                    },
                },
                PendingIntent {
                    key: "k7".to_owned(),
                    intent: Intent::StartTrip {
                        id: TRIP_START.to_owned(),
                        date: Date::constant(2026, 4, 10),
                        name: "lisbon".to_owned(),
                    },
                },
                PendingIntent {
                    key: "k8".to_owned(),
                    intent: Intent::EndTrip {
                        id: TRIP_END.to_owned(),
                        date: Date::constant(2026, 4, 21),
                    },
                },
            ]
        );
    }

    const TRIP_START: &str = "k7p2q9xd-01jc8m2w9z4f7q3r5t6v8x0y1a";
    const TRIP_END: &str = "k7p2q9xd-01jcfh3k5n7p9r1s3t5v7w9y2b";

    #[test]
    fn invalid_trip_date_is_rejected() {
        let result = pending_intents(vec![FfiPendingIntent {
            key: "k1".to_owned(),
            intent: FfiIntent::StartTrip {
                id: TRIP_START.to_owned(),
                date: "2026-02-30".to_owned(),
                name: "lisbon".to_owned(),
            },
        }]);
        assert!(matches!(result, Err(LentilError::Other { .. })));

        let result = pending_intents(vec![FfiPendingIntent {
            key: "k1".to_owned(),
            intent: FfiIntent::EndTrip {
                id: TRIP_END.to_owned(),
                date: "today".to_owned(),
            },
        }]);
        assert!(matches!(result, Err(LentilError::Other { .. })));
    }

    #[test]
    fn core_intents_convert_back_for_the_app() {
        let id = "giro-3f9a1c2e7b4d".to_owned();
        assert_eq!(
            FfiIntent::try_from(Intent::AddTag {
                id: id.clone(),
                tag: "solo".to_owned(),
            })
            .unwrap(),
            FfiIntent::AddTag {
                id: id.clone(),
                tag: "solo".to_owned(),
            }
        );
        assert_eq!(
            FfiIntent::try_from(Intent::RemoveTag {
                id: id.clone(),
                tag: "friends".to_owned(),
            })
            .unwrap(),
            FfiIntent::RemoveTag {
                id: id.clone(),
                tag: "friends".to_owned(),
            }
        );
        assert!(matches!(
            FfiIntent::try_from(Intent::SetPayee {
                id,
                old: None,
                new: "Shop".to_owned(),
            }),
            Err(LentilError::Other { .. })
        ));
    }

    #[test]
    fn tag_choices_convert() {
        let choices = TagChoices {
            tags: vec![TagChoice {
                tag: "irregular".to_owned(),
                replaces: vec!["recurring".to_owned()],
            }],
            families: vec!["trip-".to_owned()],
            source: ChoiceSource::Invalid,
        };
        assert_eq!(
            FfiTagChoices::from(choices),
            FfiTagChoices {
                tags: vec![FfiTagChoice {
                    tag: "irregular".to_owned(),
                    replaces: vec!["recurring".to_owned()],
                }],
                families: vec!["trip-".to_owned()],
                source: FfiChoiceSource::Invalid,
            }
        );
        for (source, ffi) in [
            (ChoiceSource::Taxonomy, FfiChoiceSource::Taxonomy),
            (ChoiceSource::Missing, FfiChoiceSource::Missing),
        ] {
            assert_eq!(FfiChoiceSource::from(source), ffi);
        }
    }

    #[test]
    fn invalid_open_date_is_rejected() {
        let result = pending_intents(vec![FfiPendingIntent {
            key: "k1".to_owned(),
            intent: FfiIntent::OpenAccount {
                account: "Expenses:Garden".to_owned(),
                date: "2026-13-01".to_owned(),
            },
        }]);

        assert!(matches!(result, Err(LentilError::Other { .. })));
    }

    #[test]
    fn credentials_never_show_secrets_in_debug() {
        let credentials = FfiCredentials::UserPassword {
            username: "alex".to_owned(),
            password: "token-not-for-logs".to_owned(),
        };

        assert!(!format!("{credentials:?}").contains("token-not-for-logs"));
        assert!(matches!(
            Credentials::from(credentials),
            Credentials::UserPassword { password, .. } if password == "token-not-for-logs"
        ));
    }

    #[test]
    fn finding_args_survive_as_map() {
        let mut args = BTreeMap::new();
        args.insert(
            "path".to_owned(),
            "manual/k7p2q9xd/2026-10.beancount".to_owned(),
        );
        let finding = Finding {
            code: FindingCode::OutsideWriteArea,
            severity: Severity::Blocking,
            file: None,
            line: None,
            args,
        };

        let ffi = FfiFinding::from(finding);

        assert_eq!(
            ffi.args.get("path").map(String::as_str),
            Some("manual/k7p2q9xd/2026-10.beancount")
        );
    }
}
