//! Repository layout (spec section 3) and the files `init` creates (PRD FR-A1).

use crate::snapshot::Snapshot;

/// The include patterns `main.beancount` holds, in order (spec section 3).
pub const MAIN_INCLUDES: [&str; 5] = [
    "fava.beancount",
    "accounts.beancount",
    "imports/*/*/*.beancount",
    "manual/*/*.beancount",
    "prices/*/*.beancount",
];

/// Placeholder files that keep each glob include matching in a repository
/// without imports, manual entries or prices. Python Beancount and rustledger
/// both report a glob without matches as an error.
pub const PLACEHOLDERS: [&str; 3] = [
    "imports/keep/keep/keep.beancount",
    "manual/keep/keep.beancount",
    "prices/keep/keep.beancount",
];

/// Options for a new repository.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InitOptions {
    /// `option "title"`.
    pub title: String,
    /// `option "operating_currency"`.
    pub currency: String,
}

/// The files of a new repository: `lentil.toml`, `main.beancount`,
/// `fava.beancount`, `accounts.beancount`, `rules.toml`, `.gitignore`,
/// `README.md` and the placeholders.
#[must_use]
pub fn init_files(options: &InitOptions) -> Snapshot {
    let _ = options;
    todo!("task C")
}

/// The exact `main.beancount` of spec section 3 for a title and currency.
#[must_use]
pub fn main_file(title: &str, currency: &str) -> String {
    let _ = (title, currency);
    todo!("task C")
}
