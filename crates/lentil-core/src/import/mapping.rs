//! CSV mapping files under `sources/` (spec section 15).

use thiserror::Error;

/// A parsed mapping file. Unknown keys are rejected.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mapping {
    // Fields defined in task B.
}

/// Why a mapping file is rejected.
#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum MappingError {
    /// Not TOML, an unknown or missing key, or a wrong type. Detail from the TOML parser.
    #[error("invalid mapping: {0}")]
    Syntax(String),
    /// A regular expression does not compile.
    #[error("invalid regular expression {0:?}")]
    Regex(String),
    /// An encoding other than the four of spec section 15.
    #[error("unsupported encoding {0:?}")]
    Encoding(String),
    /// A date format with codes other than `%d`, `%m`, `%Y`, `%y`.
    #[error("unsupported date format {0:?}")]
    DateFormat(String),
    /// Keys that do not fit together, such as `from = "column"` without `column`.
    #[error("inconsistent mapping: {0}")]
    Inconsistent(String),
}

impl Mapping {
    /// Parses a mapping file.
    ///
    /// # Errors
    ///
    /// Returns a [`MappingError`] for any invalid content.
    pub fn parse(text: &str) -> Result<Self, MappingError> {
        let _ = text;
        todo!("task B")
    }
}
