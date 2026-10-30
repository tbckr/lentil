//! Byte-precise text edits (PRD principle 7, spec section 10).
//!
//! Each function changes only the bytes of one element and leaves the rest of
//! the text byte-identical. Positions come from [`crate::ledger::Txn`].

use std::ops::Range;

use thiserror::Error;

/// Why an edit cannot be made at the given position.
#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum EditError {
    /// The bytes at the position are not what the caller expected.
    #[error("unexpected text at {0:?}")]
    Mismatch(Range<usize>),
}

/// Replaces the flag character at `offset` (expected to be `old`) with `new`.
///
/// # Errors
///
/// [`EditError::Mismatch`] if the character at `offset` is not `old`.
pub fn set_flag(text: &str, offset: usize, old: char, new: char) -> Result<String, EditError> {
    let _ = (text, offset, old, new);
    todo!("task A")
}

/// Replaces the account name at `span` (expected to be `old`) with `new`.
///
/// # Errors
///
/// [`EditError::Mismatch`] if the text at `span` is not `old`.
pub fn set_account(
    text: &str,
    span: Range<usize>,
    old: &str,
    new: &str,
) -> Result<String, EditError> {
    let _ = (text, span, old, new);
    todo!("task A")
}

/// Comments out the lines of `span` by prepending `; ` to each (spec section 10, `dismiss`).
///
/// `span` must start at the beginning of a line; every line it touches is
/// commented out, including blank-free indented lines inside it.
///
/// # Errors
///
/// [`EditError::Mismatch`] if `span` does not start at a line start.
pub fn comment_out(text: &str, span: Range<usize>) -> Result<String, EditError> {
    let _ = (text, span);
    todo!("task A")
}

/// Appends `line` as a new last line, adding a line break before it if the
/// text does not end with one.
#[must_use]
pub fn append_line(text: &str, line: &str) -> String {
    let _ = (text, line);
    todo!("task A")
}
