//! The CSV adapter (spec section 15).

use super::{ImportError, Statement};
use crate::import::mapping::Mapping;

/// Reads a CSV export into a statement.
///
/// `default_currency` is the source's currency, used when the mapping has no
/// currency column.
///
/// # Errors
///
/// Returns an [`ImportError`] if the export does not match the mapping. Rows
/// are never dropped silently.
pub fn read_statement(
    bytes: &[u8],
    mapping: &Mapping,
    default_currency: &str,
) -> Result<Statement, ImportError> {
    let _ = (bytes, mapping, default_currency);
    todo!("task B")
}
