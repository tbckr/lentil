//! Validation before every commit (spec section 13) and the supported
//! profile (spec section 14).

use crate::config::Config;
use crate::finding::Finding;
use crate::ledger::Ledger;
use crate::snapshot::Snapshot;

/// The role a client acts in for one commit (spec section 7).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Role {
    /// Creates new run files only.
    Importer,
    /// Edits existing transactions; writes `manual/<device-id>/` and appends `open` directives.
    Editor,
    /// May change anything, as long as the result follows the spec.
    Admin,
}

impl Role {
    /// The lowercase name used in commit subjects and trailers.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Importer => "importer",
            Self::Editor => "editor",
            Self::Admin => "admin",
        }
    }
}

/// What a commit is checked against.
#[derive(Clone, Copy, Debug)]
pub struct Context<'a> {
    /// The role of the commit.
    pub role: Role,
    /// The writing device, for the editor's `manual/<device-id>/` area.
    pub device: Option<&'a str>,
    /// The state the change starts from; `None` for a repository's first commit.
    pub before: Option<&'a Snapshot>,
}

/// The validated state.
#[derive(Clone, Debug)]
pub struct Validation {
    /// Parsed configuration, if valid.
    pub config: Option<Config>,
    /// The loaded ledger, if the configuration was valid.
    pub ledger: Option<Ledger>,
    /// All findings, blocking first.
    pub findings: Vec<Finding>,
}

impl Validation {
    /// Whether any finding blocks the commit.
    #[must_use]
    pub fn is_blocking(&self) -> bool {
        crate::finding::any_blocking(&self.findings)
    }
}

/// Validates the complete state `after` for a commit in `context`.
///
/// Runs every check of spec section 13 and the profile of section 14; with
/// `context.before`, also checks that each change lies in the role's write
/// areas and that only new elements were checked where the spec says "new".
#[must_use]
pub fn validate(after: &Snapshot, context: &Context<'_>) -> Validation {
    let _ = (after, context);
    todo!("task C")
}

/// Checks a fetched state before an editor or importer writes to it (spec
/// section 13): findings that make the repository read-only, empty if writable.
#[must_use]
pub fn read_only_reasons(snapshot: &Snapshot) -> Vec<Finding> {
    let _ = snapshot;
    todo!("task C")
}
