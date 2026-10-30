//! FFI records and enums, and their conversions from and to the core's types.

use std::collections::HashMap;

use jiff::civil::Date;
use lentil_core::finding::{Finding, FindingCode, Severity};
use lentil_core::intent::{ConflictCode, Intent, Outcome, PendingIntent};
use lentil_core::sync::{Author, Credentials};

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
                date: date.parse::<Date>().map_err(|_| LentilError::Other {
                    detail: format!("invalid date {date:?} in intent {}", pending.key),
                })?,
            },
        };
        Ok(Self {
            key: pending.key,
            intent,
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
        ConflictCode::PostingNotFound => "PostingNotFound",
        ConflictCode::SourcePosting => "SourcePosting",
        ConflictCode::AccountNotOpen => "AccountNotOpen",
        ConflictCode::NotEditable => "NotEditable",
        ConflictCode::FileExists => "FileExists",
        ConflictCode::InvalidAccount => "InvalidAccount",
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
    /// `lentil_desc`, the bank's description.
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
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use lentil_core::intent::Conflict;

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
            ConflictCode::PostingNotFound,
            ConflictCode::SourcePosting,
            ConflictCode::AccountNotOpen,
            ConflictCode::NotEditable,
            ConflictCode::FileExists,
            ConflictCode::InvalidAccount,
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
            ]
        );
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
