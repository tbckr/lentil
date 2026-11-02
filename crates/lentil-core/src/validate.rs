//! Validation before every commit (spec section 13) and the supported
//! profile (spec section 14).
//!
//! [`validate`] checks a complete repository state. State checks (layout,
//! configuration, the core's errors, unique IDs, the profile, the main file,
//! postings on source accounts) always run; with [`Context::before`], the
//! changes from that state are also checked against the write areas of the
//! commit's role (spec section 7). Every finding is a typed code; nothing here
//! produces user-facing text.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::LazyLock;

use regex::Regex;
use rust_decimal::Decimal;
use rustledger_core::{Directive, MetaValue, Metadata, Transaction};
use rustledger_parser::ParseResult;

use crate::config::{CONFIG_FILE, Config, ConfigError, SourceConfig, is_valid_name};
use crate::finding::{Finding, FindingCode, Severity};
use crate::layout::{ACCOUNTS_FILE, FAVA_FILE, MAIN_INCLUDES, PLACEHOLDERS};
use crate::ledger::{self, ConstructKind, Ledger, META_DESC, META_ID, PROTECTED_META};
use crate::snapshot::{Change, Snapshot, glob_match, is_load_cache};

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

/// The options the profile supports (spec section 14).
const SUPPORTED_OPTIONS: [&str; 2] = ["title", "operating_currency"];

/// Top-level directories whose names follow spec section 3.
const NAMED_DIRS: [&str; 5] = ["imports", "manual", "prices", "sources", "status"];

/// Values of the `construct` argument of [`FindingCode::OutsideProfile`].
pub mod construct {
    /// An option other than `title` and `operating_currency`, or any option
    /// outside the main file. Extra arg: `name`.
    pub const OPTION: &str = "option";
    /// An `include` outside the main file. Extra arg: `include`.
    pub const INCLUDE: &str = "include";
    /// A `plugin` directive. Extra arg: `name`.
    pub const PLUGIN: &str = "plugin";
    /// A `pushtag` or `poptag` line.
    pub const TAG_STACK: &str = "tag-stack";
    /// A cost annotation `{...}`. Extra arg: `id` if the transaction has one.
    pub const COST: &str = "cost";
    /// More than one posting without a complete amount. Extra arg: `id`.
    pub const ELIDED_POSTINGS: &str = "elided-postings";
    /// A `pad` on an account that is not a snapshot account. Extra arg: `account`.
    pub const PAD: &str = "pad";
}

/// Validates the complete state `after` for a commit in `context`.
///
/// Runs every check of spec section 13 and the profile of section 14; with
/// `context.before`, also checks that each change lies in the role's write
/// areas and that only new elements were checked where the spec says "new".
#[must_use]
pub fn validate(after: &Snapshot, context: &Context<'_>) -> Validation {
    let mut findings = Vec::new();

    let config = match read_config(after) {
        Ok(config) => Some(config),
        Err(finding) => {
            findings.push(finding);
            None
        }
    };
    let main_file = config
        .as_ref()
        .map_or(crate::layout::MAIN_FILE, |c| c.main_file.as_str());
    check_layout(after, main_file, &mut findings);

    let ledger = config.as_ref().map(|config| {
        let ledger = ledger::load(after, &config.main_file);
        check_core_errors(after, &ledger, &mut findings);
        check_balance_failures(&ledger, &mut findings);
        check_required_accounts(config, &ledger, &mut findings);
        check_unique_ids(&ledger, &mut findings);
        check_profile(config, &ledger, &mut findings);
        check_main_file(after, &config.main_file, &mut findings);
        check_fava_file(after, &mut findings);
        check_manual_postings(config, &ledger, &mut findings);
        ledger
    });

    if let Some(before) = context.before {
        let before_config;
        let role_config = match &config {
            Some(config) => Some(config),
            None => {
                before_config = read_config(before).ok();
                before_config.as_ref()
            }
        };
        check_write_areas(before, after, role_config, context, &mut findings);
    }

    findings.sort_by_key(|f| f.severity);
    Validation {
        config,
        ledger,
        findings,
    }
}

/// Checks a fetched state before an editor or importer writes to it (spec
/// section 13): findings that make the repository read-only, empty if writable.
#[must_use]
pub fn read_only_reasons(snapshot: &Snapshot) -> Vec<Finding> {
    let context = Context {
        role: Role::Admin,
        device: None,
        before: None,
    };
    validate(snapshot, &context)
        .findings
        .into_iter()
        .filter(Finding::is_blocking)
        .collect()
}

// ---------------------------------------------------------------------------
// Configuration

fn read_config(snapshot: &Snapshot) -> Result<Config, Finding> {
    let invalid = |detail: &str| {
        Finding::blocking(FindingCode::ConfigInvalid)
            .at(CONFIG_FILE, None)
            .arg("detail", detail)
    };
    let Some(bytes) = snapshot.get(CONFIG_FILE) else {
        return Err(invalid("missing"));
    };
    let Ok(text) = std::str::from_utf8(bytes) else {
        return Err(invalid("not UTF-8"));
    };
    let config = match Config::parse(text) {
        Ok(config) => config,
        Err(ConfigError::UnsupportedSpecVersion { found, supported }) => {
            return Err(Finding::blocking(FindingCode::SpecVersionUnsupported)
                .at(CONFIG_FILE, None)
                .arg("found", found.to_string())
                .arg("supported", supported.to_string()));
        }
        Err(err) => return Err(invalid(&err.to_string())),
    };
    // The main file sits at the root: the include patterns of spec section 3
    // are relative to it and the layout is defined from the root.
    let valid_main = config
        .main_file
        .strip_suffix(".beancount")
        .is_some_and(is_valid_name);
    if !valid_main {
        return Err(invalid(&format!(
            "invalid main_file {:?}",
            config.main_file
        )));
    }
    Ok(config)
}

/// `fallback_account` and `unattributed_account` must be opened (spec section 4).
fn check_required_accounts(config: &Config, ledger: &Ledger, findings: &mut Vec<Finding>) {
    for account in [&config.fallback_account, &config.unattributed_account] {
        let opened = ledger
            .accounts
            .get(account.as_str())
            .is_some_and(|s| s.opened.is_some());
        if !opened {
            findings.push(
                Finding::blocking(FindingCode::AccountNotOpen)
                    .at(CONFIG_FILE, None)
                    .arg("account", account.as_str()),
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Layout (spec section 3)

fn check_layout(snapshot: &Snapshot, main_file: &str, findings: &mut Vec<Finding>) {
    for path in snapshot.paths() {
        let name = path.rsplit('/').next().unwrap_or(path);
        if is_load_cache(name) {
            findings.push(path_finding(FindingCode::LoadCacheCommitted, path));
        } else if path.ends_with(".beancount") && !is_ledger_path(path, main_file) {
            findings.push(path_finding(FindingCode::StrayLedgerFile, path));
        } else if !has_valid_names(path) {
            findings.push(path_finding(FindingCode::InvalidPath, path));
        }
    }
}

fn path_finding(code: FindingCode, path: &str) -> Finding {
    Finding::blocking(code).at(path, None).arg("path", path)
}

/// Whether a `.beancount` file lies where spec section 3 allows one.
fn is_ledger_path(path: &str, main_file: &str) -> bool {
    path == main_file
        || path == FAVA_FILE
        || path == ACCOUNTS_FILE
        || MAIN_INCLUDES[2..].iter().any(|p| glob_match(p, path))
}

/// Whether every name below `imports/`, `manual/`, `prices/`, `sources/` and
/// `status/` uses `[a-z0-9-]`, apart from run timestamps (spec section 3).
fn has_valid_names(path: &str) -> bool {
    let segments: Vec<&str> = path.split('/').collect();
    let Some((file, dirs)) = segments.split_last() else {
        return true;
    };
    if dirs.is_empty() || !NAMED_DIRS.contains(&dirs[0]) {
        return true;
    }
    if !dirs[1..].iter().all(|d| is_valid_name(d)) {
        return false;
    }
    if PLACEHOLDERS.contains(&path) {
        return true;
    }
    if dirs[0] == "imports" && path.ends_with(".beancount") {
        return run_file_parts(path).is_some();
    }
    match file.rsplit_once('.') {
        Some((stem, ext)) => {
            is_valid_name(stem)
                && !ext.is_empty()
                && ext
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        }
        None => is_valid_name(file),
    }
}

/// `(source-id, year)` of a run file
/// `imports/<source-id>/<YYYY>/<YYYYMMDDTHHMMSSZ>.beancount` whose year
/// directory matches its timestamp (spec section 8).
fn run_file_parts(path: &str) -> Option<(&str, &str)> {
    let segments: Vec<&str> = path.split('/').collect();
    let [top, source, year, file] = segments.as_slice() else {
        return None;
    };
    let stem = file.strip_suffix(".beancount")?;
    (*top == "imports"
        && is_valid_name(source)
        && is_run_timestamp(stem)
        && stem.get(..4) == Some(*year))
    .then_some((*source, *year))
}

/// Whether `stem` is a valid UTC timestamp `YYYYMMDDTHHMMSSZ`.
fn is_run_timestamp(stem: &str) -> bool {
    let shape = stem.len() == 16
        && stem.bytes().enumerate().all(|(i, b)| match i {
            8 => b == b'T',
            15 => b == b'Z',
            _ => b.is_ascii_digit(),
        });
    if !shape {
        return false;
    }
    // Out-of-range fields become -1, which `DateTime::new` rejects.
    let field = |r: std::ops::Range<usize>| stem[r].parse::<i8>().unwrap_or(-1);
    let year = stem[..4].parse::<i16>().unwrap_or(-1);
    jiff::civil::DateTime::new(
        year,
        field(4..6),
        field(6..8),
        field(9..11),
        field(11..13),
        field(13..15),
        0,
    )
    .is_ok()
}

/// Whether `path` is `manual/<device>/<YYYY-MM>.beancount` (spec section 9).
fn is_month_file_of(path: &str, device: &str) -> bool {
    let Some(rest) = path
        .strip_prefix("manual/")
        .and_then(|r| r.strip_prefix(device))
        .and_then(|r| r.strip_prefix('/'))
        .and_then(|r| r.strip_suffix(".beancount"))
    else {
        return false;
    };
    let b = rest.as_bytes();
    b.len() == 7
        && b[4] == b'-'
        && b.iter()
            .enumerate()
            .all(|(i, c)| i == 4 || c.is_ascii_digit())
        && matches!(rest[5..].parse::<u8>(), Ok(1..=12))
}

/// Whether `path` is a ledger file in `manual/<device>/`.
fn is_manual_file_of(path: &str, device: &str) -> bool {
    path.strip_prefix("manual/")
        .and_then(|r| r.strip_prefix(device))
        .and_then(|r| r.strip_prefix('/'))
        .is_some_and(|name| !name.contains('/') && name.ends_with(".beancount"))
}

// ---------------------------------------------------------------------------
// The core's errors (spec section 13)

static ACCOUNT_NAME: LazyLock<Regex> = LazyLock::new(|| {
    #[allow(clippy::unwrap_used)] // A constant pattern.
    Regex::new(r"\p{Lu}[\p{L}\p{N}-]*(?::[\p{Lu}\p{N}][\p{L}\p{N}-]*)+").unwrap()
});

fn check_core_errors(snapshot: &Snapshot, ledger: &Ledger, findings: &mut Vec<Finding>) {
    for err in &ledger.errors {
        let at = |finding: Finding| match &err.file {
            Some(file) => finding.at(file.clone(), err.line),
            None => finding,
        };
        match err.code.as_str() {
            "LOAD" => load_error(snapshot, ledger, err, findings),
            "E3001" => findings.push(at(Finding::blocking(FindingCode::TransactionUnbalanced)
                .arg("detail", err.message.clone()))),
            "E1001" | "E1003" => {
                let account = ACCOUNT_NAME.find(&err.message).map_or("", |m| m.as_str());
                findings.push(at(Finding::blocking(FindingCode::AccountNotOpen)
                    .arg("account", account)
                    .arg("detail", err.message.clone())));
            }
            code => {
                let severity = if err.is_error {
                    Severity::Blocking
                } else {
                    Severity::Warning
                };
                findings.push(at(Finding::new(FindingCode::CoreError, severity)
                    .arg("code", code)
                    .arg("detail", err.message.clone())));
            }
        }
    }
}

/// Maps a loader error: path traversal, parse errors (located by parsing the
/// file again) and anything else that stops a file from loading.
fn load_error(
    snapshot: &Snapshot,
    ledger: &Ledger,
    err: &ledger::CoreError,
    findings: &mut Vec<Finding>,
) {
    if let Some(rest) = err.message.strip_prefix("path traversal not allowed: ") {
        let include = rest
            .split(" escapes base directory ")
            .next()
            .unwrap_or(rest);
        let mut finding =
            Finding::blocking(FindingCode::IncludeOutsideRepository).arg("include", include);
        if let Some(c) = ledger
            .constructs
            .iter()
            .find(|c| c.kind == ConstructKind::Include(include.to_owned()))
        {
            finding = finding.at(c.file.clone(), Some(c.line));
        }
        findings.push(finding);
        return;
    }
    if let Some(path) = err.message.strip_prefix("parse errors in ") {
        let file = snapshot
            .paths()
            .filter(|p| path.ends_with(&format!("/{p}")))
            .max_by_key(|p| p.len());
        if let Some((file, text)) = file.and_then(|f| snapshot.text(f).map(|t| (f, t))) {
            // Only the first error per file: what follows it depends on how
            // the parser recovers, and Python Beancount recovers differently.
            let parsed = rustledger_parser::parse_without_occurrences(text);
            if let Some(first) = parsed.errors.iter().min_by_key(|e| e.span.start) {
                findings.push(
                    Finding::blocking(FindingCode::ParseError)
                        .at(file, Some(line_of(text, first.span.start)))
                        .arg("detail", first.to_string()),
                );
                return;
            }
        }
    }
    let mut finding = Finding::blocking(FindingCode::ParseError).arg("detail", err.message.clone());
    if let Some(file) = &err.file {
        finding = finding.at(file.clone(), err.line);
    }
    findings.push(finding);
}

fn check_balance_failures(ledger: &Ledger, findings: &mut Vec<Finding>) {
    for failure in &ledger.balance_failures {
        let mut finding = Finding::warning(FindingCode::BalanceAssertionFailed)
            .arg("account", failure.account.clone())
            .arg("date", failure.date.to_string())
            .arg("asserted", failure.asserted.to_string())
            .arg("difference", failure.difference.to_string())
            .arg("currency", failure.currency.clone());
        if let Some(file) = &failure.file {
            finding = finding.at(file.clone(), failure.line);
        }
        findings.push(finding);
    }
}

// ---------------------------------------------------------------------------
// Identity (spec section 5)

fn check_unique_ids(ledger: &Ledger, findings: &mut Vec<Finding>) {
    let mut seen = BTreeSet::new();
    for txn in &ledger.transactions {
        let Some(id) = txn.id.as_deref() else {
            continue;
        };
        if !seen.insert(id) || ledger.dismissed_ids.contains(id) {
            findings.push(
                Finding::blocking(FindingCode::DuplicateLentilId)
                    .at(txn.file.clone(), Some(txn.line))
                    .arg("id", id),
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Profile (spec section 14)

fn check_profile(config: &Config, ledger: &Ledger, findings: &mut Vec<Finding>) {
    let outside = |construct: &str, file: &str, line: Option<u32>| {
        Finding::blocking(FindingCode::OutsideProfile)
            .at(file, line)
            .arg("construct", construct)
    };
    for c in &ledger.constructs {
        let in_main = c.file == config.main_file;
        match &c.kind {
            ConstructKind::Option { name, .. } => {
                if !in_main || !SUPPORTED_OPTIONS.contains(&name.as_str()) {
                    findings
                        .push(outside(construct::OPTION, &c.file, Some(c.line)).arg("name", name));
                }
            }
            ConstructKind::Include(include) => {
                // Includes of the main file are checked against spec section 3.
                if !in_main {
                    findings.push(
                        outside(construct::INCLUDE, &c.file, Some(c.line)).arg("include", include),
                    );
                }
            }
            ConstructKind::Plugin(name) => {
                findings.push(outside(construct::PLUGIN, &c.file, Some(c.line)).arg("name", name));
            }
            ConstructKind::TagStack => {
                findings.push(outside(construct::TAG_STACK, &c.file, Some(c.line)));
            }
        }
    }
    for txn in &ledger.transactions {
        let with_id = |f: Finding| match &txn.id {
            Some(id) => f.arg("id", id.clone()),
            None => f,
        };
        if txn.postings.iter().any(|p| p.has_cost) {
            findings.push(with_id(outside(construct::COST, &txn.file, Some(txn.line))));
        }
        if txn.postings.iter().filter(|p| p.units.is_none()).count() > 1 {
            findings.push(with_id(outside(
                construct::ELIDED_POSTINGS,
                &txn.file,
                Some(txn.line),
            )));
        }
    }
    for pad in &ledger.pads {
        if let Some(account) = [&pad.account, &pad.source_account]
            .into_iter()
            .find(|a| config.is_source_account(a))
        {
            findings.push(
                Finding::blocking(FindingCode::PadOnSourceAccount)
                    .at(pad.file.clone(), Some(pad.line))
                    .arg("account", account.clone()),
            );
        } else if !config.snapshots.iter().any(|s| s.account == pad.account) {
            findings.push(
                outside(construct::PAD, &pad.file, Some(pad.line))
                    .arg("account", pad.account.clone()),
            );
        }
    }
}

/// The main file holds exactly the two options and the includes of spec
/// section 3, and nothing else. Comments and blank lines do not count; other
/// options and plugins are reported by the profile check.
fn check_main_file(snapshot: &Snapshot, main_file: &str, findings: &mut Vec<Finding>) {
    let mismatch = |detail: &str, line: Option<u32>| {
        Finding::blocking(FindingCode::MainFileMismatch)
            .at(main_file, line)
            .arg("detail", detail)
    };
    let Some(text) = snapshot.text(main_file) else {
        findings.push(mismatch("missing", None));
        return;
    };
    let parsed = rustledger_parser::parse_without_occurrences(text);
    for name in SUPPORTED_OPTIONS {
        let count = parsed.options.iter().filter(|(n, _, _)| n == name).count();
        if count != 1 {
            findings.push(mismatch(name, None));
        }
    }
    let includes: Vec<&str> = parsed.includes.iter().map(|(p, _)| p.as_str()).collect();
    if includes != MAIN_INCLUDES {
        let line = parsed
            .includes
            .iter()
            .zip(MAIN_INCLUDES)
            .find(|((p, _), expected)| p != expected)
            .or_else(|| parsed.includes.get(MAIN_INCLUDES.len()).map(|i| (i, "")))
            .map(|((_, span), _)| line_of(text, span.start));
        findings.push(mismatch("includes", line));
    }
    if let Some(first) = parsed.directives.first() {
        findings.push(mismatch("directive", Some(line_of(text, first.span.start))));
    }
}

/// `fava.beancount` holds only Fava's custom directives (spec section 3).
/// Options, includes and plugins in it are reported by the profile check.
fn check_fava_file(snapshot: &Snapshot, findings: &mut Vec<Finding>) {
    let Some(text) = snapshot.text(FAVA_FILE) else {
        return;
    };
    let parsed = rustledger_parser::parse_without_occurrences(text);
    for d in &parsed.directives {
        let is_fava = matches!(
            &d.value,
            Directive::Custom(c) if c.custom_type == "fava-option" || c.custom_type == "fava-sidebar-link"
        );
        if !is_fava {
            findings.push(
                Finding::blocking(FindingCode::FavaFileMismatch)
                    .at(FAVA_FILE, Some(line_of(text, d.span.start))),
            );
        }
    }
}

/// Manual transactions never post to a source account (spec section 9).
fn check_manual_postings(config: &Config, ledger: &Ledger, findings: &mut Vec<Finding>) {
    for txn in ledger
        .transactions
        .iter()
        .filter(|t| t.file.starts_with("manual/"))
    {
        for posting in &txn.postings {
            if config.is_source_account(&posting.account) {
                let mut finding = Finding::blocking(FindingCode::ManualPostingOnSourceAccount)
                    .at(txn.file.clone(), Some(txn.line))
                    .arg("account", posting.account.clone());
                if let Some(id) = &txn.id {
                    finding = finding.arg("id", id.clone());
                }
                findings.push(finding);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Write areas (spec section 7)

/// Values of the `reason` argument of [`FindingCode::OutsideWriteArea`] and
/// [`FindingCode::ProtectedTransactionChanged`].
pub mod reason {
    /// A file the role may not create, change or remove.
    pub const PATH: &str = "path";
    /// A directive the role may not write or change here.
    pub const DIRECTIVE: &str = "directive";
    /// An importer wrote a transaction without flag `!`.
    pub const FLAG: &str = "flag";
    /// A new transaction lacks `lentil_id`.
    pub const MISSING_ID: &str = "missing-lentil-id";
    /// An imported transaction lacks `lentil_desc`.
    pub const MISSING_DESC: &str = "missing-lentil-desc";
    /// An imported transaction does not have exactly two postings, or an
    /// editor changed the number of postings.
    pub const POSTING_COUNT: &str = "posting-count";
    /// An imported transaction has no single source posting with an amount,
    /// or an editor changed the account of a source posting or moved a
    /// posting onto a source account.
    pub const SOURCE_POSTING: &str = "source-posting";
    /// An importer's balance assertion is not on the source account, or there is more than one.
    pub const BALANCE: &str = "balance";
    /// A transaction without `lentil_id` was changed, removed or added.
    pub const WITHOUT_ID: &str = "transaction-without-id";
    /// An editor added a transaction outside its own manual files.
    pub const NEW_TRANSACTION: &str = "new-transaction";
    /// An editor removed a comment, for example a dismissed transaction or a mark.
    pub const COMMENT: &str = "comment";
    /// An editor changed a posting flag.
    pub const POSTING_FLAG: &str = "posting-flag";
    /// A transaction's date changed.
    pub const DATE: &str = "date";
    /// A posting's amount, cost or price changed.
    pub const UNITS: &str = "units";
    /// A transaction was removed without being commented out.
    pub const REMOVED: &str = "removed";
}

fn check_write_areas(
    before: &Snapshot,
    after: &Snapshot,
    config: Option<&Config>,
    context: &Context<'_>,
    findings: &mut Vec<Finding>,
) {
    let role = context.role;
    if role == Role::Admin {
        return;
    }
    let mut areas = Areas {
        role,
        config,
        findings,
    };
    for change in after.changes_from(before) {
        match (role, &change) {
            (Role::Importer, Change::Added(path)) => {
                let source =
                    run_file_parts(path).and_then(|(id, _)| config.and_then(|c| c.source(id)));
                match (source, after.text(path)) {
                    (Some(source), Some(text)) => areas.check_run_file(path, text, source),
                    _ => areas.outside(path, None, reason::PATH),
                }
            }
            (Role::Editor, Change::Added(path)) => match (context.device, after.text(path)) {
                (Some(device), Some(text)) if is_month_file_of(path, device) => {
                    let parsed = rustledger_parser::parse_without_occurrences(text);
                    areas.check_header_constructs(path, text, 0, &parsed, None);
                    for d in &parsed.directives {
                        areas.check_new_entry(path, text, d);
                    }
                }
                _ => areas.outside(path, None, reason::PATH),
            },
            (Role::Editor, Change::Modified(path)) if path == ACCOUNTS_FILE => {
                let (old, new) = (before.text(path), after.text(path));
                match (old, new) {
                    (Some(old), Some(new)) => areas.check_accounts_append(path, old, new),
                    _ => areas.outside(path, None, reason::PATH),
                }
            }
            (Role::Editor, Change::Modified(path)) if is_data_file(path) => {
                let own = context.device.is_some_and(|d| is_manual_file_of(path, d));
                match (before.text(path), after.text(path)) {
                    (Some(old), Some(new)) => areas.check_edited_file(path, old, new, own),
                    _ => areas.outside(path, None, reason::PATH),
                }
            }
            (_, change) => areas.outside(change.path(), None, reason::PATH),
        }
    }
}

/// Ledger files with entries that editors may edit (spec section 10).
fn is_data_file(path: &str) -> bool {
    MAIN_INCLUDES[2..].iter().any(|p| glob_match(p, path))
}

struct Areas<'a, 'f> {
    role: Role,
    config: Option<&'a Config>,
    findings: &'f mut Vec<Finding>,
}

impl Areas<'_, '_> {
    fn outside(&mut self, path: &str, line: Option<u32>, reason: &str) {
        self.findings.push(
            Finding::blocking(FindingCode::OutsideWriteArea)
                .at(path, line)
                .arg("path", path)
                .arg("role", self.role.as_str())
                .arg("reason", reason),
        );
    }

    fn is_source_account(&self, account: &str) -> bool {
        self.config.is_some_and(|c| c.is_source_account(account))
    }

    fn missing_zero_tolerance(&mut self, path: &str, line: u32, account: &str) {
        self.findings.push(
            Finding::blocking(FindingCode::MissingZeroTolerance)
                .at(path, Some(line))
                .arg("account", account),
        );
    }

    /// Options, includes and plugins are never part of a role's change.
    /// With `before`, the ones already there may stay. `offset` is where the
    /// parsed text starts in `text`.
    fn check_header_constructs(
        &mut self,
        path: &str,
        text: &str,
        offset: usize,
        parsed: &ParseResult,
        before: Option<&ParseResult>,
    ) {
        if let Some(before) = before {
            let same = strip_spans(&before.options) == strip_spans(&parsed.options)
                && before
                    .includes
                    .iter()
                    .map(|(p, _)| p)
                    .eq(parsed.includes.iter().map(|(p, _)| p))
                && before
                    .plugins
                    .iter()
                    .map(|(n, c, _)| (n, c))
                    .eq(parsed.plugins.iter().map(|(n, c, _)| (n, c)));
            if !same {
                self.outside(path, None, reason::DIRECTIVE);
            }
            return;
        }
        let first = parsed
            .options
            .iter()
            .map(|(_, _, s)| s.start)
            .chain(parsed.includes.iter().map(|(_, s)| s.start))
            .chain(parsed.plugins.iter().map(|(_, _, s)| s.start))
            .min();
        if let Some(start) = first {
            self.outside(path, Some(line_of(text, offset + start)), reason::DIRECTIVE);
        }
    }

    /// A new run file (spec section 8).
    fn check_run_file(&mut self, path: &str, text: &str, source: &SourceConfig) {
        let parsed = rustledger_parser::parse_without_occurrences(text);
        self.check_header_constructs(path, text, 0, &parsed, None);
        let mut balances = 0;
        for d in &parsed.directives {
            let line = line_of(text, d.span.start);
            match &d.value {
                Directive::Transaction(t) => {
                    if t.flag != '!' {
                        self.outside(path, Some(line), reason::FLAG);
                    }
                    if meta_str(&t.meta, META_ID).is_none() {
                        self.outside(path, Some(line), reason::MISSING_ID);
                    }
                    if meta_str(&t.meta, META_DESC).is_none() {
                        self.outside(path, Some(line), reason::MISSING_DESC);
                    }
                    if t.postings.len() != 2 {
                        self.outside(path, Some(line), reason::POSTING_COUNT);
                    }
                    let source_postings = t
                        .postings
                        .iter()
                        .filter(|p| p.value.account.as_str() == source.account)
                        .collect::<Vec<_>>();
                    if source_postings.len() != 1 || source_postings[0].value.units.is_none() {
                        self.outside(path, Some(line), reason::SOURCE_POSTING);
                    }
                }
                Directive::Balance(b) => {
                    balances += 1;
                    if balances > 1 || b.account.as_str() != source.account {
                        self.outside(path, Some(line), reason::BALANCE);
                    }
                    if b.tolerance != Some(Decimal::ZERO) {
                        self.missing_zero_tolerance(path, line, b.account.as_str());
                    }
                }
                _ => self.outside(path, Some(line), reason::DIRECTIVE),
            }
        }
    }

    /// A directive an editor adds to its own manual files (spec section 9):
    /// transactions with `lentil_id`, balance assertions with zero tolerance
    /// and pads.
    fn check_new_entry(&mut self, path: &str, text: &str, d: &rustledger_core::Spanned<Directive>) {
        let line = line_of(text, d.span.start);
        match &d.value {
            Directive::Transaction(t) => {
                if meta_str(&t.meta, META_ID).is_none() {
                    self.outside(path, Some(line), reason::MISSING_ID);
                }
            }
            Directive::Balance(b) => {
                if b.tolerance != Some(Decimal::ZERO) {
                    self.missing_zero_tolerance(path, line, b.account.as_str());
                }
            }
            Directive::Pad(_) => {}
            _ => self.outside(path, Some(line), reason::DIRECTIVE),
        }
    }

    /// Editors may only append `open` directives to `accounts.beancount`.
    fn check_accounts_append(&mut self, path: &str, old: &str, new: &str) {
        let appended = new
            .strip_prefix(old)
            .filter(|rest| old.is_empty() || old.ends_with('\n') || rest.starts_with('\n'));
        let Some(appended) = appended else {
            self.outside(path, None, reason::DIRECTIVE);
            return;
        };
        let parsed = rustledger_parser::parse_without_occurrences(appended);
        let offset = old.len();
        self.check_header_constructs(path, new, offset, &parsed, None);
        for d in &parsed.directives {
            if !matches!(d.value, Directive::Open(_)) {
                self.outside(
                    path,
                    Some(line_of(new, offset + d.span.start)),
                    reason::DIRECTIVE,
                );
            }
        }
    }

    /// Editors change transactions only through the edit operations of spec
    /// section 10; in their own manual files they may also append entries.
    fn check_edited_file(&mut self, path: &str, old_text: &str, new_text: &str, own: bool) {
        let old = rustledger_parser::parse_without_occurrences(old_text);
        let new = rustledger_parser::parse_without_occurrences(new_text);
        self.check_header_constructs(path, new_text, 0, &new, Some(&old));

        // Directives other than transactions stay; own files may gain entries.
        let mut added: Vec<&rustledger_core::Spanned<Directive>> = new
            .directives
            .iter()
            .filter(|d| !matches!(d.value, Directive::Transaction(_)))
            .collect();
        for d in old
            .directives
            .iter()
            .filter(|d| !matches!(d.value, Directive::Transaction(_)))
        {
            match added.iter().position(|n| n.value == d.value) {
                Some(i) => {
                    added.remove(i);
                }
                None => self.outside(path, None, reason::DIRECTIVE),
            }
        }
        for d in added {
            if own {
                self.check_new_entry(path, new_text, d);
            } else {
                self.outside(
                    path,
                    Some(line_of(new_text, d.span.start)),
                    reason::DIRECTIVE,
                );
            }
        }

        // Transactions without lentil_id are read-only (spec section 5).
        let mut new_unnamed: Vec<(&Transaction, usize)> = transactions(&new)
            .filter(|(t, _)| meta_str(&t.meta, META_ID).is_none())
            .collect();
        let mut old_unnamed: Vec<&Transaction> = Vec::new();
        for (t, _) in transactions(&old).filter(|(t, _)| meta_str(&t.meta, META_ID).is_none()) {
            match new_unnamed.iter().position(|(n, _)| *n == t) {
                Some(i) => {
                    new_unnamed.remove(i);
                }
                None => old_unnamed.push(t),
            }
        }

        // Transactions with lentil_id, matched by ID. A transaction that lost
        // its ID or got another one is found by its date and postings.
        let old_named = named_transactions(&old);
        let new_named = named_transactions(&new);
        let old_dismissed = dismissed_ids(&old);
        let new_dismissed = dismissed_ids(&new);
        let mut new_unmatched: Vec<(&Transaction, usize)> = new_named
            .iter()
            .filter(|(id, _)| !old_named.contains_key(*id))
            .map(|(_, (t, start))| (*t, *start))
            .chain(new_unnamed)
            .collect();
        for (id, (old_txn, _)) in &old_named {
            if let Some((new_txn, start)) = new_named.get(id) {
                self.compare(path, id, old_txn, new_txn, line_of(new_text, *start));
            } else if new_dismissed.contains(*id) {
                // Dismissed: commented out in place (spec section 10). The
                // comment block must still hold the whole transaction, changed
                // at most as an editor may change it.
                let dismissed = dismissed_block(new_text, id);
                let parsed = dismissed
                    .as_ref()
                    .map(|(block, _)| rustledger_parser::parse(block));
                let txn = parsed.as_ref().and_then(|p| {
                    p.directives.iter().find_map(|d| match &d.value {
                        Directive::Transaction(t) if meta_str(&t.meta, META_ID) == Some(*id) => {
                            Some(t)
                        }
                        _ => None,
                    })
                });
                match (txn, &dismissed) {
                    (Some(t), Some((_, line))) => self.compare(path, id, old_txn, t, *line),
                    (_, dismissed) => self.protected_transaction(
                        path,
                        dismissed.as_ref().map(|(_, line)| *line),
                        id,
                        reason::REMOVED,
                    ),
                }
            } else if let Some(i) = new_unmatched
                .iter()
                .position(|(n, _)| same_entry(old_txn, n))
            {
                let (_, start) = new_unmatched.remove(i);
                self.protected_metadata(path, line_of(new_text, start), id, META_ID);
            } else {
                self.protected_transaction(path, None, id, reason::REMOVED);
            }
        }
        for (t, start) in new_unmatched {
            // Own manual files may gain entries with an ID; nothing may
            // restore a dismissed transaction or change one without an ID.
            let line = Some(line_of(new_text, start));
            match meta_str(&t.meta, META_ID) {
                Some(id) if own && !old_dismissed.contains(id) => {}
                Some(_) => self.outside(path, line, reason::NEW_TRANSACTION),
                None if old_unnamed.pop().is_some() || !own => {
                    self.outside(path, line, reason::WITHOUT_ID);
                }
                None => self.outside(path, line, reason::MISSING_ID),
            }
        }
        for _ in old_unnamed {
            self.outside(path, None, reason::WITHOUT_ID);
        }
        for id in &old_dismissed {
            if !new_dismissed.contains(id) && !new_named.contains_key(id.as_str()) {
                self.protected_transaction(path, None, id, reason::REMOVED);
            }
        }

        // Comments are never removed: they hold dismissed transactions and marks.
        let mut new_comments: BTreeMap<&str, usize> = BTreeMap::new();
        for c in &new.comments {
            *new_comments.entry(c.value.trim()).or_default() += 1;
        }
        for c in &old.comments {
            match new_comments.get_mut(c.value.trim()) {
                Some(n) if *n > 0 => *n -= 1,
                _ => {
                    self.outside(path, None, reason::COMMENT);
                    break;
                }
            }
        }
    }

    /// Compares one transaction before and after an editor's change.
    fn compare(&mut self, path: &str, id: &str, old: &Transaction, new: &Transaction, line: u32) {
        if old.date != new.date {
            self.protected_transaction(path, Some(line), id, reason::DATE);
        }
        for key in PROTECTED_META {
            if old.meta.get(key) != new.meta.get(key) {
                self.protected_metadata(path, line, id, key);
            }
        }
        if old.postings.len() != new.postings.len() {
            self.protected_transaction(path, Some(line), id, reason::POSTING_COUNT);
            return;
        }
        for (old_p, new_p) in old.postings.iter().zip(&new.postings) {
            let (old_p, new_p) = (&old_p.value, &new_p.value);
            if old_p.units != new_p.units || old_p.cost != new_p.cost || old_p.price != new_p.price
            {
                self.protected_transaction(path, Some(line), id, reason::UNITS);
            }
            if old_p.account != new_p.account
                && (self.is_source_account(old_p.account.as_str())
                    || self.is_source_account(new_p.account.as_str()))
            {
                self.protected_transaction(path, Some(line), id, reason::SOURCE_POSTING);
            }
            if old_p.flag != new_p.flag {
                self.outside(path, Some(line), reason::POSTING_FLAG);
            }
            for key in PROTECTED_META {
                if old_p.meta.get(key) != new_p.meta.get(key) {
                    self.protected_metadata(path, line, id, key);
                }
            }
        }
    }

    fn protected_transaction(&mut self, path: &str, line: Option<u32>, id: &str, reason: &str) {
        self.findings.push(
            Finding::blocking(FindingCode::ProtectedTransactionChanged)
                .at(path, line)
                .arg("id", id)
                .arg("reason", reason),
        );
    }

    fn protected_metadata(&mut self, path: &str, line: u32, id: &str, key: &str) {
        self.findings.push(
            Finding::blocking(FindingCode::ProtectedMetadataChanged)
                .at(path, Some(line))
                .arg("id", id)
                .arg("key", key),
        );
    }
}

fn strip_spans<S>(items: &[(String, String, S)]) -> Vec<(&str, &str)> {
    items
        .iter()
        .map(|(a, b, _)| (a.as_str(), b.as_str()))
        .collect()
}

/// Whether two transactions have the same date and postings, so one is the
/// other with changed metadata.
fn same_entry(old: &Transaction, new: &Transaction) -> bool {
    old.date == new.date
        && old.postings.len() == new.postings.len()
        && old.postings.iter().zip(&new.postings).all(|(o, n)| {
            o.value.account == n.value.account
                && o.value.units == n.value.units
                && o.value.cost == n.value.cost
                && o.value.price == n.value.price
        })
}

fn transactions(parsed: &ParseResult) -> impl Iterator<Item = (&Transaction, usize)> {
    parsed.directives.iter().filter_map(|d| match &d.value {
        Directive::Transaction(t) => Some((t, d.span.start)),
        _ => None,
    })
}

/// Transactions by `lentil_id`; the first one wins if an ID repeats.
fn named_transactions(parsed: &ParseResult) -> BTreeMap<&str, (&Transaction, usize)> {
    let mut named = BTreeMap::new();
    for (t, start) in transactions(parsed) {
        if let Some(id) = meta_str(&t.meta, META_ID) {
            named.entry(id).or_insert((t, start));
        }
    }
    named
}

static DISMISSED_ID: LazyLock<Regex> = LazyLock::new(|| {
    #[allow(clippy::unwrap_used)] // A constant pattern.
    Regex::new(r#"^;\s*lentil_id:\s*"([^"]+)""#).unwrap()
});

/// `lentil_id`s in comment lines: dismissed transactions (spec section 10).
/// The transaction a `dismiss` commented out (spec section 10): the comment
/// lines around the `lentil_id` line, from the header line `; <date> …` through
/// the last indented line, with the `; ` prefix removed, and the header's line.
fn dismissed_block(text: &str, id: &str) -> Option<(String, u32)> {
    let lines: Vec<&str> = text.lines().collect();
    let target = lines
        .iter()
        .position(|l| DISMISSED_ID.captures(l).is_some_and(|caps| &caps[1] == id))?;
    let mut start = target;
    loop {
        let body = lines[start].strip_prefix("; ")?;
        if body.starts_with(|c: char| c.is_ascii_digit()) {
            break;
        }
        if !body.starts_with([' ', '\t']) {
            return None;
        }
        start = start.checked_sub(1)?;
    }
    let mut end = target;
    while lines.get(end + 1).is_some_and(|l| {
        l.strip_prefix("; ")
            .is_some_and(|b| b.starts_with([' ', '\t']) && !b.trim().is_empty())
    }) {
        end += 1;
    }
    let mut block = String::new();
    for line in &lines[start..=end] {
        block.push_str(line.get(2..).unwrap_or_default());
        block.push('\n');
    }
    Some((block, u32::try_from(start + 1).unwrap_or(u32::MAX)))
}

fn dismissed_ids(parsed: &ParseResult) -> BTreeSet<String> {
    parsed
        .comments
        .iter()
        .filter_map(|c| {
            DISMISSED_ID
                .captures(&c.value)
                .map(|caps| caps[1].to_owned())
        })
        .collect()
}

fn meta_str<'m>(meta: &'m Metadata, key: &str) -> Option<&'m str> {
    match meta.get(key) {
        Some(MetaValue::String(s)) => Some(s),
        _ => None,
    }
}

/// 1-based line of a byte offset.
fn line_of(text: &str, offset: usize) -> u32 {
    let end = offset.min(text.len());
    let line = text.as_bytes()[..end]
        .iter()
        .filter(|&&b| b == b'\n')
        .count()
        + 1;
    u32::try_from(line).unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::{InitOptions, init_files};

    const DEVICE: &str = "k7p2q9xd";

    const CONFIG: &str = r#"spec_version = 0
main_file = "main.beancount"
fallback_account = "Expenses:Uncategorized"
unattributed_account = "Expenses:Unattributed"
price_currencies = []

[[source]]
id = "giro"
account = "Assets:Bank:Giro"
currency = "EUR"
importer = "csv"
mapping = "sources/giro.toml"

[[source]]
id = "card"
account = "Liabilities:Card"
currency = "EUR"
importer = "csv"
mapping = "sources/card.toml"

[[snapshot]]
account = "Assets:PaymentService"
currency = "EUR"
"#;

    const ACCOUNTS_EXTRA: &str = "2026-01-01 open Assets:Bank:Giro EUR
2026-01-01 open Liabilities:Card EUR
2026-01-01 open Assets:PaymentService EUR
2026-01-01 open Assets:Cash EUR
2026-01-01 open Expenses:Food
2026-01-01 open Expenses:Books
2026-01-01 open Income:Salary
";

    const GIRO_RUN: &str = "imports/giro/2026/20261020T071502Z.beancount";
    const GIRO_RUN_TEXT: &str = r#"; lentil import run
; source: giro, started: 2026-10-20T07:15:02Z, client: lentil-cli/0.1.0

2026-10-18 ! "Corner Grocery" "Card payment"
  lentil_id: "giro-3f9a1c2e7b4d"
  lentil_desc: "CORNER GROCERY 4711 CARD PAYMENT"
  Assets:Bank:Giro        -42.17 EUR
  Expenses:Uncategorized

2026-10-19 ! "City Utilities" "Electricity, monthly instalment"
  lentil_id: "giro-8c01d4e9a2f7"
  lentil_ref: "2026101912345"
  lentil_desc: "CITY UTILITIES ELECTRICITY INSTALMENT 10/2026"
  Assets:Bank:Giro        -64.00 EUR
  Expenses:Uncategorized

; 2026-10-19 ! "Card Company" "Card statement"
;   lentil_id: "giro-77aa88bb99cc"
;   lentil_desc: "CARD COMPANY STATEMENT 09/2026"
;   Assets:Bank:Giro        -20.00 EUR
;   Expenses:Uncategorized

2026-10-20 balance Assets:Bank:Giro  -106.17 ~ 0.00 EUR
"#;

    const CARD_RUN: &str = "imports/card/2026/20261021T080000Z.beancount";
    const CARD_RUN_TEXT: &str = r#"; lentil import run
; lentil: unchecked

2026-10-10 ! "Bookshop" "Books"
  lentil_id: "card-111111111111"
  lentil_desc: "BOOKSHOP 12 MAIN STREET"
  Liabilities:Card        -20.00 EUR
  Expenses:Books
"#;

    const MANUAL: &str = "manual/k7p2q9xd/2026-10.beancount";
    const MANUAL_TEXT: &str = r#"2026-10-05 * "Market" "Vegetables"
  lentil_id: "k7p2q9xd-01jb2m3n4p5q6r7s8t9v0w1x2y"
  Assets:Cash             -5.00 EUR
  Expenses:Food
"#;

    const OTHER_MANUAL: &str = "manual/zz9x8w7v/2026-09.beancount";
    const OTHER_MANUAL_TEXT: &str = r#"2026-09-12 * "Kiosk" "Newspaper"
  Assets:Cash             -2.50 EUR
  Expenses:Books
"#;

    const NEW_RUN: &str = "imports/giro/2026/20261101T060000Z.beancount";
    const NEW_RUN_TEXT: &str = r#"; lentil import run

2026-10-28 ! "Corner Grocery" "Card payment"
  lentil_id: "giro-aaaaaaaaaaa1"
  lentil_desc: "CORNER GROCERY 4711 CARD PAYMENT"
  Assets:Bank:Giro        -10.00 EUR
  Expenses:Uncategorized

2026-10-29 ! "Bakery" "Bread"
  lentil_id: "giro-aaaaaaaaaaa2"
  lentil_ref: "REF-2"
  lentil_desc: "BAKERY"
  Assets:Bank:Giro        -3.00 EUR
  Expenses:Food

2026-10-31 balance Assets:Bank:Giro  -119.17 ~ 0.00 EUR
"#;

    fn base() -> Snapshot {
        let mut files = init_files(&InitOptions {
            title: "Ledger".into(),
            currency: "EUR".into(),
        });
        files.insert(CONFIG_FILE, CONFIG);
        let accounts = format!("{}{ACCOUNTS_EXTRA}", files.text(ACCOUNTS_FILE).unwrap());
        files.insert(ACCOUNTS_FILE, accounts);
        files.insert("sources/giro.toml", "[file]\nheader = [\"Date\"]\n");
        files.insert("sources/card.toml", "[file]\nheader = [\"Date\"]\n");
        files.insert(GIRO_RUN, GIRO_RUN_TEXT);
        files.insert(CARD_RUN, CARD_RUN_TEXT);
        files.insert(MANUAL, MANUAL_TEXT);
        files.insert(OTHER_MANUAL, OTHER_MANUAL_TEXT);
        files
    }

    /// `files` with the first `from` in `path` replaced by `to`.
    fn with(files: &Snapshot, path: &str, from: &str, to: &str) -> Snapshot {
        let mut files = files.clone();
        let text = files.text(path).unwrap();
        assert!(text.contains(from), "{path} lacks {from:?}");
        let replaced = text.replacen(from, to, 1);
        files.insert(path, replaced);
        files
    }

    fn appended(files: &Snapshot, path: &str, text: &str) -> Snapshot {
        let mut files = files.clone();
        let old = files.text(path).unwrap_or_default().to_owned();
        files.insert(path, format!("{old}{text}"));
        files
    }

    fn inserted(files: &Snapshot, path: &str, text: impl Into<Vec<u8>>) -> Snapshot {
        let mut files = files.clone();
        files.insert(path, text);
        files
    }

    fn removed(files: &Snapshot, path: &str) -> Snapshot {
        let mut files = files.clone();
        assert!(files.remove(path).is_some(), "{path}");
        files
    }

    fn admin(after: &Snapshot) -> Validation {
        validate(
            after,
            &Context {
                role: Role::Admin,
                device: None,
                before: None,
            },
        )
    }

    fn as_role(role: Role, before: &Snapshot, after: &Snapshot) -> Validation {
        validate(
            after,
            &Context {
                role,
                device: Some(DEVICE),
                before: Some(before),
            },
        )
    }

    fn importer(after: &Snapshot) -> Validation {
        as_role(Role::Importer, &base(), after)
    }

    fn editor(after: &Snapshot) -> Validation {
        as_role(Role::Editor, &base(), after)
    }

    fn codes(v: &Validation) -> BTreeSet<FindingCode> {
        v.findings.iter().map(|f| f.code).collect()
    }

    fn assert_clean(v: &Validation) {
        assert!(v.findings.is_empty(), "{:#?}", v.findings);
    }

    /// The findings have exactly these codes.
    fn assert_codes(v: &Validation, expected: &[FindingCode]) {
        let expected: BTreeSet<FindingCode> = expected.iter().copied().collect();
        assert_eq!(codes(v), expected, "{:#?}", v.findings);
    }

    fn find(v: &Validation, code: FindingCode) -> &Finding {
        v.findings
            .iter()
            .find(|f| f.code == code)
            .unwrap_or_else(|| panic!("no {code:?} in {:#?}", v.findings))
    }

    fn args(v: &Validation, code: FindingCode, key: &str) -> Vec<String> {
        v.findings
            .iter()
            .filter(|f| f.code == code)
            .filter_map(|f| f.args.get(key).cloned())
            .collect()
    }

    /// Comments out the transaction whose header starts with `header`, as `dismiss` does.
    fn dismissed(files: &Snapshot, path: &str, header: &str) -> Snapshot {
        let text = files.text(path).unwrap();
        let start = text.find(header).unwrap();
        let end = text[start..]
            .find("\n\n")
            .map_or(text.len(), |e| start + e + 1);
        let block: String = text[start..end]
            .lines()
            .map(|l| format!("; {l}\n"))
            .collect();
        inserted(
            files,
            path,
            format!("{}{block}{}", &text[..start], &text[end..]),
        )
    }

    // --- Base states ------------------------------------------------------

    #[test]
    fn base_state_is_valid() {
        let v = admin(&base());
        assert_clean(&v);
        assert!(v.config.is_some());
        let ledger = v.ledger.unwrap();
        assert_eq!(ledger.transactions.len(), 5);
        assert!(!v.findings.iter().any(Finding::is_blocking));
    }

    #[test]
    fn init_files_are_valid_and_writable() {
        let files = init_files(&InitOptions {
            title: "My Ledger".into(),
            currency: "CHF".into(),
        });
        assert_clean(&admin(&files));
        assert!(read_only_reasons(&files).is_empty());
    }

    // --- Configuration (spec sections 2 and 4) ----------------------------

    #[test]
    fn missing_config_is_blocking() {
        let v = admin(&removed(&base(), CONFIG_FILE));
        assert_codes(&v, &[FindingCode::ConfigInvalid]);
        assert!(v.config.is_none() && v.ledger.is_none());
        assert!(v.is_blocking());
    }

    #[test]
    fn invalid_config_is_blocking() {
        let v = admin(&inserted(&base(), CONFIG_FILE, "spec_version = 0\n"));
        assert_codes(&v, &[FindingCode::ConfigInvalid]);
        let v = admin(&inserted(&base(), CONFIG_FILE, vec![0xff, 0xfe]));
        assert_codes(&v, &[FindingCode::ConfigInvalid]);
    }

    #[test]
    fn newer_spec_version_is_read_only() {
        let files = with(&base(), CONFIG_FILE, "spec_version = 0", "spec_version = 1");
        let v = admin(&files);
        assert_codes(&v, &[FindingCode::SpecVersionUnsupported]);
        let finding = find(&v, FindingCode::SpecVersionUnsupported);
        assert_eq!(finding.args["found"], "1");
        assert_eq!(finding.args["supported"], "0");
        let reasons = read_only_reasons(&files);
        assert_eq!(reasons.len(), 1);
        assert_eq!(reasons[0].code, FindingCode::SpecVersionUnsupported);
    }

    #[test]
    fn main_file_must_be_a_beancount_file_at_the_root() {
        for main in [
            "ledger/main.beancount",
            "main.txt",
            "../main.beancount",
            "Main.beancount",
        ] {
            let files = with(
                &base(),
                CONFIG_FILE,
                "main_file = \"main.beancount\"",
                &format!("main_file = {main:?}"),
            );
            assert_codes(&admin(&files), &[FindingCode::ConfigInvalid]);
        }
    }

    #[test]
    fn configured_main_file_is_used() {
        let mut files = with(
            &base(),
            CONFIG_FILE,
            "main_file = \"main.beancount\"",
            "main_file = \"ledger.beancount\"",
        );
        let main = files.remove("main.beancount").unwrap();
        files.insert("ledger.beancount", main);
        assert_clean(&admin(&files));
    }

    #[test]
    fn fallback_and_unattributed_accounts_must_be_open() {
        let files = with(
            &base(),
            ACCOUNTS_FILE,
            "2000-01-01 open Expenses:Unattributed\n",
            "",
        );
        let v = admin(&files);
        assert_codes(&v, &[FindingCode::AccountNotOpen]);
        let finding = find(&v, FindingCode::AccountNotOpen);
        assert_eq!(finding.file.as_deref(), Some(CONFIG_FILE));
        assert_eq!(finding.args["account"], "Expenses:Unattributed");
    }

    // --- Layout (spec section 3) ------------------------------------------

    #[test]
    fn ledger_files_outside_the_include_patterns_are_stray() {
        for path in [
            "notes.beancount",
            "imports/giro/extra.beancount",
            "imports/giro/2026/10/20261020T071502Z.beancount",
            "manual/k7p2q9xd/2026/2026-10.beancount",
            "manual/k7p2q9xd/.hidden.beancount",
            "sources/giro.beancount",
        ] {
            let v = admin(&inserted(&base(), path, "; nothing\n"));
            assert_codes(&v, &[FindingCode::StrayLedgerFile]);
            assert_eq!(find(&v, FindingCode::StrayLedgerFile).args["path"], path);
        }
    }

    #[test]
    fn names_use_lowercase_letters_digits_and_hyphens() {
        for path in [
            "imports/Giro/2026/20261020T071502Z.beancount",
            "imports/giro_x/2026/20261020T071502Z.beancount",
            "manual/K7P2Q9XD/2026-10.beancount",
            "manual/k7p2q9xd/Oct_2026.beancount",
            "prices/2026/2026.10.20.beancount",
            "sources/Giro.toml",
            "sources/.gitkeep",
            "status/bot 1.json",
        ] {
            let v = admin(&inserted(&base(), path, "; nothing\n"));
            assert_codes(&v, &[FindingCode::InvalidPath]);
        }
        // Fixed names, run timestamps, placeholders and files outside the
        // named directories are fine.
        for path in [
            "prices/2026/20261020.beancount",
            "manual/k7p2q9xd/2026-11.beancount",
            "sources/giro-2.toml",
            "status/bot-1.json",
            "LICENSE",
            "docs/Notes.md",
        ] {
            assert_clean(&admin(&inserted(&base(), path, "; nothing\n")));
        }
    }

    #[test]
    fn run_files_need_a_valid_timestamp_in_their_year() {
        for path in [
            "imports/giro/2025/20261020T071502Z.beancount",
            "imports/giro/2026/20261320T071502Z.beancount",
            "imports/giro/2026/20261020T251502Z.beancount",
            "imports/giro/2026/20261020t071502z.beancount",
            "imports/giro/2026/run-1.beancount",
            "imports/giro/2026/20261020T071502Z-2.beancount",
        ] {
            let v = admin(&inserted(&base(), path, "; nothing\n"));
            assert_codes(&v, &[FindingCode::InvalidPath]);
        }
    }

    #[test]
    fn load_caches_must_not_be_committed() {
        for path in [".main.beancount.cache", ".main.beancount.picklecache"] {
            let v = admin(&inserted(&base(), path, "x"));
            assert_codes(&v, &[FindingCode::LoadCacheCommitted]);
        }
    }

    #[test]
    fn main_file_must_equal_section_3() {
        let main = crate::layout::main_file("Ledger", "EUR");
        let variants = [
            main.replace(
                "include \"fava.beancount\"\ninclude \"accounts.beancount\"\n",
                "include \"accounts.beancount\"\ninclude \"fava.beancount\"\n",
            ),
            main.replace("include \"prices/*/*.beancount\"\n", ""),
            main.replace("manual/*/*.beancount", "manual/**/*.beancount"),
            main.replace("option \"title\" \"Ledger\"\n", ""),
            main.replace("option \"operating_currency\" \"EUR\"\n", ""),
            format!("{main}option \"operating_currency\" \"USD\"\n"),
            format!("{main}\n2026-01-01 open Assets:Other\n"),
        ];
        for variant in variants {
            let v = admin(&inserted(&base(), "main.beancount", variant.as_str()));
            assert!(
                codes(&v).contains(&FindingCode::MainFileMismatch),
                "{variant}\n{:#?}",
                v.findings
            );
        }
        // Any title and currency, and comments, are fine.
        let custom = format!(
            "; My ledger\n{}",
            crate::layout::main_file("Household \"2026\"", "USD")
        );
        assert_clean(&admin(&inserted(&base(), "main.beancount", custom)));
    }

    #[test]
    fn fava_file_holds_only_fava_custom_directives() {
        let files = appended(
            &base(),
            FAVA_FILE,
            "2000-01-01 custom \"fava-option\" \"language\" \"de\"\n",
        );
        assert_clean(&admin(&files));
        for extra in [
            "2000-01-01 custom \"budget\" Expenses:Food \"monthly\" 100.00 EUR\n",
            "2026-01-01 open Assets:Other\n",
            "2026-01-01 note Assets:Cash \"x\"\n",
        ] {
            let v = admin(&appended(&base(), FAVA_FILE, extra));
            assert_codes(&v, &[FindingCode::FavaFileMismatch]);
            assert_eq!(find(&v, FindingCode::FavaFileMismatch).line, Some(6));
        }
    }

    #[test]
    fn include_outside_the_repository_is_blocking() {
        let files = appended(
            &base(),
            "main.beancount",
            "include \"../outside.beancount\"\n",
        );
        let v = admin(&files);
        let finding = find(&v, FindingCode::IncludeOutsideRepository);
        assert_eq!(finding.args["include"], "../outside.beancount");
        assert_eq!(finding.file.as_deref(), Some("main.beancount"));
        assert_eq!(finding.line, Some(9));
        assert!(codes(&v).contains(&FindingCode::MainFileMismatch));
    }

    #[test]
    fn includes_outside_the_main_file_are_outside_the_profile() {
        let files = appended(
            &base(),
            MANUAL,
            "include \"../zz9x8w7v/2026-09.beancount\"\n",
        );
        let v = admin(&files);
        assert!(args(&v, FindingCode::OutsideProfile, "construct").contains(&"include".into()));
    }

    #[test]
    fn only_the_admin_changes_the_main_file() {
        let files = with(&base(), "main.beancount", "\"Ledger\"", "\"Other\"");
        assert_clean(&as_role(Role::Admin, &base(), &files));
        for role in [Role::Importer, Role::Editor] {
            let v = as_role(role, &base(), &files);
            assert_codes(&v, &[FindingCode::OutsideWriteArea]);
            assert_eq!(
                args(&v, FindingCode::OutsideWriteArea, "path"),
                ["main.beancount"]
            );
            assert_eq!(
                args(&v, FindingCode::OutsideWriteArea, "role"),
                [role.as_str()]
            );
        }
    }

    // --- The core's errors (spec section 13) ------------------------------

    #[test]
    fn parse_error_is_blocking_with_its_location() {
        let files = with(&base(), MANUAL, "2026-10-05 *", "2026-13-05 *");
        let v = admin(&files);
        let finding = find(&v, FindingCode::ParseError);
        assert_eq!(finding.file.as_deref(), Some(MANUAL));
        assert_eq!(finding.line, Some(1));
        assert!(v.is_blocking());
    }

    #[test]
    fn unbalanced_transaction_is_blocking() {
        let files = with(
            &base(),
            MANUAL,
            "  Expenses:Food\n",
            "  Expenses:Food  4.00 EUR\n",
        );
        let v = admin(&files);
        assert_codes(&v, &[FindingCode::TransactionUnbalanced]);
        let finding = find(&v, FindingCode::TransactionUnbalanced);
        assert_eq!(
            (finding.file.as_deref(), finding.line),
            (Some(MANUAL), Some(1))
        );
    }

    #[test]
    fn account_not_open_or_closed_is_blocking() {
        let files = with(&base(), MANUAL, "Expenses:Food", "Expenses:Restaurant");
        let v = admin(&files);
        assert_codes(&v, &[FindingCode::AccountNotOpen]);
        assert_eq!(
            args(&v, FindingCode::AccountNotOpen, "account"),
            ["Expenses:Restaurant"]
        );

        let files = appended(&base(), ACCOUNTS_FILE, "2026-10-01 close Expenses:Food\n");
        let v = admin(&files);
        assert_codes(&v, &[FindingCode::AccountNotOpen]);
        assert_eq!(
            args(&v, FindingCode::AccountNotOpen, "account"),
            ["Expenses:Food"]
        );
    }

    #[test]
    fn other_core_errors_are_blocking() {
        let files = appended(&base(), ACCOUNTS_FILE, "2026-02-01 open Expenses:Books\n");
        let v = admin(&files);
        assert_codes(&v, &[FindingCode::CoreError]);
        assert_eq!(args(&v, FindingCode::CoreError, "code"), ["E1002"]);
        assert!(v.is_blocking());

        // An unused pad (spec section 9).
        let files = appended(
            &base(),
            MANUAL,
            "\n2026-10-30 pad Assets:PaymentService Equity:OpeningBalances\n2026-10-31 balance Assets:PaymentService  0.00 ~ 0.00 EUR\n",
        );
        let v = admin(&files);
        assert_codes(&v, &[FindingCode::CoreError]);
        assert_eq!(args(&v, FindingCode::CoreError, "code"), ["E2003"]);
        assert!(v.is_blocking());
    }

    #[test]
    fn core_error_codes_map_to_findings() {
        let error = |code: &str, message: &str, is_error: bool| ledger::CoreError {
            code: code.into(),
            message: message.into(),
            file: Some("x.beancount".into()),
            line: Some(3),
            is_error,
        };
        let ledger = Ledger {
            errors: vec![
                error("E3001", "Transaction does not balance", true),
                error("E1001", "Account Expenses:Nope was never opened", true),
                error(
                    "E1003",
                    "Posting to inactive account Assets:Old on 2026-01-01",
                    true,
                ),
                error("E1002", "Account already open", true),
                error("E10002", "Entry dated in the future", false),
                error(
                    "LOAD",
                    "path traversal not allowed: ../x.beancount escapes base directory /repo",
                    true,
                ),
                error(
                    "LOAD",
                    "include pattern \"x/*.beancount\" does not match any files",
                    true,
                ),
            ],
            ..Ledger::default()
        };
        let mut findings = Vec::new();
        check_core_errors(&Snapshot::new(), &ledger, &mut findings);
        let summary: Vec<_> = findings.iter().map(|f| (f.code, f.severity)).collect();
        assert_eq!(
            summary,
            vec![
                (FindingCode::TransactionUnbalanced, Severity::Blocking),
                (FindingCode::AccountNotOpen, Severity::Blocking),
                (FindingCode::AccountNotOpen, Severity::Blocking),
                (FindingCode::CoreError, Severity::Blocking),
                (FindingCode::CoreError, Severity::Warning),
                (FindingCode::IncludeOutsideRepository, Severity::Blocking),
                (FindingCode::ParseError, Severity::Blocking),
            ]
        );
        assert_eq!(findings[1].args["account"], "Expenses:Nope");
        assert_eq!(findings[2].args["account"], "Assets:Old");
        assert_eq!(findings[4].args["code"], "E10002");
        assert_eq!(findings[5].args["include"], "../x.beancount");
        assert_eq!(findings[0].file.as_deref(), Some("x.beancount"));
        assert_eq!(findings[0].line, Some(3));
    }

    #[test]
    fn failing_balance_assertion_is_a_warning() {
        let files = with(&base(), GIRO_RUN, "-106.17 ~ 0.00", "-106.18 ~ 0.00");
        let v = admin(&files);
        assert_codes(&v, &[FindingCode::BalanceAssertionFailed]);
        assert!(!v.is_blocking());
        let finding = find(&v, FindingCode::BalanceAssertionFailed);
        assert_eq!(finding.severity, Severity::Warning);
        assert_eq!(
            (finding.file.as_deref(), finding.line),
            (Some(GIRO_RUN), Some(23))
        );
        assert_eq!(finding.args["account"], "Assets:Bank:Giro");
        assert_eq!(finding.args["date"], "2026-10-20");
        assert_eq!(finding.args["asserted"], "-106.18");
        assert_eq!(finding.args["difference"], "0.01");
        assert_eq!(finding.args["currency"], "EUR");
        assert!(read_only_reasons(&files).is_empty());
    }

    #[test]
    fn findings_are_ordered_blocking_first() {
        let files = with(&base(), GIRO_RUN, "-106.17 ~ 0.00", "-106.18 ~ 0.00");
        let files = with(
            &files,
            MANUAL,
            "  Expenses:Food\n",
            "  Expenses:Food  4.00 EUR\n",
        );
        let v = admin(&files);
        let severities: Vec<_> = v.findings.iter().map(|f| f.severity).collect();
        assert_eq!(severities, vec![Severity::Blocking, Severity::Warning]);
    }

    // --- Identity (spec section 5) ----------------------------------------

    #[test]
    fn lentil_ids_are_unique_among_active_transactions() {
        let copy = "\n2026-10-06 * \"Market\" \"Fruit\"\n  lentil_id: \"giro-3f9a1c2e7b4d\"\n  Assets:Cash  -1.00 EUR\n  Expenses:Food\n";
        let v = admin(&appended(&base(), MANUAL, copy));
        assert_codes(&v, &[FindingCode::DuplicateLentilId]);
        let finding = find(&v, FindingCode::DuplicateLentilId);
        assert_eq!(finding.args["id"], "giro-3f9a1c2e7b4d");
        assert_eq!(finding.file.as_deref(), Some(MANUAL));
    }

    #[test]
    fn lentil_ids_are_unique_among_active_and_dismissed_transactions() {
        let copy = "\n2026-10-06 * \"Market\" \"Fruit\"\n  lentil_id: \"giro-77aa88bb99cc\"\n  Assets:Cash  -1.00 EUR\n  Expenses:Food\n";
        let v = admin(&appended(&base(), MANUAL, copy));
        assert_codes(&v, &[FindingCode::DuplicateLentilId]);
        assert_eq!(
            args(&v, FindingCode::DuplicateLentilId, "id"),
            ["giro-77aa88bb99cc"]
        );
    }

    // --- Profile (spec section 14) ----------------------------------------

    #[test]
    fn options_other_than_title_and_operating_currency_are_outside() {
        let v = admin(&appended(
            &base(),
            "main.beancount",
            "option \"booking_method\" \"FIFO\"\n",
        ));
        assert_codes(&v, &[FindingCode::OutsideProfile]);
        assert_eq!(
            args(&v, FindingCode::OutsideProfile, "construct"),
            ["option"]
        );
        assert_eq!(
            args(&v, FindingCode::OutsideProfile, "name"),
            ["booking_method"]
        );

        // Options belong in the main file only.
        let v = admin(&appended(
            &base(),
            ACCOUNTS_FILE,
            "option \"title\" \"Other\"\n",
        ));
        assert!(
            codes(&v).contains(&FindingCode::OutsideProfile),
            "{:#?}",
            v.findings
        );
        assert_eq!(
            find(&v, FindingCode::OutsideProfile).file.as_deref(),
            Some(ACCOUNTS_FILE)
        );
    }

    #[test]
    fn plugins_are_outside() {
        let v = admin(&appended(
            &base(),
            "main.beancount",
            "plugin \"beancount.plugins.auto_accounts\"\n",
        ));
        assert_codes(&v, &[FindingCode::OutsideProfile]);
        assert_eq!(
            args(&v, FindingCode::OutsideProfile, "construct"),
            ["plugin"]
        );
        assert!(
            !read_only_reasons(&appended(
                &base(),
                "main.beancount",
                "plugin \"beancount.plugins.auto_accounts\"\n"
            ))
            .is_empty()
        );
    }

    #[test]
    fn tag_stacks_are_outside() {
        let files = inserted(
            &base(),
            MANUAL,
            format!("pushtag #trip\n{MANUAL_TEXT}poptag #trip\n"),
        );
        let v = admin(&files);
        assert_codes(&v, &[FindingCode::OutsideProfile]);
        assert_eq!(
            args(&v, FindingCode::OutsideProfile, "construct"),
            ["tag-stack", "tag-stack"]
        );
    }

    #[test]
    fn cost_annotations_are_outside() {
        let entry = "\n2026-10-07 * \"Broker\" \"Shares\"\n  lentil_id: \"k7p2q9xd-cost\"\n  Assets:Cash  2 ABC {5.00 EUR}\n  Expenses:Food  -10.00 EUR\n";
        let v = admin(&appended(&base(), MANUAL, entry));
        assert!(args(&v, FindingCode::OutsideProfile, "construct").contains(&"cost".into()));
        assert_eq!(
            args(&v, FindingCode::OutsideProfile, "id"),
            ["k7p2q9xd-cost"]
        );
    }

    #[test]
    fn at_most_one_elided_posting() {
        let entry = "\n2026-10-07 * \"Shop\" \"Split\"\n  lentil_id: \"k7p2q9xd-split\"\n  Assets:Cash  -10.00 EUR\n  Expenses:Food\n  Expenses:Books\n";
        let v = admin(&appended(&base(), MANUAL, entry));
        assert!(
            args(&v, FindingCode::OutsideProfile, "construct").contains(&"elided-postings".into()),
            "{:#?}",
            v.findings
        );
        let entry = "\n2026-10-07 * \"Shop\" \"Split\"\n  lentil_id: \"k7p2q9xd-split\"\n  Assets:Cash  -10.00 EUR\n  Expenses:Food  4.00 EUR\n  Expenses:Books\n";
        assert_clean(&admin(&appended(&base(), MANUAL, entry)));
    }

    #[test]
    fn supported_constructs_are_inside() {
        let accounts = "2026-01-01 commodity USD\n2026-01-01 open Expenses:Travel\n";
        let entries = r#"
2026-10-08 * "Hotel" "Two nights" #trip ^booking-1
  lentil_id: "k7p2q9xd-hotel"
  receipt: "kept"
  Assets:Cash             -200.00 EUR
  ! Expenses:Travel        216.00 USD @@ 200.00 EUR

2026-10-08 note Assets:Cash "Counted the cash"
2026-10-08 event "location" "Lisbon"
2026-10-08 custom "budget" Expenses:Food "monthly" 100.00 EUR
2026-10-08 query "food" "SELECT account, sum(position) WHERE account ~ 'Food'"
2026-10-08 price USD 0.92 EUR
txn-free comment lines are fine too
"#;
        let entries = entries.replace("txn-free comment lines are fine too\n", "; a comment\n");
        let files = appended(&base(), ACCOUNTS_FILE, accounts);
        let files = appended(&files, MANUAL, &entries);
        assert_clean(&admin(&files));
    }

    #[test]
    fn pads_only_on_snapshot_accounts() {
        let snapshot = "\n2026-10-30 pad Assets:PaymentService Equity:OpeningBalances\n2026-10-31 balance Assets:PaymentService  80.00 ~ 0.00 EUR\n";
        assert_clean(&admin(&appended(&base(), MANUAL, snapshot)));

        let other = "\n2026-10-30 pad Assets:Cash Equity:OpeningBalances\n2026-10-31 balance Assets:Cash  80.00 ~ 0.00 EUR\n";
        let v = admin(&appended(&base(), MANUAL, other));
        assert_codes(&v, &[FindingCode::OutsideProfile]);
        assert_eq!(args(&v, FindingCode::OutsideProfile, "construct"), ["pad"]);
        assert_eq!(
            args(&v, FindingCode::OutsideProfile, "account"),
            ["Assets:Cash"]
        );

        let source = "\n2026-10-25 pad Assets:Bank:Giro Equity:OpeningBalances\n2026-10-26 balance Assets:Bank:Giro  0.00 ~ 0.00 EUR\n";
        let v = admin(&appended(&base(), MANUAL, source));
        assert_codes(&v, &[FindingCode::PadOnSourceAccount]);
        assert_eq!(
            args(&v, FindingCode::PadOnSourceAccount, "account"),
            ["Assets:Bank:Giro"]
        );

        let from_source = "\n2026-10-30 pad Assets:PaymentService Assets:Bank:Giro\n2026-10-31 balance Assets:PaymentService  80.00 ~ 0.00 EUR\n";
        let v = admin(&appended(&base(), MANUAL, from_source));
        assert!(
            codes(&v).contains(&FindingCode::PadOnSourceAccount),
            "{:#?}",
            v.findings
        );
    }

    // --- Manual entries (spec section 9) ----------------------------------

    #[test]
    fn manual_transactions_never_post_to_source_accounts() {
        let files = with(&base(), MANUAL, "Assets:Cash", "Assets:Bank:Giro");
        let v = admin(&files);
        // The giro assertion now fails too, which is only a warning.
        assert_codes(
            &v,
            &[
                FindingCode::ManualPostingOnSourceAccount,
                FindingCode::BalanceAssertionFailed,
            ],
        );
        let finding = find(&v, FindingCode::ManualPostingOnSourceAccount);
        assert_eq!(finding.args["account"], "Assets:Bank:Giro");
        assert_eq!(finding.args["id"], "k7p2q9xd-01jb2m3n4p5q6r7s8t9v0w1x2y");
        assert_eq!(
            (finding.file.as_deref(), finding.line),
            (Some(MANUAL), Some(1))
        );
    }

    // --- Importer (spec sections 7 and 8) ---------------------------------

    #[test]
    fn importer_may_add_a_run_file() {
        let v = importer(&inserted(&base(), NEW_RUN, NEW_RUN_TEXT));
        assert_clean(&v);
    }

    #[test]
    fn importer_run_file_belongs_to_a_configured_source() {
        let path = "imports/savings/2026/20261101T060000Z.beancount";
        let v = importer(&inserted(&base(), path, NEW_RUN_TEXT));
        assert_codes(&v, &[FindingCode::OutsideWriteArea]);
        assert_eq!(args(&v, FindingCode::OutsideWriteArea, "reason"), ["path"]);
        assert_eq!(
            args(&v, FindingCode::OutsideWriteArea, "role"),
            ["importer"]
        );
    }

    #[test]
    fn importer_run_file_path_follows_section_8() {
        let path = "imports/giro/2026/run.beancount";
        let v = importer(&inserted(&base(), path, NEW_RUN_TEXT));
        assert_codes(
            &v,
            &[FindingCode::OutsideWriteArea, FindingCode::InvalidPath],
        );
    }

    #[test]
    fn importer_never_changes_existing_files() {
        let changed = [
            with(&base(), GIRO_RUN, "\"Card payment\"", "\"Groceries\""),
            removed(&base(), CARD_RUN),
            appended(&base(), ACCOUNTS_FILE, "2026-10-01 open Expenses:Gifts\n"),
            appended(&base(), "rules.toml", "# more\n"),
            inserted(
                &base(),
                "manual/k7p2q9xd/2026-11.beancount",
                MANUAL_TEXT
                    .replace("2026-10-05", "2026-11-05")
                    .replace("x2y", "x9y"),
            ),
        ];
        for files in changed {
            let v = importer(&files);
            assert_codes(&v, &[FindingCode::OutsideWriteArea]);
        }
        let v = importer(&inserted(&base(), "notes.beancount", "; x\n"));
        assert_codes(
            &v,
            &[FindingCode::OutsideWriteArea, FindingCode::StrayLedgerFile],
        );
    }

    #[test]
    fn importer_transactions_follow_section_8() {
        let cases = [
            ("2026-10-28 ! \"Corner", "2026-10-28 * \"Corner", "flag"),
            (
                "  lentil_id: \"giro-aaaaaaaaaaa1\"\n",
                "",
                "missing-lentil-id",
            ),
            ("  lentil_desc: \"BAKERY\"\n", "", "missing-lentil-desc"),
            (
                "  Expenses:Food\n",
                "  Expenses:Food  2.00 EUR\n  Expenses:Uncategorized\n",
                "posting-count",
            ),
            (
                "  Assets:Bank:Giro        -3.00 EUR\n",
                "  Assets:Cash        -3.00 EUR\n",
                "source-posting",
            ),
            (
                "; lentil import run\n",
                "2026-10-01 open Expenses:Gifts\n",
                "directive",
            ),
            (
                "; lentil import run\n",
                "option \"title\" \"x\"\n",
                "directive",
            ),
        ];
        for (from, to, reason) in cases {
            let text = NEW_RUN_TEXT.replacen(from, to, 1);
            assert_ne!(text, NEW_RUN_TEXT, "{from}");
            let v = importer(&inserted(&base(), NEW_RUN, text));
            let reasons = args(&v, FindingCode::OutsideWriteArea, "reason");
            assert!(
                reasons.contains(&reason.to_owned()),
                "{reason}: {:#?}",
                v.findings
            );
            let finding = find(&v, FindingCode::OutsideWriteArea);
            assert_eq!(finding.file.as_deref(), Some(NEW_RUN));
        }
    }

    #[test]
    fn importer_balance_assertions_carry_zero_tolerance() {
        for to in ["-119.17 EUR", "-119.17 ~ 0.01 EUR"] {
            let text = NEW_RUN_TEXT.replace("-119.17 ~ 0.00 EUR", to);
            let v = importer(&inserted(&base(), NEW_RUN, text));
            assert_codes(&v, &[FindingCode::MissingZeroTolerance]);
            let finding = find(&v, FindingCode::MissingZeroTolerance);
            assert_eq!(finding.args["account"], "Assets:Bank:Giro");
            assert_eq!(
                (finding.file.as_deref(), finding.line),
                (Some(NEW_RUN), Some(16))
            );
        }
        let second =
            format!("{NEW_RUN_TEXT}2026-11-01 balance Assets:Bank:Giro  -119.17 ~ 0.00 EUR\n");
        let v = importer(&inserted(&base(), NEW_RUN, second));
        assert_eq!(
            args(&v, FindingCode::OutsideWriteArea, "reason"),
            ["balance"]
        );
        let other = format!("{NEW_RUN_TEXT}2026-11-01 balance Assets:Cash  -5.00 ~ 0.00 EUR\n");
        let v = importer(&inserted(&base(), NEW_RUN, other));
        assert_eq!(
            args(&v, FindingCode::OutsideWriteArea, "reason"),
            ["balance"]
        );
    }

    #[test]
    fn first_commit_has_no_write_area_checks() {
        let files = inserted(
            &base(),
            NEW_RUN,
            NEW_RUN_TEXT.replace("! \"Bakery\"", "* \"Bakery\""),
        );
        let v = validate(
            &files,
            &Context {
                role: Role::Importer,
                device: None,
                before: None,
            },
        );
        assert_clean(&v);
    }

    // --- Editor (spec sections 7, 9 and 10) -------------------------------

    #[test]
    fn editor_may_confirm_set_accounts_and_edit_texts() {
        let files = with(
            &base(),
            GIRO_RUN,
            "2026-10-18 ! \"Corner Grocery\" \"Card payment\"",
            "2026-10-18 * \"Corner Shop\" \"Groceries\" #food ^receipt-1",
        );
        let files = with(
            &files,
            GIRO_RUN,
            "  Expenses:Uncategorized\n",
            "  Expenses:Food\n",
        );
        let files = with(
            &files,
            GIRO_RUN,
            "  lentil_ref: \"2026101912345\"\n",
            "  lentil_ref: \"2026101912345\"\n  note: \"annual contract\"\n",
        );
        let files = with(
            &files,
            MANUAL,
            "\"Market\" \"Vegetables\"",
            "\"Farmers Market\" \"Vegetables\"",
        );
        assert_clean(&editor(&files));
    }

    #[test]
    fn editor_may_dismiss_transactions() {
        let files = dismissed(&base(), GIRO_RUN, "2026-10-18 ! \"Corner Grocery\"");
        let v = editor(&files);
        // Dismissing removes the transaction from the balance, so the run's
        // assertion fails: a warning, never a block.
        assert_codes(&v, &[FindingCode::BalanceAssertionFailed]);
        assert!(!v.is_blocking());
    }

    #[test]
    fn editor_never_changes_protected_metadata() {
        let cases = [
            (
                "  lentil_desc: \"CORNER GROCERY 4711 CARD PAYMENT\"\n",
                "  lentil_desc: \"CORNER GROCERY\"\n",
                "lentil_desc",
            ),
            ("  lentil_ref: \"2026101912345\"\n", "", "lentil_ref"),
            (
                "  lentil_desc: \"CORNER GROCERY 4711 CARD PAYMENT\"\n",
                "  lentil_desc: \"CORNER GROCERY 4711 CARD PAYMENT\"\n  lentil_ref: \"NEW\"\n",
                "lentil_ref",
            ),
            (
                "\"giro-3f9a1c2e7b4d\"",
                "\"giro-000000000000\"",
                "lentil_id",
            ),
            ("  lentil_id: \"giro-3f9a1c2e7b4d\"\n", "", "lentil_id"),
        ];
        for (from, to, key) in cases {
            let v = editor(&with(&base(), GIRO_RUN, from, to));
            assert_codes(&v, &[FindingCode::ProtectedMetadataChanged]);
            assert_eq!(
                args(&v, FindingCode::ProtectedMetadataChanged, "key"),
                [key],
                "{to}"
            );
            let finding = find(&v, FindingCode::ProtectedMetadataChanged);
            assert_eq!(finding.file.as_deref(), Some(GIRO_RUN));
        }
    }

    #[test]
    fn a_bare_id_comment_is_no_dismissal() {
        let block = "2026-10-18 ! \"Corner Grocery\" \"Card payment\"
  lentil_id: \"giro-3f9a1c2e7b4d\"
  lentil_desc: \"CORNER GROCERY 4711 CARD PAYMENT\"
  Assets:Bank:Giro        -42.17 EUR
  Expenses:Uncategorized
";
        let commented: String = block.lines().map(|l| format!("; {l}\n")).collect();
        let dismissed = with(&base(), GIRO_RUN, block, &commented);
        assert!(
            !editor(&dismissed).is_blocking(),
            "{:#?}",
            editor(&dismissed).findings
        );

        let truncated = with(
            &base(),
            GIRO_RUN,
            block,
            "; lentil_id: \"giro-3f9a1c2e7b4d\"\n",
        );
        let v = editor(&truncated);
        assert!(
            args(&v, FindingCode::ProtectedTransactionChanged, "reason")
                .contains(&"removed".to_owned()),
            "{:#?}",
            v.findings
        );

        let changed = with(
            &base(),
            GIRO_RUN,
            block,
            &commented.replace("-42.17", "-4.17"),
        );
        let v = editor(&changed);
        assert!(
            args(&v, FindingCode::ProtectedTransactionChanged, "reason")
                .contains(&"units".to_owned()),
            "{:#?}",
            v.findings
        );
    }

    #[test]
    fn editor_never_changes_dates_amounts_or_source_postings() {
        let cases = [
            ("2026-10-18 !", "2026-10-17 !", "date"),
            ("-42.17 EUR", "-42.71 EUR", "units"),
            (
                "  Expenses:Uncategorized\n",
                "  Expenses:Uncategorized  42.17 EUR\n",
                "units",
            ),
            (
                "  Assets:Bank:Giro        -42.17 EUR",
                "  Assets:Cash        -42.17 EUR",
                "source-posting",
            ),
            (
                "  Expenses:Uncategorized\n",
                "  Assets:Bank:Giro\n",
                "source-posting",
            ),
            (
                "  Expenses:Uncategorized\n",
                "  Liabilities:Card\n",
                "source-posting",
            ),
            (
                "  Expenses:Uncategorized\n",
                "  Expenses:Uncategorized  40.00 EUR\n  Expenses:Food\n",
                "posting-count",
            ),
        ];
        for (from, to, reason) in cases {
            let v = editor(&with(&base(), GIRO_RUN, from, to));
            let reasons = args(&v, FindingCode::ProtectedTransactionChanged, "reason");
            assert!(
                reasons.contains(&reason.to_owned()),
                "{reason}: {:#?}",
                v.findings
            );
            assert_eq!(
                args(&v, FindingCode::ProtectedTransactionChanged, "id")[0],
                "giro-3f9a1c2e7b4d"
            );
        }
    }

    #[test]
    fn editor_never_deletes_transactions() {
        let text = GIRO_RUN_TEXT.replace(
            "2026-10-18 ! \"Corner Grocery\" \"Card payment\"\n  lentil_id: \"giro-3f9a1c2e7b4d\"\n  lentil_desc: \"CORNER GROCERY 4711 CARD PAYMENT\"\n  Assets:Bank:Giro        -42.17 EUR\n  Expenses:Uncategorized\n\n",
            "",
        );
        assert_ne!(text, GIRO_RUN_TEXT);
        let v = editor(&inserted(&base(), GIRO_RUN, text));
        assert!(
            codes(&v).contains(&FindingCode::ProtectedTransactionChanged),
            "{:#?}",
            v.findings
        );
        assert_eq!(
            args(&v, FindingCode::ProtectedTransactionChanged, "reason"),
            ["removed"]
        );
        assert_eq!(
            args(&v, FindingCode::ProtectedTransactionChanged, "id"),
            ["giro-3f9a1c2e7b4d"]
        );
    }

    #[test]
    fn editor_never_deletes_or_restores_dismissed_transactions() {
        let start = GIRO_RUN_TEXT
            .find("; 2026-10-19 ! \"Card Company\"")
            .unwrap();
        let end = GIRO_RUN_TEXT.find("\n2026-10-20 balance").unwrap();
        let deleted = format!("{}{}", &GIRO_RUN_TEXT[..start], &GIRO_RUN_TEXT[end + 1..]);
        let v = editor(&inserted(&base(), GIRO_RUN, deleted));
        assert!(
            args(&v, FindingCode::ProtectedTransactionChanged, "reason")
                .contains(&"removed".into())
        );
        assert!(args(&v, FindingCode::OutsideWriteArea, "reason").contains(&"comment".into()));

        let restored: String = GIRO_RUN_TEXT[start..end]
            .lines()
            .map(|l| format!("{}\n", l.strip_prefix("; ").unwrap_or(l)))
            .collect();
        let restored = format!(
            "{}{restored}{}",
            &GIRO_RUN_TEXT[..start],
            &GIRO_RUN_TEXT[end..]
        );
        let v = editor(&inserted(&base(), GIRO_RUN, restored));
        assert!(
            args(&v, FindingCode::OutsideWriteArea, "reason").contains(&"new-transaction".into()),
            "{:#?}",
            v.findings
        );
    }

    #[test]
    fn editor_never_changes_other_directives_or_adds_transactions_to_runs() {
        let v = editor(&with(&base(), GIRO_RUN, "-106.17 ~ 0.00", "-106.00 ~ 0.00"));
        assert!(args(&v, FindingCode::OutsideWriteArea, "reason").contains(&"directive".into()));

        let v = editor(&appended(
            &base(),
            GIRO_RUN,
            "2026-10-21 note Assets:Bank:Giro \"x\"\n",
        ));
        assert_eq!(
            args(&v, FindingCode::OutsideWriteArea, "reason"),
            ["directive"]
        );

        let extra = "\n2026-10-21 * \"Shop\" \"x\"\n  lentil_id: \"k7p2q9xd-extra\"\n  Assets:Cash  -1.00 EUR\n  Expenses:Food\n";
        let v = editor(&appended(&base(), GIRO_RUN, extra));
        assert_codes(&v, &[FindingCode::OutsideWriteArea]);
        assert_eq!(
            args(&v, FindingCode::OutsideWriteArea, "reason"),
            ["new-transaction"]
        );

        let v = editor(&appended(&base(), GIRO_RUN, "option \"title\" \"x\"\n"));
        assert!(args(&v, FindingCode::OutsideWriteArea, "reason").contains(&"directive".into()));
    }

    #[test]
    fn editor_never_changes_transactions_without_id() {
        let v = editor(&with(
            &base(),
            OTHER_MANUAL,
            "\"Newspaper\"",
            "\"Magazine\"",
        ));
        assert_codes(&v, &[FindingCode::OutsideWriteArea]);
        assert_eq!(
            args(&v, FindingCode::OutsideWriteArea, "reason"),
            ["transaction-without-id"]
        );
    }

    #[test]
    fn editor_never_changes_posting_flags() {
        let v = editor(&with(
            &base(),
            GIRO_RUN,
            "  Expenses:Uncategorized\n",
            "  ! Expenses:Uncategorized\n",
        ));
        assert_codes(&v, &[FindingCode::OutsideWriteArea]);
        assert_eq!(
            args(&v, FindingCode::OutsideWriteArea, "reason"),
            ["posting-flag"]
        );
    }

    #[test]
    fn editor_never_removes_comments() {
        let v = editor(&with(&base(), CARD_RUN, "; lentil: unchecked\n", ""));
        assert_codes(&v, &[FindingCode::OutsideWriteArea]);
        assert_eq!(
            args(&v, FindingCode::OutsideWriteArea, "reason"),
            ["comment"]
        );
    }

    #[test]
    fn editor_may_append_open_directives() {
        let files = appended(&base(), ACCOUNTS_FILE, "2026-10-30 open Expenses:Gifts\n");
        assert_clean(&editor(&files));
        // Also after a file that lacks its final line break.
        let mut before = base();
        let text = before.text(ACCOUNTS_FILE).unwrap().trim_end().to_owned();
        before.insert(ACCOUNTS_FILE, text.clone());
        let after = inserted(
            &before,
            ACCOUNTS_FILE,
            format!("{text}\n2026-10-30 open Expenses:Gifts\n"),
        );
        assert_clean(&as_role(Role::Editor, &before, &after));
    }

    #[test]
    fn editor_only_appends_open_directives_to_accounts() {
        let cases = [
            appended(&base(), ACCOUNTS_FILE, "2026-10-30 close Expenses:Books\n"),
            appended(&base(), ACCOUNTS_FILE, "2026-10-30 commodity USD\n"),
            with(
                &base(),
                ACCOUNTS_FILE,
                "open Assets:Cash EUR",
                "open Assets:Cash EUR,USD",
            ),
            with(
                &base(),
                ACCOUNTS_FILE,
                "2026-01-01 open Income:Salary\n",
                "",
            ),
        ];
        for files in cases {
            let v = editor(&files);
            assert!(
                codes(&v).contains(&FindingCode::OutsideWriteArea),
                "{:#?}",
                v.findings
            );
            assert_eq!(
                find(&v, FindingCode::OutsideWriteArea).args["path"],
                ACCOUNTS_FILE
            );
        }
        let v = editor(&appended(
            &base(),
            ACCOUNTS_FILE,
            "option \"title\" \"x\"\n",
        ));
        assert!(args(&v, FindingCode::OutsideWriteArea, "reason").contains(&"directive".into()));
    }

    #[test]
    fn editor_may_write_its_own_manual_files() {
        let new_file = r#"2026-11-02 * "Market" "Fruit"
  lentil_id: "k7p2q9xd-01jb2m3n4p5q6r7s8t9v0w1x3z"
  Assets:Cash             -3.00 EUR
  Expenses:Food

2026-10-31 pad Assets:PaymentService Expenses:Unattributed
2026-11-01 balance Assets:PaymentService  80.00 ~ 0.00 EUR
"#;
        let files = inserted(&base(), "manual/k7p2q9xd/2026-11.beancount", new_file);
        assert_clean(&editor(&files));

        let entry = "\n2026-10-06 * \"Bakery\" \"Bread\"\n  lentil_id: \"k7p2q9xd-01jb2m3n4p5q6r7s8t9v0w1x4a\"\n  Assets:Cash  -2.00 EUR\n  Expenses:Food\n";
        assert_clean(&editor(&appended(&base(), MANUAL, entry)));
    }

    #[test]
    fn editor_never_writes_other_devices_files() {
        let path = "manual/zz9x8w7v/2026-11.beancount";
        let v = editor(&inserted(&base(), path, MANUAL_TEXT));
        assert!(args(&v, FindingCode::OutsideWriteArea, "path").contains(&path.to_owned()));

        let entry = "\n2026-09-13 * \"Bakery\" \"Bread\"\n  lentil_id: \"k7p2q9xd-other\"\n  Assets:Cash  -2.00 EUR\n  Expenses:Food\n";
        let v = editor(&appended(&base(), OTHER_MANUAL, entry));
        assert_eq!(
            args(&v, FindingCode::OutsideWriteArea, "reason"),
            ["new-transaction"]
        );

        let own_path = "manual/k7p2q9xd/2026-11.beancount";
        let files = inserted(
            &base(),
            own_path,
            MANUAL_TEXT
                .replace("2026-10-05", "2026-11-05")
                .replace("x2y", "x9y"),
        );
        let v = validate(
            &files,
            &Context {
                role: Role::Editor,
                device: None,
                before: Some(&base()),
            },
        );
        assert_codes(&v, &[FindingCode::OutsideWriteArea]);

        let v = editor(&inserted(
            &base(),
            "manual/k7p2q9xd/notes.beancount",
            "; notes\n",
        ));
        assert_codes(&v, &[FindingCode::OutsideWriteArea]);
    }

    #[test]
    fn editor_entries_carry_ids_and_zero_tolerance() {
        let path = "manual/k7p2q9xd/2026-11.beancount";
        let no_id =
            "2026-11-02 * \"Market\" \"Fruit\"\n  Assets:Cash  -3.00 EUR\n  Expenses:Food\n";
        let v = editor(&inserted(&base(), path, no_id));
        assert_eq!(
            args(&v, FindingCode::OutsideWriteArea, "reason"),
            ["missing-lentil-id"]
        );

        let entry = format!("\n{no_id}");
        let v = editor(&appended(&base(), MANUAL, &entry));
        assert_eq!(
            args(&v, FindingCode::OutsideWriteArea, "reason"),
            ["missing-lentil-id"]
        );

        let snapshot = "2026-10-31 pad Assets:PaymentService Expenses:Unattributed\n2026-11-01 balance Assets:PaymentService  80.00 EUR\n";
        let v = editor(&inserted(&base(), path, snapshot));
        assert_codes(&v, &[FindingCode::MissingZeroTolerance]);
        let v = editor(&appended(&base(), MANUAL, &format!("\n{snapshot}")));
        assert_codes(&v, &[FindingCode::MissingZeroTolerance]);

        let v = editor(&inserted(&base(), path, "2026-11-01 open Expenses:Gifts\n"));
        assert_eq!(
            args(&v, FindingCode::OutsideWriteArea, "reason"),
            ["directive"]
        );
    }

    #[test]
    fn editor_never_posts_manual_entries_or_pads_to_source_accounts() {
        let path = "manual/k7p2q9xd/2026-11.beancount";
        let entry = "2026-11-02 * \"ATM\" \"Cash\"\n  lentil_id: \"k7p2q9xd-atm\"\n  Assets:Bank:Giro  -50.00 EUR\n  Assets:Cash\n";
        let v = editor(&inserted(&base(), path, entry));
        assert_codes(&v, &[FindingCode::ManualPostingOnSourceAccount]);

        let pad = "2026-10-25 pad Assets:Bank:Giro Expenses:Unattributed\n2026-10-26 balance Assets:Bank:Giro  0.00 ~ 0.00 EUR\n";
        let v = editor(&inserted(&base(), path, pad));
        assert!(
            codes(&v).contains(&FindingCode::PadOnSourceAccount),
            "{:#?}",
            v.findings
        );
    }

    #[test]
    fn editor_never_changes_configuration_or_removes_files() {
        let cases = [
            with(
                &base(),
                CONFIG_FILE,
                "price_currencies = []",
                "price_currencies = [\"USD\"]",
            ),
            appended(&base(), "rules.toml", "# more\n"),
            appended(&base(), "sources/giro.toml", "# more\n"),
            appended(&base(), FAVA_FILE, "; more\n"),
            removed(&base(), CARD_RUN),
            removed(&base(), "README.md"),
        ];
        for files in cases {
            let v = editor(&files);
            assert_codes(&v, &[FindingCode::OutsideWriteArea]);
            assert_eq!(args(&v, FindingCode::OutsideWriteArea, "reason"), ["path"]);
        }
    }

    // --- Admin (spec section 7) -------------------------------------------

    #[test]
    fn admin_may_change_anything_that_keeps_the_spec() {
        let files = with(&base(), GIRO_RUN, "-42.17 EUR", "-42.71 EUR");
        let files = with(&files, GIRO_RUN, "-106.17 ~ 0.00", "-106.71 ~ 0.00");
        let files = with(&files, GIRO_RUN, "  lentil_ref: \"2026101912345\"\n", "");
        let files = appended(&files, "rules.toml", "# more\n");
        let files = removed(&files, CARD_RUN);
        let files = with(&files, ACCOUNTS_FILE, "2026-01-01 open Income:Salary\n", "");
        // The zero tolerance rule is the importer's; the admin may write any assertion.
        let files = appended(
            &files,
            MANUAL,
            "\n2026-10-06 balance Assets:Cash  -7.50 EUR\n",
        );
        assert_clean(&as_role(Role::Admin, &base(), &files));
    }

    #[test]
    fn admin_must_not_break_the_spec() {
        let files = appended(
            &base(),
            "main.beancount",
            "plugin \"beancount.plugins.auto_accounts\"\n",
        );
        let v = as_role(Role::Admin, &base(), &files);
        assert_codes(&v, &[FindingCode::OutsideProfile]);
        assert!(v.is_blocking());
    }

    // --- Read-only states (spec section 13) -------------------------------

    #[test]
    fn read_only_reasons_are_the_blocking_findings_of_the_state() {
        assert!(read_only_reasons(&base()).is_empty());
        let reasons = read_only_reasons(&removed(&base(), CONFIG_FILE));
        assert_eq!(reasons[0].code, FindingCode::ConfigInvalid);
        let files = with(
            &base(),
            MANUAL,
            "  Expenses:Food\n",
            "  Expenses:Food  4.00 EUR\n",
        );
        let files = with(&files, GIRO_RUN, "-106.17 ~ 0.00", "-106.18 ~ 0.00");
        let reasons = read_only_reasons(&files);
        assert_eq!(reasons.len(), 1);
        assert_eq!(reasons[0].code, FindingCode::TransactionUnbalanced);
    }

    #[test]
    fn run_timestamps_are_checked_as_dates() {
        assert!(is_run_timestamp("20261020T071502Z"));
        assert!(is_run_timestamp("20240229T235959Z"));
        assert!(!is_run_timestamp("20230229T000000Z"));
        assert!(!is_run_timestamp("20261020T076002Z"));
        assert!(!is_run_timestamp("2026102OT071502Z"));
        assert!(!is_run_timestamp("20261020 071502Z"));
        assert!(is_month_file_of(
            "manual/k7p2q9xd/2026-10.beancount",
            DEVICE
        ));
        assert!(!is_month_file_of(
            "manual/k7p2q9xd/2026-13.beancount",
            DEVICE
        ));
        assert!(!is_month_file_of(
            "manual/k7p2q9xd/2026-1.beancount",
            DEVICE
        ));
        assert!(!is_month_file_of("manual/other/2026-10.beancount", DEVICE));
    }
}
