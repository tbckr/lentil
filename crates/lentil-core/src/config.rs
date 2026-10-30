//! `lentil.toml`, the repository configuration (spec section 4).

use serde::Deserialize;
use thiserror::Error;

/// The highest `spec_version` this core reads and writes.
pub const SUPPORTED_SPEC_VERSION: u32 = 0;

/// Path of the configuration file, relative to the repository root.
pub const CONFIG_FILE: &str = "lentil.toml";

/// The parsed `lentil.toml`.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// Convention spec version the repository follows.
    pub spec_version: u32,
    /// Entry point of the ledger, normally `main.beancount`.
    pub main_file: String,
    /// Counter account when no suggestion exists.
    pub fallback_account: String,
    /// Receives differences between balance snapshots (v1).
    pub unattributed_account: String,
    /// Currencies for which `lentil prices` fetches ECB rates (v1).
    #[serde(default)]
    pub price_currencies: Vec<String>,
    /// Imported accounts.
    #[serde(default, rename = "source")]
    pub sources: Vec<SourceConfig>,
    /// Balance-tracked accounts (v1).
    #[serde(default, rename = "snapshot")]
    pub snapshots: Vec<SnapshotConfig>,
}

/// One imported bank account or card (`[[source]]`).
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceConfig {
    /// `[a-z0-9-]`, part of every imported ID and of the import paths.
    pub id: String,
    /// The Beancount account the source books to.
    pub account: String,
    /// The account's currency, used when the export has no currency column.
    pub currency: String,
    /// Which adapter reads the source's exports.
    pub importer: ImporterKind,
    /// Mapping file under `sources/` (spec section 15), required for CSV.
    pub mapping: Option<String>,
}

/// The adapter of a source.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ImporterKind {
    /// CSV with a declarative mapping (spec section 15).
    Csv,
}

/// One balance-tracked account (`[[snapshot]]`, v1).
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SnapshotConfig {
    /// The Beancount account.
    pub account: String,
    /// The account's currency.
    pub currency: String,
}

/// Why a `lentil.toml` cannot be used.
#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum ConfigError {
    /// Not valid TOML, an unknown key, a missing required key or a wrong type.
    #[error("invalid lentil.toml: {0}")]
    Syntax(String),
    /// The repository uses a newer spec than this client supports.
    #[error("spec_version {found} is newer than the supported version {supported}")]
    UnsupportedSpecVersion {
        /// The repository's version.
        found: u32,
        /// The client's version.
        supported: u32,
    },
    /// A source ID is not `[a-z0-9-]`, is reserved, or appears twice.
    #[error("invalid source id {0:?}")]
    InvalidSourceId(String),
    /// Two sources share an ID.
    #[error("duplicate source id {0:?}")]
    DuplicateSourceId(String),
    /// A CSV source without a mapping, or a mapping outside `sources/`.
    #[error("invalid mapping path for source {0:?}")]
    InvalidMapping(String),
    /// An account is both a source and a snapshot account.
    #[error("account {0:?} is both a source and a snapshot account")]
    SourceAndSnapshot(String),
}

/// Source IDs that name placeholder directories (see `layout`).
pub const RESERVED_SOURCE_IDS: &[&str] = &["keep"];

impl Config {
    /// Parses and checks a `lentil.toml`.
    ///
    /// The spec version is checked first, so a repository written by a newer
    /// client is reported as such rather than as a syntax error.
    ///
    /// # Errors
    ///
    /// Returns a [`ConfigError`] if the file is invalid.
    pub fn parse(text: &str) -> Result<Self, ConfigError> {
        let value: toml::Table =
            toml::from_str(text).map_err(|e| ConfigError::Syntax(e.message().to_owned()))?;
        if let Some(found) = value
            .get("spec_version")
            .and_then(toml::Value::as_integer)
            .and_then(|v| u32::try_from(v).ok())
            && found > SUPPORTED_SPEC_VERSION
        {
            return Err(ConfigError::UnsupportedSpecVersion {
                found,
                supported: SUPPORTED_SPEC_VERSION,
            });
        }
        let config: Self =
            toml::from_str(text).map_err(|e| ConfigError::Syntax(e.message().to_owned()))?;
        config.check()?;
        Ok(config)
    }

    fn check(&self) -> Result<(), ConfigError> {
        let mut seen = std::collections::BTreeSet::new();
        for source in &self.sources {
            if !is_valid_name(&source.id) || RESERVED_SOURCE_IDS.contains(&source.id.as_str()) {
                return Err(ConfigError::InvalidSourceId(source.id.clone()));
            }
            if !seen.insert(source.id.as_str()) {
                return Err(ConfigError::DuplicateSourceId(source.id.clone()));
            }
            match (&source.importer, &source.mapping) {
                (ImporterKind::Csv, Some(mapping))
                    if mapping.starts_with("sources/")
                        && mapping.ends_with(".toml")
                        && !mapping.contains("..") => {}
                _ => return Err(ConfigError::InvalidMapping(source.id.clone())),
            }
            if self.snapshots.iter().any(|s| s.account == source.account) {
                return Err(ConfigError::SourceAndSnapshot(source.account.clone()));
            }
        }
        Ok(())
    }

    /// The source with the given ID.
    #[must_use]
    pub fn source(&self, id: &str) -> Option<&SourceConfig> {
        self.sources.iter().find(|s| s.id == id)
    }

    /// The source that books to `account`.
    #[must_use]
    pub fn source_for_account(&self, account: &str) -> Option<&SourceConfig> {
        self.sources.iter().find(|s| s.account == account)
    }

    /// Whether `account` is the account of a source.
    #[must_use]
    pub fn is_source_account(&self, account: &str) -> bool {
        self.source_for_account(account).is_some()
    }
}

/// Whether `name` uses only `[a-z0-9-]` and is not empty (spec section 3).
#[must_use]
pub fn is_valid_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXAMPLE: &str = r#"
spec_version = 0
main_file = "main.beancount"
fallback_account = "Expenses:Uncategorized"
unattributed_account = "Expenses:Unattributed"
price_currencies = ["USD"]

[[source]]
id = "giro"
account = "Assets:Bank:Giro"
currency = "EUR"
importer = "csv"
mapping = "sources/giro.toml"

[[snapshot]]
account = "Assets:PaymentService"
currency = "EUR"
"#;

    #[test]
    fn parses_the_spec_example() {
        let config = Config::parse(EXAMPLE).unwrap();
        assert_eq!(config.spec_version, 0);
        assert_eq!(config.sources[0].id, "giro");
        assert_eq!(config.sources[0].importer, ImporterKind::Csv);
        assert_eq!(config.snapshots[0].account, "Assets:PaymentService");
        assert!(config.is_source_account("Assets:Bank:Giro"));
        assert!(!config.is_source_account("Assets:PaymentService"));
    }

    #[test]
    fn rejects_unknown_keys() {
        let text = format!("{EXAMPLE}\nextra = 1\n");
        assert!(matches!(Config::parse(&text), Err(ConfigError::Syntax(_))));
    }

    #[test]
    fn rejects_missing_required_keys() {
        assert!(matches!(
            Config::parse("spec_version = 0\n"),
            Err(ConfigError::Syntax(_))
        ));
    }

    #[test]
    fn reports_newer_spec_version_before_syntax() {
        assert_eq!(
            Config::parse("spec_version = 1\nfuture_key = true\n"),
            Err(ConfigError::UnsupportedSpecVersion {
                found: 1,
                supported: 0
            })
        );
    }

    #[test]
    fn rejects_bad_source_ids() {
        for id in ["Giro", "gi ro", "", "keep"] {
            let text = EXAMPLE.replace("id = \"giro\"", &format!("id = {id:?}"));
            assert_eq!(
                Config::parse(&text),
                Err(ConfigError::InvalidSourceId(id.to_owned())),
                "{id}"
            );
        }
    }

    #[test]
    fn rejects_account_that_is_source_and_snapshot() {
        let text = EXAMPLE.replace("Assets:PaymentService", "Assets:Bank:Giro");
        assert_eq!(
            Config::parse(&text),
            Err(ConfigError::SourceAndSnapshot("Assets:Bank:Giro".into()))
        );
    }

    #[test]
    fn rejects_mapping_outside_sources() {
        let text = EXAMPLE.replace("sources/giro.toml", "../giro.toml");
        assert_eq!(
            Config::parse(&text),
            Err(ConfigError::InvalidMapping("giro".into()))
        );
    }
}
