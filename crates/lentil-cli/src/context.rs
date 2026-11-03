//! What every command needs: the repository root, its configuration and
//! rules, the device ID, the author, and printing findings and sync errors.

use std::path::{Path, PathBuf};

use lentil_core::config::{CONFIG_FILE, Config, ConfigError};
use lentil_core::env::OsRandom;
use lentil_core::finding::Finding;
use lentil_core::id;
use lentil_core::import::{Rules, RulesError};
use lentil_core::snapshot::Snapshot;
use lentil_core::sync::git::Git2Backend;
use lentil_core::sync::{Author, Identity, SyncError};
use lentil_core::tags::{Taxonomy, TaxonomyError};
use lentil_core::validate::Role;

use crate::i18n::{self, Lang, Msg};
use crate::{CLIENT, CliError};

/// The nearest directory at or above `start` that holds a `lentil.toml`.
pub fn find_root(lang: Lang, start: Option<&Path>) -> Result<PathBuf, CliError> {
    let start = match start {
        Some(dir) => dir.to_path_buf(),
        None => std::env::current_dir().map_err(|e| CliError(e.to_string()))?,
    };
    let start = start.canonicalize().unwrap_or(start);
    let mut dir = Some(start.as_path());
    while let Some(candidate) = dir {
        if candidate.join(CONFIG_FILE).is_file() {
            return Ok(candidate.to_path_buf());
        }
        dir = candidate.parent();
    }
    Err(CliError(i18n::msg(
        lang,
        Msg::NoRepository,
        &[("dir", &start.display().to_string())],
    )))
}

/// The device ID of this machine, created once (spec section 9) and kept in
/// the user's configuration directory, outside every repository.
pub fn device_id() -> Result<String, CliError> {
    let dir = config_dir().ok_or_else(|| CliError("no HOME directory".to_owned()))?;
    let path = dir.join("device-id");
    if let Ok(existing) = std::fs::read_to_string(&path) {
        let existing = existing.trim();
        if existing.len() == 8
            && existing
                .bytes()
                .all(|b| b.is_ascii_lowercase() || (b'2'..=b'7').contains(&b))
        {
            return Ok(existing.to_owned());
        }
    }
    let new = id::new_device_id(&mut OsRandom);
    std::fs::create_dir_all(&dir).map_err(|e| CliError(e.to_string()))?;
    std::fs::write(&path, format!("{new}\n")).map_err(|e| CliError(e.to_string()))?;
    Ok(new)
}

fn config_dir() -> Option<PathBuf> {
    std::env::var_os("XDG_CONFIG_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .map(|d| d.join("lentil"))
}

/// The `lentil.toml` of a repository state.
pub fn config(lang: Lang, snapshot: &Snapshot) -> Result<Config, CliError> {
    Config::parse(snapshot.text(CONFIG_FILE).unwrap_or_default())
        .map_err(|e| config_error(lang, &e))
}

/// The `rules.toml` of a repository state, checked against its `config`.
pub fn rules(lang: Lang, snapshot: &Snapshot, config: &Config) -> Result<Rules, CliError> {
    Rules::load(snapshot, config).map_err(|e| rules_error(lang, &e))
}

/// The parsed `tags.toml` of `snapshot`, `None` without the file (spec
/// section 16).
pub fn taxonomy(lang: Lang, snapshot: &Snapshot) -> Result<Option<Taxonomy>, CliError> {
    Taxonomy::load(snapshot)
        .transpose()
        .map_err(|e| taxonomy_error(lang, &e))
}

/// A rejected `tags.toml` as an error.
pub fn taxonomy_error(lang: Lang, error: &TaxonomyError) -> CliError {
    CliError(i18n::msg(
        lang,
        Msg::TagsInvalid,
        &[("detail", &i18n::taxonomy_error(lang, error))],
    ))
}

/// An unusable `lentil.toml` as an error.
pub fn config_error(lang: Lang, error: &ConfigError) -> CliError {
    CliError(i18n::msg(
        lang,
        Msg::ConfigMissing,
        &[("detail", &error.to_string())],
    ))
}

/// An unusable `rules.toml` as an error.
pub fn rules_error(lang: Lang, error: &RulesError) -> CliError {
    CliError(i18n::msg(
        lang,
        Msg::RulesInvalid,
        &[("detail", &i18n::rules_error(lang, error))],
    ))
}

/// The `Lentil-*` trailer values for a role.
pub fn identity(role: Role) -> Result<Identity, CliError> {
    Ok(Identity {
        role,
        client: CLIENT.to_owned(),
        device: device_id()?,
    })
}

/// The commit author from the repository's Git configuration.
pub fn author(lang: Lang, git: &Git2Backend) -> Result<Author, CliError> {
    git.configured_author()
        .ok_or_else(|| CliError(i18n::text(lang, Msg::NoAuthor).to_owned()))
}

/// Prints findings to stderr, one per line.
pub fn print_findings(lang: Lang, findings: &[Finding]) {
    for f in findings {
        eprintln!("{}", i18n::finding(lang, f));
    }
}

/// Turns a sync error into a printed explanation and an error.
pub fn sync_error(lang: Lang, error: SyncError) -> CliError {
    match error {
        SyncError::Git(e) => CliError(i18n::git_error(lang, &e)),
        SyncError::ReadOnly(findings) => {
            eprintln!("{}", i18n::text(lang, Msg::ReadOnly));
            print_findings(lang, &findings);
            CliError(
                i18n::text(lang, Msg::ReadOnly)
                    .trim_end_matches(':')
                    .to_owned(),
            )
        }
        SyncError::Blocked(findings) => {
            eprintln!("{}", i18n::text(lang, Msg::Blocked));
            print_findings(lang, &findings);
            CliError(
                i18n::text(lang, Msg::Blocked)
                    .trim_end_matches(':')
                    .to_owned(),
            )
        }
        SyncError::TooManyAttempts => CliError(i18n::text(lang, Msg::TooManyAttempts).to_owned()),
    }
}
