//! The one error type the app sees.
//!
//! Variants are codes the app translates. Details never carry credentials:
//! network and lookup errors drop libgit2's message, which may contain the
//! remote URL, and the remaining details have URL user info masked.

use lentil_core::correction::CorrectionError;
use lentil_core::sync::{GitError, SyncError};

use crate::types::{FfiFinding, findings};

/// Why a call failed.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error, uniffi::Error)]
pub enum LentilError {
    /// The remote is unreachable (offline, DNS, TLS). Pending intents stay.
    #[error("the remote is unreachable")]
    Network,
    /// The remote rejected the credentials.
    #[error("authentication failed")]
    Auth,
    /// The repository or its `main` branch does not exist.
    #[error("repository or branch not found")]
    NotFound,
    /// The local clone has changes a sync would discard.
    #[error("the local clone has changes")]
    LocalChanges,
    /// The repository needs repair by the admin role before the app may write.
    #[error("the repository is read-only")]
    ReadOnly {
        /// Why, blocking findings first.
        findings: Vec<FfiFinding>,
    },
    /// The new state has blocking findings; nothing was committed.
    #[error("validation failed")]
    Blocked {
        /// The findings.
        findings: Vec<FfiFinding>,
    },
    /// Others kept pushing first; try again later.
    #[error("gave up after too many attempts")]
    TooManyAttempts,
    /// The transaction to correct has changed: no active transaction has its
    /// `lentil_id`, or it no longer has exactly one counter posting.
    #[error("the transaction cannot be corrected")]
    NotCorrectable,
    /// Reading or writing the clone failed.
    #[error("file system error: {detail}")]
    Io {
        /// The operating system's message.
        detail: String,
    },
    /// Anything else.
    #[error("{detail}")]
    Other {
        /// A message for bug reports, not for translation.
        detail: String,
    },
}

impl From<GitError> for LentilError {
    fn from(error: GitError) -> Self {
        match error {
            GitError::Network(_) => Self::Network,
            GitError::Auth => Self::Auth,
            GitError::NotFound(_) => Self::NotFound,
            GitError::LocalChanges(_) => Self::LocalChanges,
            GitError::Other(detail) => Self::Other {
                detail: redact(&detail),
            },
        }
    }
}

impl From<SyncError> for LentilError {
    fn from(error: SyncError) -> Self {
        match error {
            SyncError::Git(error) => error.into(),
            SyncError::ReadOnly(found) => Self::ReadOnly {
                findings: findings(found),
            },
            SyncError::Blocked(found) => Self::Blocked {
                findings: findings(found),
            },
            SyncError::TooManyAttempts => Self::TooManyAttempts,
        }
    }
}

impl From<CorrectionError> for LentilError {
    fn from(error: CorrectionError) -> Self {
        match error {
            CorrectionError::NotFound | CorrectionError::NotEditable => Self::NotCorrectable,
        }
    }
}

impl From<std::io::Error> for LentilError {
    fn from(error: std::io::Error) -> Self {
        Self::Io {
            detail: error.to_string(),
        }
    }
}

impl From<git2::Error> for LentilError {
    fn from(error: git2::Error) -> Self {
        Self::Other {
            detail: redact(error.message()),
        }
    }
}

/// Masks the user info of URLs in `text`, such as a token in
/// `https://user:token@host/repo.git`.
pub(crate) fn redact(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(scheme) = rest.find("://") {
        let authority_start = scheme + 3;
        out.push_str(&rest[..authority_start]);
        rest = &rest[authority_start..];
        let authority_end = rest
            .find(|c: char| c == '/' || c.is_whitespace())
            .unwrap_or(rest.len());
        match rest[..authority_end].rfind('@') {
            Some(at) => {
                out.push_str("***");
                rest = &rest[at..];
            }
            None => {
                out.push_str(&rest[..authority_end]);
                rest = &rest[authority_end..];
            }
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use lentil_core::finding::{Finding, FindingCode};

    use super::*;

    #[test]
    fn git_errors_become_codes_without_details() {
        assert_eq!(
            LentilError::from(GitError::Network(
                "failed to connect to https://alex:secret@git.example.org".to_owned()
            )),
            LentilError::Network
        );
        assert_eq!(LentilError::from(GitError::Auth), LentilError::Auth);
        assert_eq!(
            LentilError::from(GitError::NotFound(
                "https://alex:secret@git.example.org/x".to_owned()
            )),
            LentilError::NotFound
        );
        assert_eq!(
            LentilError::from(GitError::LocalChanges("1 commit".to_owned())),
            LentilError::LocalChanges
        );
    }

    #[test]
    fn other_git_errors_keep_a_redacted_detail() {
        let error = LentilError::from(GitError::Other(
            "unexpected http status for https://alex:secret@git.example.org/ledger.git".to_owned(),
        ));

        assert_eq!(
            error,
            LentilError::Other {
                detail: "unexpected http status for https://***@git.example.org/ledger.git"
                    .to_owned()
            }
        );
    }

    #[test]
    fn sync_errors_map_to_flat_variants() {
        assert_eq!(
            LentilError::from(SyncError::Git(GitError::Auth)),
            LentilError::Auth
        );
        assert_eq!(
            LentilError::from(SyncError::TooManyAttempts),
            LentilError::TooManyAttempts
        );
        let LentilError::ReadOnly { findings } = LentilError::from(SyncError::ReadOnly(vec![
            Finding::blocking(FindingCode::OutsideProfile).arg("construct", "plugin"),
        ])) else {
            panic!("expected ReadOnly");
        };
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].code, "OutsideProfile");
        let LentilError::Blocked { findings } = LentilError::from(SyncError::Blocked(vec![
            Finding::blocking(FindingCode::AccountNotOpen).arg("account", "Expenses:Garden"),
        ])) else {
            panic!("expected Blocked");
        };
        assert_eq!(findings[0].code, "AccountNotOpen");
    }

    #[test]
    fn correction_errors_become_one_code() {
        for error in [CorrectionError::NotFound, CorrectionError::NotEditable] {
            assert_eq!(LentilError::from(error), LentilError::NotCorrectable);
        }
    }

    #[test]
    fn redact_masks_user_info_only() {
        assert_eq!(
            redact("https://alex:secret@git.example.org/ledger.git and ssh://git@host/x"),
            "https://***@git.example.org/ledger.git and ssh://***@host/x"
        );
        assert_eq!(
            redact("see https://git.example.org/a@b"),
            "see https://git.example.org/a@b"
        );
        assert_eq!(redact("no url here"), "no url here");
        assert_eq!(redact("https://"), "https://");
    }
}
