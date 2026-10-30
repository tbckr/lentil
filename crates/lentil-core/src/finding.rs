//! Typed findings that every client translates itself.
//!
//! The core never produces user-facing English text. A finding carries a
//! [`FindingCode`], a severity, an optional location and named arguments
//! (account names, IDs, amounts) that a client inserts into its own message.

use std::collections::BTreeMap;

/// How a finding affects writing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Severity {
    /// Stops the commit (spec section 13).
    Blocking,
    /// Reported, never blocks.
    Warning,
}

/// What a finding is about. Each client maps every code to a message.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum FindingCode {
    /// `lentil.toml` is missing or invalid. Args: `detail`.
    ConfigInvalid,
    /// The repository's `spec_version` is newer than the client's. Args: `found`, `supported`.
    SpecVersionUnsupported,
    /// A file reachable from the main file does not parse. Args: `detail`.
    ParseError,
    /// A transaction does not balance. Args: `detail`.
    TransactionUnbalanced,
    /// A posting uses an account that is not open, or already closed. Args: `account`.
    AccountNotOpen,
    /// Two transactions share a `lentil_id`. Args: `id`.
    DuplicateLentilId,
    /// A change lies outside the write areas of the commit's role. Args: `path`, `role`.
    OutsideWriteArea,
    /// A construct outside the supported profile (spec section 14). Args: `construct`.
    OutsideProfile,
    /// `main.beancount` differs from spec section 3. Args: `detail`.
    MainFileMismatch,
    /// An include points outside the repository. Args: `include`.
    IncludeOutsideRepository,
    /// A new balance assertion by an importer lacks `~ 0.00`. Args: `account`.
    MissingZeroTolerance,
    /// A manual transaction posts to a source account. Args: `account`, `id`.
    ManualPostingOnSourceAccount,
    /// A pad posts to a source account. Args: `account`.
    PadOnSourceAccount,
    /// A `lentil_id`, `lentil_ref` or `lentil_desc` was changed or removed. Args: `id`, `key`.
    ProtectedMetadataChanged,
    /// A transaction was removed or its date or source posting changed by an editor. Args: `id`.
    ProtectedTransactionChanged,
    /// A `.beancount` file outside the include patterns of spec section 3. Args: `path`.
    StrayLedgerFile,
    /// A path that breaks the naming rules of spec section 3. Args: `path`.
    InvalidPath,
    /// A load cache was committed. Args: `path`.
    LoadCacheCommitted,
    /// Any other error the core reports. Args: `code` (rustledger's), `detail`.
    CoreError,
    /// A balance assertion fails. Args: `account`, `date`, `asserted`, `difference`, `currency`.
    BalanceAssertionFailed,
}

/// One validation or status finding.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Finding {
    /// What the finding is about.
    pub code: FindingCode,
    /// Whether it blocks a commit.
    pub severity: Severity,
    /// Repository-relative file, if the finding has a location.
    pub file: Option<String>,
    /// 1-based line in `file`.
    pub line: Option<u32>,
    /// Named arguments for the client's message.
    pub args: BTreeMap<String, String>,
}

impl Finding {
    /// A finding without location or arguments.
    #[must_use]
    pub fn new(code: FindingCode, severity: Severity) -> Self {
        Self {
            code,
            severity,
            file: None,
            line: None,
            args: BTreeMap::new(),
        }
    }

    /// A blocking finding.
    #[must_use]
    pub fn blocking(code: FindingCode) -> Self {
        Self::new(code, Severity::Blocking)
    }

    /// A warning.
    #[must_use]
    pub fn warning(code: FindingCode) -> Self {
        Self::new(code, Severity::Warning)
    }

    /// Sets the location.
    #[must_use]
    pub fn at(mut self, file: impl Into<String>, line: Option<u32>) -> Self {
        self.file = Some(file.into());
        self.line = line;
        self
    }

    /// Adds a named argument.
    #[must_use]
    pub fn arg(mut self, key: &str, value: impl Into<String>) -> Self {
        self.args.insert(key.to_owned(), value.into());
        self
    }

    /// Whether the finding blocks a commit.
    #[must_use]
    pub fn is_blocking(&self) -> bool {
        self.severity == Severity::Blocking
    }
}

/// Whether any finding blocks a commit.
#[must_use]
pub fn any_blocking(findings: &[Finding]) -> bool {
    findings.iter().any(Finding::is_blocking)
}
