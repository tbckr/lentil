//! Suggestion rules from `rules.toml` and learned suggestions (spec section 16).

use thiserror::Error;

use crate::ledger::Ledger;

/// Path of the rules file, relative to the repository root.
pub const RULES_FILE: &str = "rules.toml";

/// The parsed `rules.toml`; rules are evaluated in file order.
#[derive(Clone, Debug, Default)]
pub struct Rules {
    // Fields defined in task B.
}

/// Why `rules.toml` is rejected.
#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum RulesError {
    /// Not TOML, an unknown key or a missing `account`.
    #[error("invalid rules.toml: {0}")]
    Syntax(String),
    /// A regular expression does not compile.
    #[error("invalid regular expression {0:?}")]
    Regex(String),
    /// A rule matches on nothing (no `desc`, `payee` or `narration`).
    #[error("rule {0} has no condition")]
    NoCondition(usize),
}

impl Rules {
    /// Parses `rules.toml`. An empty file has no rules.
    ///
    /// # Errors
    ///
    /// Returns a [`RulesError`] for invalid content.
    pub fn parse(text: &str) -> Result<Self, RulesError> {
        let _ = text;
        todo!("task B")
    }

    /// The account of the first matching rule.
    #[must_use]
    pub fn suggest(
        &self,
        source: &str,
        desc: &str,
        payee: Option<&str>,
        narration: &str,
    ) -> Option<&str> {
        let _ = (source, desc, payee, narration);
        todo!("task B")
    }
}

/// Suggestions learned from reviewed transactions of one source account.
#[derive(Clone, Debug, Default)]
pub struct Learned {
    // Fields defined in task B.
}

impl Learned {
    /// Learns from reviewed (`*`) transactions posting to `source_account`,
    /// using only `lentil_desc` and the counter account (PRD FR-C8).
    #[must_use]
    pub fn from_ledger(ledger: &Ledger, source_account: &str) -> Self {
        let _ = (ledger, source_account);
        todo!("task B")
    }

    /// The learned counter account for a description, if any.
    #[must_use]
    pub fn suggest(&self, desc: &str) -> Option<&str> {
        let _ = desc;
        todo!("task B")
    }
}
