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

use crate::id;
use crate::snapshot::{Snapshot, glob_match};
use crate::tags::is_tag_name;
use crate::trips::TRIP_EVENT;

/// Metadata key of the stable transaction ID (spec section 5).
pub const META_ID: &str = "lentil_id";
/// Metadata key of the bank's reference.
pub const META_REF: &str = "lentil_ref";
/// Metadata key of the bank's counterparty, as imported.
pub const META_COUNTERPARTY: &str = "lentil_counterparty";
/// Metadata key of the bank's purpose, as imported.
pub const META_PURPOSE: &str = "lentil_purpose";
/// Metadata key of the bank's booking type, as imported.
pub const META_TYPE: &str = "lentil_type";
/// Keys no client may change or remove.
pub const PROTECTED_META: [&str; 5] = [
    META_ID,
    META_REF,
    META_COUNTERPARTY,
    META_PURPOSE,
    META_TYPE,
];

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
    /// Bytes of the payee string in the header, quotes included, if the
    /// transaction has a payee and its header strings were found.
    pub payee_span: Option<Range<usize>>,
    /// Bytes of the narration string in the header, quotes included, or
    /// `None` if the header has no string or its strings were not found.
    pub narration_span: Option<Range<usize>>,
    /// `lentil_counterparty`, the bank's counterparty.
    pub counterparty: Option<String>,
    /// `lentil_purpose`, the bank's purpose.
    pub purpose: Option<String>,
    /// `lentil_type`, the bank's booking type.
    pub booking_type: Option<String>,
    /// `lentil_ref`, the bank's reference.
    pub reference: Option<String>,
    /// Tags without `#`.
    pub tags: Vec<String>,
    /// Where the tags stand, or `None` if the header strings were not found,
    /// a scanned line holds a token other than a tag, a link or a comment, or
    /// the tags written do not read as the parsed ones (for example a tag
    /// from `pushtag`).
    pub tag_spans: Option<TagSpans>,
    /// Links without `^`.
    pub links: Vec<String>,
    /// Postings in file order.
    pub postings: Vec<Posting>,
}

/// Where the tags of a transaction stand (spec section 10, `add-tag` and
/// `remove-tag`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TagSpans {
    /// Offset in the header line after its last string, tag or link, where
    /// ` #<tag>` goes: before trailing blanks and a comment.
    pub insert_at: usize,
    /// Each tag as written, in file order: its name without `#` and the bytes
    /// of `#<name>`.
    pub tags: Vec<(String, Range<usize>)>,
}

impl Txn {
    /// The original text of `lentil_counterparty` and `lentil_purpose`
    /// ([`id::original_text`]), or `None` if both are missing or blank.
    #[must_use]
    pub fn original_text(&self) -> Option<String> {
        Some(id::original_text(
            self.counterparty.as_deref(),
            self.purpose.as_deref(),
        ))
        .filter(|t| !t.is_empty())
    }
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

/// An `event "trip"` entry (spec section 9), indexed whatever its value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TripEvent {
    /// `lentil_id`, if the entry has one.
    pub id: Option<String>,
    /// Repository-relative file.
    pub file: String,
    /// Byte offset of the entry in `file`, its position for ordering.
    pub offset: usize,
    /// 1-based line of the entry.
    pub line: u32,
    /// The event's date.
    pub date: Date,
    /// The event's value: a trip name, or empty for the end of a trip.
    pub name: String,
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
    /// Every `event "trip"`, whatever its value, in file order.
    pub trip_events: Vec<TripEvent>,
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

    // The loader reads such files lossily; Lentil must not index them
    // partially, or deduplication would miss their IDs.
    for file in &ledger.files {
        if snapshot.contains(file) && snapshot.text(file).is_none() {
            ledger.errors.push(CoreError {
                code: "LOAD".to_owned(),
                message: "file is not valid UTF-8".to_owned(),
                file: Some(file.clone()),
                line: None,
                is_error: true,
            });
        }
    }
    for part in index_files(snapshot, &ledger.files) {
        ledger.merge(part);
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

/// Indexes `files` in parallel, one contiguous chunk per thread, and returns
/// the per-file results in file order.
fn index_files(snapshot: &Snapshot, files: &[String]) -> Vec<Ledger> {
    let threads = std::thread::available_parallelism().map_or(1, std::num::NonZero::get);
    let chunk = files.len().div_ceil(threads).max(1);
    std::thread::scope(|scope| {
        let workers: Vec<_> = files
            .chunks(chunk)
            .map(|chunk| {
                scope.spawn(move || {
                    chunk
                        .iter()
                        .filter_map(|file| {
                            let text = snapshot.text(file)?;
                            let mut part = Ledger::default();
                            index_file(&mut part, file, text);
                            Some(part)
                        })
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        workers
            .into_iter()
            // A worker only panics if indexing panics, which is a bug; surface it.
            .flat_map(|w| w.join().unwrap_or_else(|e| std::panic::resume_unwind(e)))
            .collect()
    })
}

impl Ledger {
    /// Appends the index of the next file in load order.
    fn merge(&mut self, part: Ledger) {
        self.transactions.extend(part.transactions);
        self.dismissed_ids.extend(part.dismissed_ids);
        for (account, state) in part.accounts {
            let entry = self.accounts.entry(account).or_default();
            if let Some(opened) = state.opened {
                entry.opened = Some(entry.opened.map_or(opened, |d| d.min(opened)));
            }
            if state.closed.is_some() {
                entry.closed = state.closed;
            }
        }
        self.balance_assertions.extend(part.balance_assertions);
        self.pads.extend(part.pads);
        self.trip_events.extend(part.trip_events);
        self.constructs.extend(part.constructs);
    }
}

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
                let (payee_span, narration_span) =
                    match header_strings(text, start, t.payee.is_some()) {
                        Some((payee, narration)) => (payee, Some(narration)),
                        None => (None, None),
                    };
                let tags: Vec<String> = t.tags.iter().map(ToString::to_string).collect();
                let tag_spans = narration_span
                    .as_ref()
                    .and_then(|narration| tag_spans(text, &span, narration, &tags));
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
                    payee_span,
                    narration_span,
                    counterparty: meta_string(&t.meta, META_COUNTERPARTY),
                    purpose: meta_string(&t.meta, META_PURPOSE),
                    booking_type: meta_string(&t.meta, META_TYPE),
                    reference: meta_string(&t.meta, META_REF),
                    tags,
                    tag_spans,
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
            Directive::Event(e) if e.event_type == TRIP_EVENT => {
                ledger.trip_events.push(TripEvent {
                    id: meta_string(&e.meta, META_ID),
                    file: file.to_owned(),
                    offset: start,
                    line: lines.line_of(start),
                    date: e.date,
                    name: e.value.clone(),
                });
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

/// The payee and narration strings of the header at `start`, quotes
/// included: the strings after the date and the flag or `txn`, up to the
/// first other token. `None` unless there is one string per parsed field, two
/// with a payee and one without. Replay checks that the strings read as the
/// parsed values before it edits them (spec section 10).
fn header_strings(
    text: &str,
    start: usize,
    has_payee: bool,
) -> Option<(Option<Range<usize>>, Range<usize>)> {
    let mut pos = start;
    // The date, then the flag or `txn`, each up to a blank or a string.
    for _ in 0..2 {
        pos += text[pos..].find([' ', '\t', '"', '\r', '\n'])?;
        pos += blanks(&text[pos..]);
    }
    let mut strings = Vec::new();
    while text[pos..].starts_with('"') {
        let end = pos + string_len(&text[pos..])?;
        strings.push(pos..end);
        pos = end + blanks(&text[end..]);
    }
    match (has_payee, strings.as_slice()) {
        (false, [narration]) => Some((None, narration.clone())),
        (true, [payee, narration]) => Some((Some(payee.clone()), narration.clone())),
        _ => None,
    }
}

/// Where the tags of the transaction at `span` stand: on the header line
/// after the narration, and on later lines of the transaction whose first
/// token is a tag or a link, outside a multi-line string. `None` if the
/// narration ends outside the span (a multi-line string continued at the
/// line start), if such a line holds a token other than a tag, a link or a
/// comment, or if the names written differ from `parsed` as a set.
fn tag_spans(
    text: &str,
    span: &Range<usize>,
    narration: &Range<usize>,
    parsed: &[String],
) -> Option<TagSpans> {
    let text = text.get(..span.end)?;
    if narration.end > text.len() {
        return None;
    }
    let mut tags = Vec::new();
    let header_end = line_end(text, narration.end);
    let insert_at =
        scan_tag_line(text, narration.end..header_end, &mut tags)?.unwrap_or(narration.end);
    let mut in_string = false;
    let mut pos = next_line(text, header_end);
    while pos < text.len() {
        let end = line_end(text, pos);
        let line = &text[pos..end];
        if !in_string && starts_with_tag_or_link(line) {
            scan_tag_line(text, pos..end, &mut tags)?;
        } else {
            in_string = string_open_after(line, in_string);
        }
        pos = next_line(text, end);
    }
    let written: BTreeSet<&str> = tags.iter().map(|(name, _)| name.as_str()).collect();
    let parsed: BTreeSet<&str> = parsed.iter().map(String::as_str).collect();
    (written == parsed).then_some(TagSpans { insert_at, tags })
}

/// The token at the start of `text`: up to a blank or a comment.
fn first_token(text: &str) -> &str {
    &text[..text.find([' ', '\t', ';']).unwrap_or(text.len())]
}

/// Whether the first token of `line` is a tag or a link, as on the tag and
/// link lines Beancount v3 accepts after a transaction header. A posting
/// with the flag `#` starts with `#` and a blank, so it is none.
fn starts_with_tag_or_link(line: &str) -> bool {
    let token = first_token(line.trim_start_matches([' ', '\t']));
    token.strip_prefix(['#', '^']).is_some_and(is_tag_name)
}

/// Whether a string literal is still open at the end of `line`, given
/// whether one was open at its start. Outside a string, `;` starts a
/// comment; inside one, a backslash escapes the next character.
fn string_open_after(line: &str, mut open: bool) -> bool {
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        match (open, c) {
            (true, '\\') => {
                chars.next();
            }
            (true, '"') => open = false,
            (false, '"') => open = true,
            (false, ';') => break,
            _ => {}
        }
    }
    open
}

/// Scans `line` for tags, links and a trailing comment, pushing each tag onto
/// `tags`. Returns the end of the last tag or link, `Some(None)` if there is
/// none, and `None` if the line holds any other token.
fn scan_tag_line(
    text: &str,
    line: Range<usize>,
    tags: &mut Vec<(String, Range<usize>)>,
) -> Option<Option<usize>> {
    let mut last = None;
    let mut pos = line.start;
    loop {
        pos += blanks(&text[pos..line.end]);
        let rest = &text[pos..line.end];
        if rest.is_empty() || rest.starts_with(';') {
            return Some(last);
        }
        let token = first_token(rest);
        let len = token.len();
        if let Some(tag) = token.strip_prefix('#').filter(|name| is_tag_name(name)) {
            tags.push((tag.to_owned(), pos..pos + len));
        } else if !token.strip_prefix('^').is_some_and(is_tag_name) {
            return None;
        }
        pos += len;
        last = Some(pos);
    }
}

/// The end of the line at `pos`: the next `\r` or `\n`, or the end of `text`.
fn line_end(text: &str, pos: usize) -> usize {
    text[pos..]
        .find(['\r', '\n'])
        .map_or(text.len(), |i| pos + i)
}

/// The start of the line after the line ending at `end`.
fn next_line(text: &str, end: usize) -> usize {
    let rest = &text[end..];
    end + if rest.starts_with("\r\n") {
        2
    } else {
        usize::from(!rest.is_empty())
    }
}

/// Byte length of the spaces and tabs at the start of `text`.
fn blanks(text: &str) -> usize {
    text.len() - text.trim_start_matches([' ', '\t']).len()
}

/// Byte length of the string literal at the start of `text`, quotes
/// included, or `None` if it is not closed. A backslash escapes the next
/// character, as in rustledger's lexer.
fn string_len(text: &str) -> Option<usize> {
    let mut chars = text.char_indices().skip(1);
    while let Some((i, c)) = chars.next() {
        match c {
            '"' => return Some(i + 1),
            '\\' => {
                chars.next();
            }
            _ => {}
        }
    }
    None
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

    fn supports_parallel_read(&self) -> bool {
        // An immutable map: safe to read from several threads.
        true
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
  lentil_counterparty: "CORNER GROCERY 4711"
  lentil_purpose: "CARD PAYMENT"
  lentil_type: "DEBIT CARD"
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
            (
                first.counterparty.as_deref(),
                first.purpose.as_deref(),
                first.booking_type.as_deref()
            ),
            (
                Some("CORNER GROCERY 4711"),
                Some("CARD PAYMENT"),
                Some("DEBIT CARD")
            )
        );
        assert_eq!(
            first.original_text().as_deref(),
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

        assert_eq!(
            &RUN[first.payee_span.clone().unwrap()],
            "\"Corner Grocery\""
        );
        assert_eq!(
            &RUN[first.narration_span.clone().unwrap()],
            "\"Card payment\""
        );

        let second = ledger.find("giro-aaaaaaaaaaaa").unwrap();
        assert_eq!(second.flag, '*');
        assert_eq!(second.flag_offset, None);
        assert_eq!(&RUN[second.payee_span.clone().unwrap()], "\"Bakery\"");
        assert_eq!(&RUN[second.narration_span.clone().unwrap()], "\"Bread\"");
        assert!(RUN[second.span.clone()].ends_with("; trailing comment in txn"));
        assert_eq!(second.postings[1].flag, Some('!'));
        assert_eq!(
            &RUN[second.postings[1].account_span.clone()],
            "Expenses:Uncategorized"
        );
    }

    /// The text at the payee and narration spans of a transaction with
    /// `header`.
    fn header_spans(header: &str) -> (Option<String>, Option<String>) {
        let text =
            format!("; x\n{header}\n  Assets:Bank:Giro  -1.00 EUR\n  Expenses:Uncategorized\n");
        let mut ledger = Ledger::default();
        index_file(&mut ledger, "f", &text);
        let txn = &ledger.transactions[0];
        let at = |span: &Option<Range<usize>>| span.clone().map(|s| text[s].to_owned());
        (at(&txn.payee_span), at(&txn.narration_span))
    }

    #[test]
    fn finds_the_header_strings() {
        let found = |payee: Option<&str>, narration: &str| {
            (payee.map(ToOwned::to_owned), Some(narration.to_owned()))
        };
        assert_eq!(
            header_spans(r#"2026-10-18 ! "Shop" "Food""#),
            found(Some(r#""Shop""#), r#""Food""#)
        );
        assert_eq!(
            header_spans(r#"2026-10-18 ! "Food" #tag ^link ; note"#),
            found(None, r#""Food""#)
        );
        assert_eq!(
            header_spans("2026-10-18 \t txn\t  \"Shop\"  \t\"Food\"\t"),
            found(Some(r#""Shop""#), r#""Food""#)
        );
        assert_eq!(
            header_spans(r#"2026-10-18 *"Shop""Food""#),
            found(Some(r#""Shop""#), r#""Food""#)
        );
        assert_eq!(
            header_spans(r#"2026-10-18 ! "Say \"hi\" \\" "Café ü""#),
            found(Some(r#""Say \"hi\" \\""#), r#""Café ü""#)
        );
        assert_eq!(
            header_spans(r#"2026-10-18 ! "" """#),
            found(Some(r#""""#), r#""""#)
        );
    }

    #[test]
    fn header_strings_that_do_not_match_the_fields_are_not_found() {
        for header in [
            "2026-10-18 *",
            "2026-10-18 * ; note",
            r#"2026-10-18 * "Shop" | "Food""#,
            r#"2026-10-18 * "Shop" "Food" "More""#,
            r#"2026-10-18 * #tag "Food""#,
        ] {
            assert_eq!(header_spans(header), (None, None), "{header}");
        }
        assert_eq!(header_strings("2026-10-18 ! \"open", 0, false), None);
        assert_eq!(header_strings("2026-10-18", 0, false), None);
    }

    const TAGGED: &str = "2026-01-02 ! \"P\" \"N\" #one ^l1 ; note\n  #two ^l2\n  #three\n  lentil_id: \"x\"\n  Assets:Bank:Giro  -1.00 EUR\n  Expenses:Uncategorized\n";

    /// The one transaction of a run file with `text`, loaded without errors.
    fn tagged_txn(text: &str) -> Txn {
        let snapshot = Snapshot::from_files([
            ("main.beancount", MAIN),
            ("accounts.beancount", ACCOUNTS),
            (RUN_FILE, text),
        ]);
        let ledger = load(&snapshot, "main.beancount");
        assert!(ledger.errors.is_empty(), "{:?}", ledger.errors);
        ledger.transactions.into_iter().next().unwrap()
    }

    fn assert_tag_spans(text: &str) {
        let spans = tagged_txn(text).tag_spans.unwrap();
        let names: Vec<&str> = spans.tags.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["one", "two", "three"]);
        for (name, range) in &spans.tags {
            assert_eq!(text[range.clone()], format!("#{name}"));
        }
        assert!(text[..spans.insert_at].ends_with("^l1"), "{text:?}");
        assert!(text[spans.insert_at..].starts_with(" ; note"), "{text:?}");
    }

    #[test]
    fn indexes_where_tags_stand() {
        assert_eq!(tagged_txn(TAGGED).tags, ["one", "two", "three"]);
        assert_tag_spans(TAGGED);
    }

    #[test]
    fn inserts_after_the_narration_without_tags() {
        let text =
            "2026-01-02 ! \"N\"   \n  Assets:Bank:Giro  -1.00 EUR\n  Expenses:Uncategorized\n";
        let spans = tagged_txn(text).tag_spans.unwrap();
        assert_eq!(spans.tags, Vec::new());
        assert_eq!(&text[..spans.insert_at], "2026-01-02 ! \"N\"");
    }

    #[test]
    fn tag_spans_in_a_crlf_file() {
        assert_tag_spans(&TAGGED.replace('\n', "\r\n"));
    }

    #[test]
    fn tags_from_pushtag_have_no_spans() {
        let text = "pushtag #trip\n\n2026-01-02 ! \"N\"\n  Assets:Bank:Giro  -1.00 EUR\n  Expenses:Uncategorized\n\npoptag #trip\n";
        let txn = tagged_txn(text);
        assert_eq!(txn.tags, ["trip"]);
        assert_eq!(txn.tag_spans, None);
    }

    /// The names and the text at each tag span of the one transaction in a
    /// run file with `text`, which loads without errors.
    fn written_tags(text: &str) -> Vec<(String, String)> {
        let spans = tagged_txn(text).tag_spans.unwrap();
        spans
            .tags
            .iter()
            .map(|(name, range)| (name.clone(), text[range.clone()].to_owned()))
            .collect()
    }

    fn named(tags: &[(&str, &str)]) -> Vec<(String, String)> {
        tags.iter()
            .map(|(name, text)| ((*name).to_owned(), (*text).to_owned()))
            .collect()
    }

    #[test]
    fn a_comment_right_after_a_tag_ends_it() {
        let text = "2026-01-02 ! \"N\" #one;note\n  #two;x\n  Assets:Bank:Giro  -1.00 EUR\n  Expenses:Uncategorized\n";
        let spans = tagged_txn(text).tag_spans.unwrap();
        assert_eq!(
            written_tags(text),
            named(&[("one", "#one"), ("two", "#two")])
        );
        assert!(text[spans.insert_at..].starts_with(";note"), "{text:?}");
    }

    #[test]
    fn a_posting_with_the_flag_hash_is_no_tag_line() {
        let text = "2026-01-02 ! \"N\" #one\n  Assets:Bank:Giro  -1.00 EUR\n  # Expenses:Uncategorized\n  #two\n";
        assert_eq!(
            written_tags(text),
            named(&[("one", "#one"), ("two", "#two")])
        );
    }

    #[test]
    fn a_line_inside_a_metadata_string_is_no_tag_line() {
        let text = "2026-01-02 ! \"N\" #one\n  note: \"first \\\" quote\n  #one\n  last\" ; \"\n  #two\n  Assets:Bank:Giro  -1.00 EUR\n  Expenses:Uncategorized\n";
        assert_eq!(tagged_txn(text).tags, ["one", "two"]);
        assert_eq!(
            written_tags(text),
            named(&[("one", "#one"), ("two", "#two")])
        );
        let spans = tagged_txn(text).tag_spans.unwrap();
        assert!(spans.tags[0].1.end < text.find("note").unwrap());
    }

    /// Indexes `text` as one file, which must not panic, and returns the
    /// `tag_spans` of its transactions.
    fn indexed_tag_spans(text: &str) -> Vec<Option<TagSpans>> {
        let mut ledger = Ledger::default();
        index_file(&mut ledger, "f", text);
        ledger
            .transactions
            .into_iter()
            .map(|t| t.tag_spans)
            .collect()
    }

    #[test]
    fn a_narration_beyond_the_indented_lines_has_no_tag_spans() {
        let text = "2026-10-18 ! \"Corner Grocery\" \"Card payment\nfor the party\"\n  lentil_id: \"giro-3f9a1c2e7b4d\"\n  Assets:Bank:Giro  -42.17 EUR\n  Expenses:Uncategorized\n";
        assert_eq!(indexed_tag_spans(text), vec![None]);
    }

    #[test]
    fn a_transaction_that_ends_at_its_narration_has_tag_spans() {
        let text = "2026-10-18 ! \"Shop\" \"x\"\n";
        assert_eq!(
            indexed_tag_spans(text),
            vec![Some(TagSpans {
                insert_at: text.len() - 1,
                tags: Vec::new(),
            })]
        );
    }

    #[test]
    fn other_tokens_after_the_narration_have_no_tag_spans() {
        for rest in [
            " \u{2713}",
            " #a \u{fc}ber",
            "\n  #urlaub \u{fc}ber",
            " #a b",
        ] {
            let text = format!(
                "2026-10-18 ! \"Shop\" \"x\"{rest}\n  lentil_id: \"x\"\n  Assets:Bank:Giro  -1.00 EUR\n  Expenses:Uncategorized\n"
            );
            assert!(
                indexed_tag_spans(&text).iter().all(Option::is_none),
                "{text:?}"
            );
        }
    }

    proptest::proptest! {
        /// Indexing never panics, whatever follows the narration or stands on
        /// the following lines.
        #[test]
        fn indexing_never_panics_on_what_follows_the_narration(
            rest in "[ \t#^;a\"\n\r\u{fc}\u{2713}-]{0,24}",
        ) {
            let text = format!(
                "2026-10-18 ! \"Shop\" \"x{rest}\n  lentil_id: \"x\"\n  Assets:Bank:Giro  -1.00 EUR\n  Expenses:Uncategorized\n"
            );
            indexed_tag_spans(&text);
            indexed_tag_spans(&text.replacen("\"x", "\"x\"", 1));
        }
    }

    #[test]
    fn original_text_joins_counterparty_and_purpose() {
        let ledger = load(&snapshot(), "main.beancount");
        let mut txn = ledger.find("giro-aaaaaaaaaaaa").unwrap().clone();
        assert_eq!(txn.original_text(), None);
        txn.purpose = Some("  Bread ".into());
        assert_eq!(txn.original_text().as_deref(), Some("Bread"));
        txn.counterparty = Some("Bakery".into());
        assert_eq!(txn.original_text().as_deref(), Some("Bakery Bread"));
        txn.purpose = None;
        assert_eq!(txn.original_text().as_deref(), Some("Bakery"));
        txn.counterparty = Some(" ".into());
        assert_eq!(txn.original_text(), None);
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
        assert_eq!(failure.line, Some(25));
    }

    #[test]
    fn open_queue_is_newest_first() {
        let mut s = snapshot();
        let extra = "2026-10-21 ! \"Later\" \"x\"\n  lentil_id: \"giro-bbbbbbbbbbbb\"\n  lentil_counterparty: \"LATER\"\n  Assets:Bank:Giro  -1.00 EUR\n  Expenses:Uncategorized\n";
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
    fn a_file_that_is_not_utf8_is_an_error() {
        let mut s = snapshot();
        let mut bytes = RUN.as_bytes().to_vec();
        bytes.extend_from_slice(b"; caf\xe9\n");
        s.insert(RUN_FILE, bytes);
        let ledger = load(&s, "main.beancount");
        let error = ledger.errors.iter().find(|e| e.code == "LOAD").unwrap();
        assert_eq!(error.file.as_deref(), Some(RUN_FILE));
        assert!(error.is_error);
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

    #[test]
    fn indexes_trip_events_whatever_their_name() {
        let manual = "manual/k7p2q9xd/2026-04.beancount";
        let text = "2026-04-10 event \"trip\" \"portugal-2026\"\n  lentil_id: \"k7p2q9xd-01jc8m2w9z4f7q3r5t6v8x0y1a\"\n2026-04-12 event \"location\" \"Lisbon\"\n2026-04-21 event \"trip\" \"\"\n";
        let mut s = snapshot();
        s.insert(
            "main.beancount",
            format!("{MAIN}include \"manual/*/*.beancount\"\n"),
        );
        let accounts = format!("{ACCOUNTS}2026-04-15 event \"trip\" \"x\"\n");
        s.insert("accounts.beancount", accounts.as_str());
        s.insert(manual, text);
        let ledger = load(&s, "main.beancount");
        assert!(ledger.errors.is_empty(), "{:?}", ledger.errors);
        let d = |s: &str| s.parse::<Date>().unwrap();
        assert_eq!(
            ledger.trip_events,
            vec![
                TripEvent {
                    id: None,
                    file: "accounts.beancount".to_owned(),
                    offset: ACCOUNTS.len(),
                    line: 4,
                    date: d("2026-04-15"),
                    name: "x".to_owned(),
                },
                TripEvent {
                    id: Some("k7p2q9xd-01jc8m2w9z4f7q3r5t6v8x0y1a".to_owned()),
                    file: manual.to_owned(),
                    offset: 0,
                    line: 1,
                    date: d("2026-04-10"),
                    name: "portugal-2026".to_owned(),
                },
                TripEvent {
                    id: None,
                    file: manual.to_owned(),
                    offset: text.find("2026-04-21").unwrap(),
                    line: 4,
                    date: d("2026-04-21"),
                    name: String::new(),
                },
            ]
        );
    }
}
