//! Loading a ledger from a [`Snapshot`] and indexing what Lentil addresses.
//!
//! The ledger is loaded twice over the same files: rustledger's loader books
//! and validates it (errors, failing balance assertions), and the parser reads
//! each file on its own, so every transaction keeps the exact byte ranges that
//! edits need (spec section 10). Review state comes from the parsed directive's
//! flag, never from query results (spec section 6).

use std::collections::{BTreeMap, BTreeSet};
use std::ops::Range;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, LazyLock};

use jiff::civil::Date;
use regex::Regex;
use rust_decimal::Decimal;
use rustledger_core::{Directive, IncompleteAmount, MetaValue, Metadata};
use rustledger_loader::{ErrorSeverity, FileSystem, LoadError, LoadOptions};

use crate::snapshot::{Snapshot, glob_match};

/// Metadata key of the stable transaction ID (spec section 5).
pub const META_ID: &str = "lentil_id";
/// Metadata key of the bank's reference.
pub const META_REF: &str = "lentil_ref";
/// Metadata key of the bank's original description.
pub const META_DESC: &str = "lentil_desc";
/// Keys no client may change or remove.
pub const PROTECTED_META: [&str; 3] = [META_ID, META_REF, META_DESC];

/// Virtual root under which snapshot files are handed to the loader.
const ROOT: &str = "/repo";

/// One transaction as written in its file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Txn {
    /// `lentil_id`, if the transaction has one.
    pub id: Option<String>,
    /// Repository-relative file.
    pub file: String,
    /// Bytes of the transaction: header, metadata, postings and indented
    /// comment lines, without the line break after the last line.
    pub span: Range<usize>,
    /// 1-based line of the header.
    pub line: u32,
    /// Byte offset of the flag character, or `None` if the header uses `txn`.
    pub flag_offset: Option<usize>,
    /// Transaction date.
    pub date: Date,
    /// Flag as parsed: `!` open, `*` reviewed (spec section 6).
    pub flag: char,
    /// Payee, if any.
    pub payee: Option<String>,
    /// Narration.
    pub narration: String,
    /// `lentil_desc`, the bank's description.
    pub desc: Option<String>,
    /// `lentil_ref`, the bank's reference.
    pub reference: Option<String>,
    /// Tags without `#`.
    pub tags: Vec<String>,
    /// Links without `^`.
    pub links: Vec<String>,
    /// Postings in file order.
    pub postings: Vec<Posting>,
}

/// One posting as written in its file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Posting {
    /// Account name.
    pub account: String,
    /// Bytes of the account name in the file.
    pub account_span: Range<usize>,
    /// Bytes of the posting line, from its indent through a trailing comment.
    pub span: Range<usize>,
    /// Number and currency, if both are written. `None` for an elided amount.
    pub units: Option<(Decimal, String)>,
    /// Whether the posting has an amount at all (possibly incomplete).
    pub has_units: bool,
    /// Whether the posting has a cost annotation `{...}` (outside the profile).
    pub has_cost: bool,
    /// Whether the posting has a price annotation `@` or `@@`.
    pub has_price: bool,
    /// Posting flag, preserved without meaning (spec section 6).
    pub flag: Option<char>,
}

/// One `balance` directive as written.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BalanceAssertion {
    /// Repository-relative file.
    pub file: String,
    /// 1-based line.
    pub line: u32,
    /// Assertion date; it holds at the start of this day.
    pub date: Date,
    /// Asserted account.
    pub account: String,
    /// Asserted number.
    pub amount: Decimal,
    /// Asserted currency.
    pub currency: String,
    /// Explicit tolerance, `Some(0)` for `~ 0.00`.
    pub tolerance: Option<Decimal>,
}

/// One `pad` directive as written.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PadEntry {
    /// Repository-relative file.
    pub file: String,
    /// 1-based line.
    pub line: u32,
    /// Pad date.
    pub date: Date,
    /// Padded account.
    pub account: String,
    /// Account the difference comes from.
    pub source_account: String,
}

/// Open and close dates of an account.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AccountState {
    /// Date of the `open` directive.
    pub opened: Option<Date>,
    /// Date of the `close` directive.
    pub closed: Option<Date>,
}

/// A failing balance assertion, as computed by the core.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BalanceFailure {
    /// Repository-relative file of the assertion, if known.
    pub file: Option<String>,
    /// 1-based line, if known.
    pub line: Option<u32>,
    /// Assertion date.
    pub date: Date,
    /// Asserted account.
    pub account: String,
    /// Asserted number.
    pub asserted: Decimal,
    /// `computed - asserted`.
    pub difference: Decimal,
    /// Currency of the assertion.
    pub currency: String,
}

/// An error reported by rustledger, other than a failing balance assertion.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CoreError {
    /// rustledger's code, such as `E1001`, or `LOAD`/`PARSE`.
    pub code: String,
    /// rustledger's English message, kept as detail only.
    pub message: String,
    /// Repository-relative file, if known.
    pub file: Option<String>,
    /// 1-based line, if known.
    pub line: Option<u32>,
    /// Whether rustledger rates it as an error rather than a warning.
    pub is_error: bool,
}

/// A raw construct found while parsing, for the profile check (spec section 14).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Construct {
    /// Repository-relative file.
    pub file: String,
    /// 1-based line.
    pub line: u32,
    /// What was found.
    pub kind: ConstructKind,
}

/// The kinds of constructs the profile check looks at.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConstructKind {
    /// `option "<name>" "<value>"`.
    Option {
        /// Option name.
        name: String,
        /// Option value.
        value: String,
    },
    /// `include "<pattern>"`.
    Include(String),
    /// `plugin "<name>"`.
    Plugin(String),
    /// A line starting with `pushtag` or `poptag`.
    TagStack,
}

/// The indexed ledger.
#[derive(Clone, Debug, Default)]
pub struct Ledger {
    /// Files reached from the main file, in load order.
    pub files: Vec<String>,
    /// All active transactions, in file order.
    pub transactions: Vec<Txn>,
    /// `lentil_id`s found in comment lines: dismissed transactions (spec section 10).
    pub dismissed_ids: BTreeSet<String>,
    /// Accounts with their open and close dates.
    pub accounts: BTreeMap<String, AccountState>,
    /// All balance assertions.
    pub balance_assertions: Vec<BalanceAssertion>,
    /// All pads.
    pub pads: Vec<PadEntry>,
    /// Options, includes, plugins and tag stacks for the profile check.
    pub constructs: Vec<Construct>,
    /// Errors from loading, parsing, booking and validation.
    pub errors: Vec<CoreError>,
    /// Failing balance assertions.
    pub balance_failures: Vec<BalanceFailure>,
}

impl Ledger {
    /// The active transaction with the given `lentil_id`.
    #[must_use]
    pub fn find(&self, id: &str) -> Option<&Txn> {
        self.transactions
            .iter()
            .find(|t| t.id.as_deref() == Some(id))
    }

    /// Every `lentil_id` in the ledger, active or dismissed (spec section 8).
    #[must_use]
    pub fn all_ids(&self) -> BTreeSet<&str> {
        self.transactions
            .iter()
            .filter_map(|t| t.id.as_deref())
            .chain(self.dismissed_ids.iter().map(String::as_str))
            .collect()
    }

    /// Whether `account` is open on `date`.
    #[must_use]
    pub fn is_open_on(&self, account: &str, date: Date) -> bool {
        self.accounts.get(account).is_some_and(|state| {
            state.opened.is_some_and(|o| o <= date) && state.closed.is_none_or(|c| date < c)
        })
    }

    /// The review queue: active transactions with flag `!`, newest first
    /// (spec section 6, PRD FR-D1). Ties keep reverse file order.
    #[must_use]
    pub fn open_queue(&self) -> Vec<&Txn> {
        let mut queue: Vec<&Txn> = self.transactions.iter().filter(|t| t.flag == '!').collect();
        queue.reverse();
        queue.sort_by_key(|t| std::cmp::Reverse(t.date));
        queue
    }
}

/// Loads the ledger whose main file is `main_file` from `snapshot`.
///
/// Never fails: problems end up in [`Ledger::errors`].
#[must_use]
pub fn load(snapshot: &Snapshot, main_file: &str) -> Ledger {
    let fs = SnapshotFs::new(snapshot);
    let options = LoadOptions {
        run_plugins: false,
        path_security: true,
        ..LoadOptions::default()
    };
    let main_path = PathBuf::from(format!("{ROOT}/{main_file}"));
    let mut ledger = Ledger::default();

    let loaded = match rustledger_loader::load_with_fs(&main_path, &options, Box::new(fs)) {
        Ok(loaded) => loaded,
        Err(err) => {
            ledger.errors.push(CoreError {
                code: "LOAD".to_owned(),
                message: err.to_string(),
                file: Some(main_file.to_owned()),
                line: None,
                is_error: true,
            });
            return ledger;
        }
    };

    for file in loaded.source_map.files() {
        if let Some(rel) = relative(&file.path) {
            ledger.files.push(rel);
        }
    }

    for err in &loaded.errors {
        if err.code == "E2001" || err.code == "E2002" {
            continue;
        }
        let (file, line) = match &err.location {
            Some(loc) => (relative(&loc.file), u32::try_from(loc.line).ok()),
            None => (None, None),
        };
        ledger.errors.push(CoreError {
            code: err.code.clone(),
            message: err.message.clone(),
            file,
            line,
            is_error: err.severity == ErrorSeverity::Error,
        });
    }

    for file in &ledger.files.clone() {
        if let Some(text) = snapshot.text(file) {
            index_file(&mut ledger, file, text);
        }
    }

    for d in &loaded.balance_discrepancies {
        let date = d.date;
        let account = d.account.to_string();
        let asserted = d.asserted;
        let location = ledger
            .balance_assertions
            .iter()
            .find(|b| b.date == date && b.account == account && b.amount == asserted);
        ledger.balance_failures.push(BalanceFailure {
            file: location.map(|b| b.file.clone()),
            line: location.map(|b| b.line),
            date,
            account,
            asserted,
            difference: d.amount.number,
            currency: d.amount.currency.to_string(),
        });
    }
    ledger
}

static DISMISSED_ID: LazyLock<Regex> = LazyLock::new(|| {
    #[allow(clippy::unwrap_used)] // A constant pattern.
    Regex::new(r#"^;\s*lentil_id:\s*"([^"]+)""#).unwrap()
});

fn index_file(ledger: &mut Ledger, file: &str, text: &str) {
    let parsed = rustledger_parser::parse_without_occurrences(text);
    let lines = LineIndex::new(text);

    for (name, value, span) in &parsed.options {
        ledger.constructs.push(Construct {
            file: file.to_owned(),
            line: lines.line_of(span.start),
            kind: ConstructKind::Option {
                name: name.clone(),
                value: value.clone(),
            },
        });
    }
    for (pattern, span) in &parsed.includes {
        ledger.constructs.push(Construct {
            file: file.to_owned(),
            line: lines.line_of(span.start),
            kind: ConstructKind::Include(pattern.clone()),
        });
    }
    for (name, _, span) in &parsed.plugins {
        ledger.constructs.push(Construct {
            file: file.to_owned(),
            line: lines.line_of(span.start),
            kind: ConstructKind::Plugin(name.clone()),
        });
    }
    let mut offset = 0;
    for line in text.split_inclusive('\n') {
        if line.starts_with("pushtag") || line.starts_with("poptag") {
            ledger.constructs.push(Construct {
                file: file.to_owned(),
                line: lines.line_of(offset),
                kind: ConstructKind::TagStack,
            });
        }
        offset += line.len();
    }

    for comment in &parsed.comments {
        if let Some(caps) = DISMISSED_ID.captures(&comment.value) {
            ledger.dismissed_ids.insert(caps[1].to_owned());
        }
    }

    for spanned in &parsed.directives {
        let start = spanned.span.start;
        match &spanned.value {
            Directive::Transaction(t) => {
                let span = start..transaction_end(text, start, spanned.span.end);
                ledger.transactions.push(Txn {
                    id: meta_string(&t.meta, META_ID),
                    file: file.to_owned(),
                    line: lines.line_of(start),
                    flag_offset: flag_offset(text, start),
                    span,
                    date: t.date,
                    flag: t.flag,
                    payee: t.payee.as_ref().map(ToString::to_string),
                    narration: t.narration.to_string(),
                    desc: meta_string(&t.meta, META_DESC),
                    reference: meta_string(&t.meta, META_REF),
                    tags: t.tags.iter().map(ToString::to_string).collect(),
                    links: t.links.iter().map(ToString::to_string).collect(),
                    postings: t
                        .postings
                        .iter()
                        .map(|p| posting(text, &p.value, p.span.start..p.span.end))
                        .collect(),
                });
            }
            Directive::Balance(b) => ledger.balance_assertions.push(BalanceAssertion {
                file: file.to_owned(),
                line: lines.line_of(start),
                date: b.date,
                account: b.account.to_string(),
                amount: b.amount.number,
                currency: b.amount.currency.to_string(),
                tolerance: b.tolerance,
            }),
            Directive::Pad(p) => ledger.pads.push(PadEntry {
                file: file.to_owned(),
                line: lines.line_of(start),
                date: p.date,
                account: p.account.to_string(),
                source_account: p.source_account.to_string(),
            }),
            Directive::Open(o) => {
                let state = ledger.accounts.entry(o.account.to_string()).or_default();
                state.opened = Some(state.opened.map_or(o.date, |d| d.min(o.date)));
            }
            Directive::Close(c) => {
                let state = ledger.accounts.entry(c.account.to_string()).or_default();
                state.closed = Some(c.date);
            }
            _ => {}
        }
    }
}

fn posting(text: &str, p: &rustledger_core::Posting, span: Range<usize>) -> Posting {
    let account = p.account.to_string();
    let account_span = account_span(text, &span, &account);
    let units = match &p.units {
        Some(IncompleteAmount::Complete(a)) => Some((a.number, a.currency.to_string())),
        _ => None,
    };
    Posting {
        account,
        account_span,
        span,
        units,
        has_units: p.units.is_some(),
        has_cost: p.cost.is_some(),
        has_price: p.price.is_some(),
        flag: p.flag,
    }
}

/// Finds the account name inside a posting line.
fn account_span(text: &str, span: &Range<usize>, account: &str) -> Range<usize> {
    let line = &text[span.clone()];
    match line.find(account) {
        Some(pos) => span.start + pos..span.start + pos + account.len(),
        None => span.start..span.start,
    }
}

/// The transaction's own lines: the header, then every following line that is
/// indented and not blank. rustledger's span also covers blank lines and
/// comments up to the next directive, which belong to the file, not the
/// transaction.
fn transaction_end(text: &str, start: usize, parsed_end: usize) -> usize {
    let mut end = start;
    let mut offset = start;
    for (i, line) in text[start..parsed_end.min(text.len())]
        .split_inclusive('\n')
        .enumerate()
    {
        let content = line.trim_end_matches(['\n', '\r']);
        let belongs = i == 0 || (content.starts_with([' ', '\t']) && !content.trim().is_empty());
        if !belongs {
            break;
        }
        end = offset + content.len();
        offset += line.len();
    }
    end
}

/// Byte offset of the flag in a transaction header such as `2026-10-18 ! "..."`.
fn flag_offset(text: &str, start: usize) -> Option<usize> {
    let header = &text[start..];
    let after_date = header.get(10..)?;
    let skipped = after_date.len() - after_date.trim_start_matches([' ', '\t']).len();
    let pos = start + 10 + skipped;
    let rest = &text[pos..];
    if rest.starts_with("txn") {
        return None;
    }
    let flag = rest.chars().next()?;
    (!flag.is_whitespace() && flag != '"').then_some(pos)
}

fn meta_string(meta: &Metadata, key: &str) -> Option<String> {
    match meta.get(key) {
        Some(MetaValue::String(s)) => Some(s.clone()),
        _ => None,
    }
}

/// Maps a loader path under [`ROOT`] back to a repository-relative path.
fn relative(path: &Path) -> Option<String> {
    path.strip_prefix(ROOT)
        .ok()
        .map(|p| p.to_string_lossy().replace('\\', "/"))
}

/// Byte offset to 1-based line numbers.
struct LineIndex {
    starts: Vec<usize>,
}

impl LineIndex {
    fn new(text: &str) -> Self {
        let mut starts = vec![0];
        starts.extend(text.match_indices('\n').map(|(i, _)| i + 1));
        Self { starts }
    }

    fn line_of(&self, offset: usize) -> u32 {
        let line = self.starts.partition_point(|&s| s <= offset);
        u32::try_from(line).unwrap_or(u32::MAX)
    }
}

/// Hands snapshot files to rustledger's loader under the virtual root [`ROOT`].
#[derive(Debug)]
struct SnapshotFs {
    files: Arc<BTreeMap<String, Arc<str>>>,
}

impl SnapshotFs {
    fn new(snapshot: &Snapshot) -> Self {
        let files = snapshot
            .files()
            .map(|(p, c)| (p.to_owned(), Arc::from(String::from_utf8_lossy(c).as_ref())))
            .collect();
        Self {
            files: Arc::new(files),
        }
    }

    fn key(path: &Path) -> Option<String> {
        relative(&clean(path))
    }
}

impl FileSystem for SnapshotFs {
    fn read(&self, path: &Path) -> Result<Arc<str>, LoadError> {
        Self::key(path)
            .and_then(|k| self.files.get(&k).cloned())
            .ok_or_else(|| LoadError::Io {
                path: path.to_path_buf(),
                source: std::io::Error::from(std::io::ErrorKind::NotFound),
            })
    }

    fn exists(&self, path: &Path) -> bool {
        Self::key(path).is_some_and(|k| self.files.contains_key(&k))
    }

    fn dir_exists(&self, path: &Path) -> bool {
        Self::key(path).is_some_and(|k| {
            let prefix = format!("{k}/");
            k.is_empty() || self.files.keys().any(|f| f.starts_with(&prefix))
        })
    }

    fn is_encrypted(&self, _path: &Path) -> bool {
        false
    }

    fn normalize(&self, path: &Path) -> PathBuf {
        clean(path)
    }

    fn glob(&self, pattern: &str) -> Result<Vec<PathBuf>, String> {
        let Some(rel) = relative(&clean(Path::new(pattern))) else {
            return Ok(Vec::new());
        };
        Ok(self
            .files
            .keys()
            .filter(|f| glob_match(&rel, f))
            .map(|f| PathBuf::from(format!("{ROOT}/{f}")))
            .collect())
    }

    fn decrypt(&self, path: &Path) -> Result<Arc<str>, LoadError> {
        Err(LoadError::Decryption {
            path: path.to_path_buf(),
            message: "encrypted files are outside the profile".to_owned(),
        })
    }
}

/// Resolves `.` and `..` lexically.
fn clean(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) const MAIN: &str = r#"option "title" "Ledger"
option "operating_currency" "EUR"

include "accounts.beancount"
include "imports/*/*/*.beancount"
"#;

    const ACCOUNTS: &str = "2020-01-01 open Assets:Bank:Giro EUR
2020-01-01 open Expenses:Uncategorized
2020-01-01 open Expenses:Food:Groceries
";

    const RUN: &str = r#"; lentil import run
; source: giro, started: 2026-10-20T07:15:02Z, client: lentil-cli/0.1.0

2026-10-18 ! "Corner Grocery" "Card payment"
  lentil_id: "giro-3f9a1c2e7b4d"
  lentil_desc: "CORNER GROCERY 4711 CARD PAYMENT"
  Assets:Bank:Giro        -42.17 EUR
  Expenses:Food:Groceries

; 2026-10-19 ! "City Utilities" "Electricity"
;   lentil_id: "giro-8c01d4e9a2f7"
;   Assets:Bank:Giro        -64.00 EUR
;   Expenses:Uncategorized

2026-10-19 txn "Bakery" "Bread" #food
  lentil_id: "giro-aaaaaaaaaaaa"
  Assets:Bank:Giro        -3.10 EUR ; trailing
    posting_meta: "x"
  ! Expenses:Uncategorized   3.10 EUR
  ; trailing comment in txn

2026-10-20 balance Assets:Bank:Giro  -45.27 ~ 0.00 EUR
2026-10-20 balance Assets:Bank:Giro  -45.28 ~ 0.00 EUR
"#;

    fn snapshot() -> Snapshot {
        Snapshot::from_files([
            ("main.beancount", MAIN),
            ("accounts.beancount", ACCOUNTS),
            ("imports/giro/2026/20261020T071502Z.beancount", RUN),
        ])
    }

    const RUN_FILE: &str = "imports/giro/2026/20261020T071502Z.beancount";

    #[test]
    fn loads_files_through_globs() {
        let ledger = load(&snapshot(), "main.beancount");
        assert_eq!(
            ledger.files,
            vec!["main.beancount", "accounts.beancount", RUN_FILE]
        );
        assert!(ledger.errors.is_empty(), "{:?}", ledger.errors);
    }

    #[test]
    fn indexes_transactions_with_exact_spans() {
        let ledger = load(&snapshot(), "main.beancount");
        assert_eq!(ledger.transactions.len(), 2);
        let first = ledger.find("giro-3f9a1c2e7b4d").unwrap();
        assert_eq!(first.flag, '!');
        assert_eq!(first.line, 4);
        assert_eq!(
            first.desc.as_deref(),
            Some("CORNER GROCERY 4711 CARD PAYMENT")
        );
        assert!(RUN[first.span.clone()].starts_with("2026-10-18 !"));
        assert!(RUN[first.span.clone()].ends_with("Expenses:Food:Groceries"));
        assert_eq!(
            &RUN[first.flag_offset.unwrap()..=first.flag_offset.unwrap()],
            "!"
        );
        assert_eq!(
            &RUN[first.postings[1].account_span.clone()],
            "Expenses:Food:Groceries"
        );
        assert_eq!(first.postings[1].units, None);
        assert_eq!(
            first.postings[0].units,
            Some((Decimal::new(-4217, 2), "EUR".to_owned()))
        );

        let second = ledger.find("giro-aaaaaaaaaaaa").unwrap();
        assert_eq!(second.flag, '*');
        assert_eq!(second.flag_offset, None);
        assert!(RUN[second.span.clone()].ends_with("; trailing comment in txn"));
        assert_eq!(second.postings[1].flag, Some('!'));
        assert_eq!(
            &RUN[second.postings[1].account_span.clone()],
            "Expenses:Uncategorized"
        );
    }

    #[test]
    fn collects_dismissed_ids_from_comments() {
        let ledger = load(&snapshot(), "main.beancount");
        assert!(ledger.dismissed_ids.contains("giro-8c01d4e9a2f7"));
        assert!(ledger.find("giro-8c01d4e9a2f7").is_none());
        assert!(ledger.all_ids().contains("giro-8c01d4e9a2f7"));
    }

    #[test]
    fn reports_balance_failures_with_location() {
        let ledger = load(&snapshot(), "main.beancount");
        assert_eq!(ledger.balance_assertions.len(), 2);
        assert_eq!(ledger.balance_failures.len(), 1);
        let failure = &ledger.balance_failures[0];
        assert_eq!(failure.asserted, Decimal::new(-4528, 2));
        assert_eq!(failure.difference, Decimal::new(1, 2));
        assert_eq!(failure.file.as_deref(), Some(RUN_FILE));
        assert_eq!(failure.line, Some(23));
    }

    #[test]
    fn open_queue_is_newest_first() {
        let mut s = snapshot();
        let extra = "2026-10-21 ! \"Later\" \"x\"\n  lentil_id: \"giro-bbbbbbbbbbbb\"\n  lentil_desc: \"LATER\"\n  Assets:Bank:Giro  -1.00 EUR\n  Expenses:Uncategorized\n";
        s.insert("imports/giro/2026/20261022T000000Z.beancount", extra);
        let ledger = load(&s, "main.beancount");
        let ids: Vec<_> = ledger
            .open_queue()
            .iter()
            .map(|t| t.id.clone().unwrap())
            .collect();
        assert_eq!(ids, vec!["giro-bbbbbbbbbbbb", "giro-3f9a1c2e7b4d"]);
    }

    #[test]
    fn empty_glob_and_unknown_account_are_errors() {
        let mut s = snapshot();
        s.remove(RUN_FILE);
        s.insert(
            "accounts.beancount",
            "2020-01-01 open Assets:Bank:Giro EUR\n2026-01-01 * \"x\"\n  Assets:Bank:Giro 1 EUR\n  Expenses:Nope\n",
        );
        let ledger = load(&s, "main.beancount");
        let codes: Vec<_> = ledger.errors.iter().map(|e| e.code.as_str()).collect();
        assert!(codes.contains(&"LOAD"), "{codes:?}");
        assert!(codes.contains(&"E1001"), "{codes:?}");
        let e1001 = ledger.errors.iter().find(|e| e.code == "E1001").unwrap();
        assert_eq!(e1001.file.as_deref(), Some("accounts.beancount"));
    }

    #[test]
    fn include_outside_root_is_an_error() {
        let mut s = snapshot();
        s.insert(
            "main.beancount",
            format!("{MAIN}include \"../outside.beancount\"\n"),
        );
        let ledger = load(&s, "main.beancount");
        assert!(
            ledger
                .errors
                .iter()
                .any(|e| e.message.contains("traversal")),
            "{:?}",
            ledger.errors
        );
    }

    #[test]
    fn is_open_on_respects_dates() {
        let ledger = load(&snapshot(), "main.beancount");
        let d = |s: &str| s.parse::<Date>().unwrap();
        assert!(ledger.is_open_on("Assets:Bank:Giro", d("2020-01-01")));
        assert!(!ledger.is_open_on("Assets:Bank:Giro", d("2019-12-31")));
        assert!(!ledger.is_open_on("Assets:Nope", d("2026-01-01")));
    }
}
