//! Edit operations as intents, and their replay (spec section 10).
//!
//! An intent names a transaction by `lentil_id` and carries the value the user
//! saw, so it can be re-applied to a newer repository state: if the current
//! value already equals the new value it is done, if it equals the old value it
//! is applied, otherwise it is a conflict.

use std::collections::{BTreeMap, HashMap};
use std::ops::Range;

use jiff::civil::Date;
use serde::{Deserialize, Serialize};

use crate::config::Config;
use crate::edit::{self, EditError};
use crate::ledger::{self, Ledger, Txn};
use crate::snapshot::Snapshot;
use crate::tags::is_tag_name;

/// One edit operation of spec section 10 (v0.1 subset).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "kebab-case")]
pub enum Intent {
    /// Set the flag of an active transaction to `*`.
    Confirm {
        /// `lentil_id` of the transaction.
        id: String,
    },
    /// Change the account of a posting that is not the source posting.
    SetAccount {
        /// `lentil_id` of the transaction.
        id: String,
        /// 0-based index of the posting in file order.
        posting: usize,
        /// The account the user saw.
        old: String,
        /// The account the user chose; must be open on the transaction date.
        new: String,
    },
    /// Set the payee of an active transaction, inserting one before the
    /// narration if it has none.
    SetPayee {
        /// `lentil_id` of the transaction.
        id: String,
        /// The payee the user saw, `None` if the transaction had none.
        old: Option<String>,
        /// The new payee.
        new: String,
    },
    /// Comment out an active transaction in place.
    Dismiss {
        /// `lentil_id` of the transaction.
        id: String,
    },
    /// Append `<date> open <account>` to `accounts.beancount`.
    OpenAccount {
        /// Account to open.
        account: String,
        /// Opening date.
        date: Date,
    },
    /// Write a new import run file.
    AddImportRun {
        /// Source ID.
        source: String,
        /// Repository-relative path of the run file.
        file: String,
        /// Complete content of the run file.
        content: String,
    },
    /// Add a tag to an active transaction, written into its header. It
    /// carries no old value: a tag already present is done (spec section 10).
    AddTag {
        /// `lentil_id` of the transaction.
        id: String,
        /// The tag, without `#`; must be a valid Beancount tag.
        tag: String,
    },
    /// Remove a tag from an active transaction wherever it is written. It
    /// carries no old value: a tag already absent is done (spec section 10).
    RemoveTag {
        /// `lentil_id` of the transaction.
        id: String,
        /// The tag, without `#`.
        tag: String,
    },
}

impl Intent {
    /// The operation name as used in commit subjects, such as `set-account`.
    #[must_use]
    pub fn op_name(&self) -> &'static str {
        match self {
            Self::Confirm { .. } => "confirm",
            Self::SetAccount { .. } => "set-account",
            Self::SetPayee { .. } => "set-payee",
            Self::Dismiss { .. } => "dismiss",
            Self::OpenAccount { .. } => "open-account",
            Self::AddImportRun { .. } => "add-import-run",
            Self::AddTag { .. } => "add-tag",
            Self::RemoveTag { .. } => "remove-tag",
        }
    }

    /// The transaction the intent addresses, if any.
    #[must_use]
    pub fn transaction_id(&self) -> Option<&str> {
        match self {
            Self::Confirm { id }
            | Self::SetAccount { id, .. }
            | Self::SetPayee { id, .. }
            | Self::Dismiss { id }
            | Self::AddTag { id, .. }
            | Self::RemoveTag { id, .. } => Some(id),
            Self::OpenAccount { .. } | Self::AddImportRun { .. } => None,
        }
    }
}

/// An intent stored on the device until it is pushed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingIntent {
    /// Unique key on the device, used to report outcomes and remove the intent.
    pub key: String,
    /// The operation.
    pub intent: Intent,
}

/// What replaying one intent did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// The precondition held and the change was made.
    Applied,
    /// The current state already equals the intended one; nothing changed.
    AlreadyApplied,
    /// Neither old nor new value matches; the intent leaves the queue and is
    /// shown to the user (spec section 11, step 3).
    Conflict(Conflict),
}

/// Why an intent could not be applied.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Conflict {
    /// The reason, translated by the client.
    pub code: ConflictCode,
    /// The value found instead of the expected old value, if any.
    pub current: Option<String>,
}

/// Reasons an intent conflicts.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ConflictCode {
    /// No active or dismissed transaction has the `lentil_id`.
    TransactionNotFound,
    /// The transaction was dismissed in the meantime.
    TransactionDismissed,
    /// The posting's account is neither the old nor the new value.
    AccountChanged,
    /// The payee is neither the old nor the new value.
    PayeeChanged,
    /// The posting index does not exist.
    PostingNotFound,
    /// The posting is on a source account, or the new account is one: editors
    /// never change a source posting or move a posting onto a source account.
    SourcePosting,
    /// The new account is not open on the transaction date.
    AccountNotOpen,
    /// The element is written in a form the core cannot edit byte-precisely,
    /// such as a header without a flag character or an `accounts.beancount`
    /// that is not UTF-8.
    NotEditable,
    /// The run file already exists with different content.
    FileExists,
    /// The account name is invalid.
    InvalidAccount,
    /// The tag of an `add-tag` is not a valid Beancount tag.
    InvalidTag,
}

/// The result of replaying intents on a snapshot.
#[derive(Clone, Debug)]
pub struct ReplayResult {
    /// The snapshot with all applied intents.
    pub snapshot: Snapshot,
    /// One outcome per input intent, keyed by [`PendingIntent::key`], in input order.
    pub outcomes: Vec<(String, Outcome)>,
}

/// Applies `intents` in order to `snapshot` (spec section 10).
///
/// Every applied intent changes only the bytes of the affected element; the
/// rest of each file stays byte-identical.
///
/// The ledger is loaded once. Later intents see the effect of earlier ones
/// (a confirmed flag, a changed account or payee, a dismissed transaction, an
/// opened account), and the edits are written into the files at the end.
/// Transactions in run files added by the same replay cannot be addressed; a
/// commit holds the changes of one role only (spec section 12), so importer
/// and editor intents never share a replay.
#[must_use]
pub fn replay(snapshot: &Snapshot, config: &Config, intents: &[PendingIntent]) -> ReplayResult {
    let ledger = ledger::load(snapshot, &config.main_file);
    let mut replay = Replay::new(snapshot, config, &ledger);
    let mut outcomes: Vec<(String, Outcome)> = intents
        .iter()
        .enumerate()
        .map(|(index, pending)| (pending.key.clone(), replay.apply(index, &pending.intent)))
        .collect();
    let snapshot = replay.finish(&mut outcomes);
    ReplayResult { snapshot, outcomes }
}

/// Commit subject summary of applied intents, such as
/// `confirm 12, set-account 3, dismiss 1` (spec section 12).
///
/// Operations appear in the order of the spec table; intents that were already
/// applied or conflicted are not counted.
#[must_use]
pub fn summary(intents: &[PendingIntent], outcomes: &[(String, Outcome)]) -> String {
    let mut ops: HashMap<&str, &Intent> = HashMap::with_capacity(intents.len());
    for pending in intents {
        ops.entry(pending.key.as_str()).or_insert(&pending.intent);
    }
    let mut counts = [("", 0_usize); OP_COUNT];
    for (key, outcome) in outcomes {
        if *outcome == Outcome::Applied
            && let Some(intent) = ops.get(key.as_str())
        {
            let (name, count) = &mut counts[intent.rank()];
            *name = intent.op_name();
            *count += 1;
        }
    }
    counts
        .iter()
        .filter(|(_, count)| *count > 0)
        .map(|(name, count)| format!("{name} {count}"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Number of v0.1 operations.
const OP_COUNT: usize = 8;

impl Intent {
    /// Position of the operation in the table of spec section 10.
    fn rank(&self) -> usize {
        match self {
            Self::Confirm { .. } => 0,
            Self::SetAccount { .. } => 1,
            Self::SetPayee { .. } => 2,
            Self::Dismiss { .. } => 3,
            Self::OpenAccount { .. } => 4,
            Self::AddImportRun { .. } => 5,
            Self::AddTag { .. } => 6,
            Self::RemoveTag { .. } => 7,
        }
    }
}

/// The flag of a reviewed transaction (spec section 6).
const REVIEWED: char = '*';

/// The file `open-account` appends to (spec section 3).
const ACCOUNTS_FILE: &str = "accounts.beancount";

/// The account roots of Beancount. Options that rename them are outside the
/// profile (spec section 14), so an account under any other root is invalid.
const ACCOUNT_ROOTS: [&str; 5] = ["Assets", "Liabilities", "Equity", "Income", "Expenses"];

/// Whether `account` is a valid account name under one of the [`ACCOUNT_ROOTS`].
fn is_valid_account(account: &str) -> bool {
    rustledger_parser::is_valid_account_name(account)
        && account
            .split_once(':')
            .is_some_and(|(root, _)| ACCOUNT_ROOTS.contains(&root))
}

/// The state of one replay: the ledger as loaded, plus the virtual effect of
/// the intents applied so far.
struct Replay<'a> {
    snapshot: &'a Snapshot,
    config: &'a Config,
    ledger: &'a Ledger,
    /// Index into `ledger.transactions` by `lentil_id`. The first transaction
    /// with an ID wins, like [`Ledger::find`]; duplicates are a validation error.
    by_id: HashMap<&'a str, usize>,
    /// Text of each file the ledger reached, checked as UTF-8 once.
    texts: HashMap<&'a str, &'a str>,
    /// Virtual state of each transaction an intent touched, by index.
    touched: BTreeMap<usize, TxnState>,
    /// Accounts opened by earlier intents, with their opening dates.
    opened: HashMap<String, Date>,
    /// Lines to append to [`ACCOUNTS_FILE`], with the intent that adds each.
    open_lines: Vec<(usize, String)>,
    /// Run files written by earlier intents.
    new_files: BTreeMap<String, String>,
}

/// The virtual state of one transaction during a replay.
#[derive(Debug)]
struct TxnState {
    flag: char,
    payee: Option<String>,
    /// Account of each posting, in file order.
    accounts: Vec<String>,
    /// Tags without duplicates: the parsed ones in file order, then the
    /// added ones in the order they were added.
    tags: Vec<String>,
    dismissed: bool,
    /// Intents whose outcome depends on this state: applied, or found already
    /// applied because of an earlier intent.
    intents: Vec<usize>,
}

impl TxnState {
    fn new(txn: &Txn) -> Self {
        Self {
            flag: txn.flag,
            payee: txn.payee.clone(),
            accounts: txn.postings.iter().map(|p| p.account.clone()).collect(),
            tags: txn.tags.iter().fold(Vec::new(), |mut tags, tag| {
                if !tags.contains(tag) {
                    tags.push(tag.clone());
                }
                tags
            }),
            dismissed: false,
            intents: Vec::new(),
        }
    }
}

/// Where a `lentil_id` stands at this point of a replay.
enum Lookup {
    /// An active transaction, by index into `ledger.transactions`.
    Active(usize),
    /// Commented out, in the repository or by an earlier intent.
    Dismissed,
    /// Neither active nor dismissed.
    NotFound,
}

/// A conflict without a current value.
fn conflict(code: ConflictCode) -> Outcome {
    Outcome::Conflict(Conflict {
        code,
        current: None,
    })
}

impl<'a> Replay<'a> {
    fn new(snapshot: &'a Snapshot, config: &'a Config, ledger: &'a Ledger) -> Self {
        let mut by_id = HashMap::with_capacity(ledger.transactions.len());
        for (index, txn) in ledger.transactions.iter().enumerate() {
            if let Some(id) = &txn.id {
                by_id.entry(id.as_str()).or_insert(index);
            }
        }
        let texts = ledger
            .files
            .iter()
            .filter_map(|file| Some((file.as_str(), snapshot.text(file)?)))
            .collect();
        Self {
            snapshot,
            config,
            ledger,
            by_id,
            texts,
            touched: BTreeMap::new(),
            opened: HashMap::new(),
            open_lines: Vec::new(),
            new_files: BTreeMap::new(),
        }
    }

    /// Applies intent number `index` to the virtual state.
    fn apply(&mut self, index: usize, intent: &Intent) -> Outcome {
        match intent {
            Intent::Confirm { id } => self.confirm(index, id),
            Intent::SetAccount {
                id,
                posting,
                old,
                new,
            } => self.set_account(index, id, *posting, old, new),
            Intent::SetPayee { id, old, new } => self.set_payee(index, id, old.as_deref(), new),
            Intent::Dismiss { id } => self.dismiss(index, id),
            Intent::OpenAccount { account, date } => self.open_account(index, account, *date),
            Intent::AddImportRun { file, content, .. } => self.add_import_run(file, content),
            Intent::AddTag { id, tag } => self.add_tag(index, id, tag),
            Intent::RemoveTag { id, tag } => self.remove_tag(index, id, tag),
        }
    }

    fn lookup(&self, id: &str) -> Lookup {
        match self.by_id.get(id) {
            Some(&i) if self.touched.get(&i).is_some_and(|s| s.dismissed) => Lookup::Dismissed,
            Some(&i) => Lookup::Active(i),
            None if self.ledger.dismissed_ids.contains(id) => Lookup::Dismissed,
            None => Lookup::NotFound,
        }
    }

    /// The active transaction `id`, or the conflict an edit of it runs into.
    fn active(&self, id: &str) -> Result<usize, Outcome> {
        match self.lookup(id) {
            Lookup::Active(i) => Ok(i),
            Lookup::Dismissed => Err(conflict(ConflictCode::TransactionDismissed)),
            Lookup::NotFound => Err(conflict(ConflictCode::TransactionNotFound)),
        }
    }

    fn state_mut(&mut self, i: usize) -> &mut TxnState {
        let txn = &self.ledger.transactions[i];
        self.touched.entry(i).or_insert_with(|| TxnState::new(txn))
    }

    /// Records that intent `index` found transaction `i` already in the
    /// intended state. Only a touched transaction's state can be undone by
    /// [`Self::finish`], so an untouched one needs no record.
    fn already_applied(&mut self, i: usize, index: usize) -> Outcome {
        if let Some(state) = self.touched.get_mut(&i) {
            state.intents.push(index);
        }
        Outcome::AlreadyApplied
    }

    fn text(&self, txn: &Txn) -> Option<&'a str> {
        self.texts.get(txn.file.as_str()).copied()
    }

    fn confirm(&mut self, index: usize, id: &str) -> Outcome {
        let i = match self.active(id) {
            Ok(i) => i,
            Err(outcome) => return outcome,
        };
        let txn = &self.ledger.transactions[i];
        let flag = self.touched.get(&i).map_or(txn.flag, |s| s.flag);
        if flag == REVIEWED {
            return self.already_applied(i, index);
        }
        if !self.text(txn).is_some_and(|text| flag_editable(text, txn)) {
            return conflict(ConflictCode::NotEditable);
        }
        let state = self.state_mut(i);
        state.flag = REVIEWED;
        state.intents.push(index);
        Outcome::Applied
    }

    fn set_account(
        &mut self,
        index: usize,
        id: &str,
        posting: usize,
        old: &str,
        new: &str,
    ) -> Outcome {
        let i = match self.active(id) {
            Ok(i) => i,
            Err(outcome) => return outcome,
        };
        let txn = &self.ledger.transactions[i];
        let Some(original) = txn.postings.get(posting) else {
            return conflict(ConflictCode::PostingNotFound);
        };
        let current = self
            .touched
            .get(&i)
            .and_then(|s| s.accounts.get(posting))
            .map_or(original.account.as_str(), String::as_str);
        // Done comes first (spec section 10, step 2), so a replay after a
        // successful push never turns into a conflict.
        if current == new {
            return self.already_applied(i, index);
        }
        // Every posting on a source account is a source posting, and no
        // posting moves onto one (spec section 7).
        if self.config.is_source_account(current) || self.config.is_source_account(new) {
            return conflict(ConflictCode::SourcePosting);
        }
        if current != old {
            return Outcome::Conflict(Conflict {
                code: ConflictCode::AccountChanged,
                current: Some(current.to_owned()),
            });
        }
        if !self.is_open_on(new, txn.date) {
            return conflict(ConflictCode::AccountNotOpen);
        }
        let editable = self.text(txn).is_some_and(|text| {
            within(&original.account_span, &txn.span)
                && text.get(original.account_span.clone()) == Some(original.account.as_str())
        });
        if !editable {
            return conflict(ConflictCode::NotEditable);
        }
        let state = self.state_mut(i);
        state.accounts[posting] = new.to_owned();
        state.intents.push(index);
        Outcome::Applied
    }

    fn set_payee(&mut self, index: usize, id: &str, old: Option<&str>, new: &str) -> Outcome {
        let i = match self.active(id) {
            Ok(i) => i,
            Err(outcome) => return outcome,
        };
        let txn = &self.ledger.transactions[i];
        let current = self
            .touched
            .get(&i)
            .map_or(txn.payee.as_deref(), |s| s.payee.as_deref());
        if current == Some(new) {
            return self.already_applied(i, index);
        }
        if current != old {
            return Outcome::Conflict(Conflict {
                code: ConflictCode::PayeeChanged,
                current: current.map(ToOwned::to_owned),
            });
        }
        if !self.text(txn).is_some_and(|text| payee_editable(text, txn)) {
            return conflict(ConflictCode::NotEditable);
        }
        let state = self.state_mut(i);
        state.payee = Some(new.to_owned());
        state.intents.push(index);
        Outcome::Applied
    }

    /// Whether transaction `i` carries `tag` at this point of the replay.
    fn has_tag(&self, i: usize, tag: &str) -> bool {
        match self.touched.get(&i) {
            Some(state) => state.tags.iter().any(|t| t == tag),
            None => self.ledger.transactions[i].tags.iter().any(|t| t == tag),
        }
    }

    /// Whether the tags of `txn` can be edited byte-precisely: their
    /// positions are known and lie inside the transaction.
    fn tags_editable(&self, txn: &Txn) -> bool {
        self.text(txn).is_some()
            && txn.tag_spans.as_ref().is_some_and(|spans| {
                within(&(spans.insert_at..spans.insert_at), &txn.span)
                    && spans.tags.iter().all(|(_, span)| within(span, &txn.span))
            })
    }

    /// `add-tag` (spec section 10): done if the tag is present, otherwise it
    /// goes into the header.
    fn add_tag(&mut self, index: usize, id: &str, tag: &str) -> Outcome {
        if !is_tag_name(tag) {
            return conflict(ConflictCode::InvalidTag);
        }
        let i = match self.active(id) {
            Ok(i) => i,
            Err(outcome) => return outcome,
        };
        if self.has_tag(i, tag) {
            return self.already_applied(i, index);
        }
        if !self.tags_editable(&self.ledger.transactions[i]) {
            return conflict(ConflictCode::NotEditable);
        }
        let state = self.state_mut(i);
        state.tags.push(tag.to_owned());
        state.intents.push(index);
        Outcome::Applied
    }

    /// `remove-tag` (spec section 10): done if the tag is absent, otherwise
    /// every occurrence is removed.
    fn remove_tag(&mut self, index: usize, id: &str, tag: &str) -> Outcome {
        let i = match self.active(id) {
            Ok(i) => i,
            Err(outcome) => return outcome,
        };
        if !self.has_tag(i, tag) {
            return self.already_applied(i, index);
        }
        if !self.tags_editable(&self.ledger.transactions[i]) {
            return conflict(ConflictCode::NotEditable);
        }
        let state = self.state_mut(i);
        state.tags.retain(|t| t != tag);
        state.intents.push(index);
        Outcome::Applied
    }

    fn dismiss(&mut self, index: usize, id: &str) -> Outcome {
        let i = match self.lookup(id) {
            Lookup::Active(i) => i,
            Lookup::Dismissed => {
                if let Some(&i) = self.by_id.get(id) {
                    return self.already_applied(i, index);
                }
                return Outcome::AlreadyApplied;
            }
            Lookup::NotFound => return conflict(ConflictCode::TransactionNotFound),
        };
        let txn = &self.ledger.transactions[i];
        if !self
            .text(txn)
            .is_some_and(|text| dismiss_editable(text, txn))
        {
            return conflict(ConflictCode::NotEditable);
        }
        let state = self.state_mut(i);
        state.dismissed = true;
        state.intents.push(index);
        Outcome::Applied
    }

    /// Whether `account` is open on `date`, counting accounts opened by
    /// earlier intents.
    fn is_open_on(&self, account: &str, date: Date) -> bool {
        self.ledger.is_open_on(account, date)
            || self.opened.get(account).is_some_and(|d| *d <= date)
    }

    fn open_account(&mut self, index: usize, account: &str, date: Date) -> Outcome {
        if !is_valid_account(account) {
            return conflict(ConflictCode::InvalidAccount);
        }
        let in_ledger = self
            .ledger
            .accounts
            .get(account)
            .is_some_and(|s| s.opened.is_some());
        if in_ledger || self.opened.contains_key(account) {
            return Outcome::AlreadyApplied;
        }
        if self.snapshot.contains(ACCOUNTS_FILE) && self.snapshot.text(ACCOUNTS_FILE).is_none() {
            return conflict(ConflictCode::NotEditable);
        }
        self.opened.insert(account.to_owned(), date);
        self.open_lines
            .push((index, format!("{date} open {account}")));
        Outcome::Applied
    }

    fn add_import_run(&mut self, file: &str, content: &str) -> Outcome {
        let existing = self
            .new_files
            .get(file)
            .map(String::as_bytes)
            .or_else(|| self.snapshot.get(file));
        match existing {
            Some(bytes) if bytes == content.as_bytes() => Outcome::AlreadyApplied,
            Some(_) => conflict(ConflictCode::FileExists),
            None if file == ACCOUNTS_FILE && !self.open_lines.is_empty() => {
                conflict(ConflictCode::FileExists)
            }
            None => {
                self.new_files.insert(file.to_owned(), content.to_owned());
                Outcome::Applied
            }
        }
    }

    /// Writes the virtual state into a copy of the snapshot.
    ///
    /// Per file, each touched transaction is rendered on its own (element
    /// edits in descending offset order, then commented out if dismissed) and
    /// the file is rebuilt in one pass, so the cost is linear in the file size.
    /// The checks in the intent handlers make every edit succeed; should one
    /// still fail, the transaction keeps its bytes and the intents that relied
    /// on it become [`ConflictCode::NotEditable`] conflicts, so nothing is
    /// dropped silently.
    fn finish(self, outcomes: &mut [(String, Outcome)]) -> Snapshot {
        let mut out = self.snapshot.clone();

        let mut by_file: BTreeMap<&str, Vec<(&Txn, &TxnState)>> = BTreeMap::new();
        for (&i, state) in &self.touched {
            let txn = &self.ledger.transactions[i];
            by_file.entry(&txn.file).or_default().push((txn, state));
        }
        for (file, mut txns) in by_file {
            let Some(&text) = self.texts.get(file) else {
                for (_, state) in &txns {
                    not_editable(outcomes, &state.intents);
                }
                continue;
            };
            txns.sort_by_key(|(txn, _)| txn.span.start);
            let mut rebuilt = String::with_capacity(text.len() + text.len() / 8);
            let mut pos = 0;
            for (txn, state) in txns {
                let gap = text.get(pos..txn.span.start);
                match (gap, render(text, txn, state)) {
                    (Some(_), Ok(None)) => {}
                    (Some(gap), Ok(Some(rendered))) => {
                        rebuilt.push_str(gap);
                        rebuilt.push_str(&rendered);
                        pos = txn.span.end;
                    }
                    _ => not_editable(outcomes, &state.intents),
                }
            }
            rebuilt.push_str(&text[pos..]);
            out.insert(file, rebuilt);
        }

        for (file, content) in self.new_files {
            out.insert(file, content);
        }

        if !self.open_lines.is_empty() {
            match out.get(ACCOUNTS_FILE).map(std::str::from_utf8).transpose() {
                Ok(text) => {
                    let mut text = text.unwrap_or_default().to_owned();
                    for (_, line) in &self.open_lines {
                        text = edit::append_line(&text, line);
                    }
                    out.insert(ACCOUNTS_FILE, text);
                }
                Err(_) => {
                    let indices: Vec<usize> = self.open_lines.iter().map(|(i, _)| *i).collect();
                    not_editable(outcomes, &indices);
                }
            }
        }
        out
    }
}

/// Turns the outcomes of `indices` into [`ConflictCode::NotEditable`] conflicts.
fn not_editable(outcomes: &mut [(String, Outcome)], indices: &[usize]) {
    for &index in indices {
        if let Some((_, outcome)) = outcomes.get_mut(index) {
            *outcome = conflict(ConflictCode::NotEditable);
        }
    }
}

/// Whether `inner` lies within `outer`.
fn within(inner: &Range<usize>, outer: &Range<usize>) -> bool {
    outer.start <= inner.start && inner.end <= outer.end && inner.start <= inner.end
}

/// Whether the flag of `txn` sits at its `flag_offset`, inside its span.
fn flag_editable(text: &str, txn: &Txn) -> bool {
    txn.flag_offset.is_some_and(|offset| {
        within(
            &(offset..offset.saturating_add(txn.flag.len_utf8())),
            &txn.span,
        ) && text
            .get(offset..)
            .is_some_and(|rest| rest.starts_with(txn.flag))
    })
}

/// Whether the payee string of `txn`, or without a payee its narration
/// string, sits at its span inside the transaction's span, written as
/// [`edit::quote`] writes it.
fn payee_editable(text: &str, txn: &Txn) -> bool {
    let reads = |span: &Range<usize>, value: &str| {
        within(span, &txn.span) && text.get(span.clone()) == Some(edit::quote(value).as_str())
    };
    match (&txn.payee, &txn.payee_span, &txn.narration_span) {
        (Some(payee), Some(span), _) => reads(span, payee),
        (None, None, Some(span)) => reads(span, &txn.narration),
        _ => false,
    }
}

/// Whether `txn` starts at a line start and all its postings lie in its span,
/// so commenting out the span leaves no stray line behind.
fn dismiss_editable(text: &str, txn: &Txn) -> bool {
    text.get(..txn.span.start)
        .is_some_and(|before| before.is_empty() || before.ends_with('\n'))
        && text.get(txn.span.clone()).is_some()
        && txn.postings.iter().all(|p| within(&p.span, &txn.span))
}

/// One element edit inside a transaction, at an absolute offset.
enum Element<'s> {
    Flag {
        offset: usize,
        old: char,
        new: char,
    },
    Account {
        span: Range<usize>,
        old: &'s str,
        new: &'s str,
    },
    Payee {
        payee_span: Option<Range<usize>>,
        narration_span: Range<usize>,
        old: Option<&'s str>,
        new: &'s str,
    },
    /// Tags added, in the order they were added, inserted at `offset`.
    Tags {
        offset: usize,
        tags: Vec<&'s str>,
    },
    /// One written occurrence of a removed tag.
    RemoveTag {
        span: Range<usize>,
        tag: &'s str,
    },
}

impl Element<'_> {
    fn start(&self) -> usize {
        match self {
            Self::Flag { offset, .. } => *offset,
            Self::Account { span, .. } | Self::RemoveTag { span, .. } => span.start,
            Self::Tags { offset, .. } => *offset,
            Self::Payee {
                payee_span,
                narration_span,
                ..
            } => payee_span.as_ref().unwrap_or(narration_span).start,
        }
    }
}

/// The new bytes of `txn`'s span for its virtual `state`, or `None` if
/// nothing changed.
fn render(text: &str, txn: &Txn, state: &TxnState) -> Result<Option<String>, EditError> {
    let mismatch = || EditError::Mismatch(txn.span.clone());
    let original = text.get(txn.span.clone()).ok_or_else(mismatch)?;

    let mut elements = Vec::new();
    if state.flag != txn.flag {
        let offset = txn.flag_offset.ok_or_else(mismatch)?;
        elements.push(Element::Flag {
            offset,
            old: txn.flag,
            new: state.flag,
        });
    }
    if state.payee != txn.payee {
        elements.push(Element::Payee {
            payee_span: txn.payee_span.clone(),
            narration_span: txn.narration_span.clone().ok_or_else(mismatch)?,
            old: txn.payee.as_deref(),
            new: state.payee.as_deref().ok_or_else(mismatch)?,
        });
    }
    for (posting, account) in txn.postings.iter().zip(&state.accounts) {
        if posting.account != *account {
            elements.push(Element::Account {
                span: posting.account_span.clone(),
                old: &posting.account,
                new: account,
            });
        }
    }
    let added: Vec<&str> = state
        .tags
        .iter()
        .filter(|tag| !txn.tags.contains(tag))
        .map(String::as_str)
        .collect();
    let removed = |tag: &String| !state.tags.contains(tag);
    if !added.is_empty() || txn.tags.iter().any(removed) {
        let spans = txn.tag_spans.as_ref().ok_or_else(mismatch)?;
        if !added.is_empty() {
            elements.push(Element::Tags {
                offset: spans.insert_at,
                tags: added,
            });
        }
        for (tag, span) in spans.tags.iter().filter(|(tag, _)| removed(tag)) {
            elements.push(Element::RemoveTag {
                span: span.clone(),
                tag,
            });
        }
    }
    if elements.is_empty() && !state.dismissed {
        return Ok(None);
    }

    // Descending offsets keep the positions of the remaining edits valid.
    elements.sort_by_key(|e| std::cmp::Reverse(e.start()));
    let base = txn.span.start;
    let mut body = original.to_owned();
    for element in elements {
        let relative = element.start().checked_sub(base).ok_or_else(mismatch)?;
        body = match element {
            Element::Flag { old, new, .. } => edit::set_flag(&body, relative, old, new)?,
            Element::Account { span, old, new } => {
                edit::set_account(&body, relative..relative + span.len(), old, new)?
            }
            Element::Payee {
                payee_span,
                narration_span,
                old,
                new,
            } => {
                let narration = narration_span
                    .start
                    .checked_sub(base)
                    .ok_or_else(mismatch)?;
                edit::set_payee(
                    &body,
                    payee_span.map(|span| relative..relative + span.len()),
                    narration..narration + narration_span.len(),
                    old,
                    new,
                )?
            }
            Element::Tags { tags, .. } => edit::insert_tags(&body, relative, &tags)?,
            Element::RemoveTag { span, tag } => {
                edit::remove_tag(&body, relative..relative + span.len(), tag)?
            }
        };
    }
    if state.dismissed {
        if !dismiss_editable(text, txn) {
            return Err(mismatch());
        }
        body = edit::comment_out(&body, 0..body.len())?;
    }
    Ok(Some(body))
}

#[cfg(test)]
mod tests {
    use super::*;

    const CONFIG: &str = r#"
spec_version = 0
main_file = "main.beancount"
fallback_account = "Expenses:Uncategorized"
unattributed_account = "Expenses:Unattributed"

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
"#;

    const MAIN: &str = r#"option "title" "Ledger"
option "operating_currency" "EUR"

include "accounts.beancount"
include "imports/*/*/*.beancount"
"#;

    const ACCOUNTS: &str = "2020-01-01 open Assets:Bank:Giro EUR
2020-01-01 open Liabilities:Card EUR
2020-01-01 open Assets:Transfer
2020-01-01 open Expenses:Uncategorized
2020-01-01 open Expenses:Food:Groceries
2020-01-01 open Expenses:Old
2021-01-01 close Expenses:Old
2026-10-19 open Expenses:Late
";

    const RUN_FILE: &str = "imports/giro/2026/20261020T071502Z.beancount";

    const GROCERY_BLOCK: &str = r#"2026-10-18 ! "Corner Grocery" "Card payment"
  lentil_id: "giro-3f9a1c2e7b4d"
  lentil_counterparty: "CORNER GROCERY 4711"
  lentil_purpose: "CARD PAYMENT"
  Assets:Bank:Giro        -42.17 EUR
  Expenses:Uncategorized"#;

    const RUN: &str = r#"; lentil import run
; source: giro, started: 2026-10-20T07:15:02Z, client: lentil-cli/0.1.0

2026-10-18 ! "Corner Grocery" "Card payment"
  lentil_id: "giro-3f9a1c2e7b4d"
  lentil_counterparty: "CORNER GROCERY 4711"
  lentil_purpose: "CARD PAYMENT"
  Assets:Bank:Giro        -42.17 EUR
  Expenses:Uncategorized

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

2026-10-20 ! "Transfer" "To card"
  lentil_id: "giro-bbbbbbbbbbbb"
  lentil_counterparty: "TRANSFER CARD"
  Assets:Bank:Giro  -100.00 EUR
  Expenses:Uncategorized

2026-10-21 balance Assets:Bank:Giro  -145.27 ~ 0.00 EUR
"#;

    const CARD_FILE: &str = "imports/card/2026/20261022T080000Z.beancount";
    /// Ends without a line break after its only transaction.
    const CARD_RUN: &str = "2026-10-21 ! \"Café\" \"Coffee\"\n  lentil_id: \"card-cccccccccccc\"\n  lentil_counterparty: \"CAFE\"\n  Liabilities:Card  -5.00 EUR\n  Expenses:Uncategorized";

    const GROCERY: &str = "giro-3f9a1c2e7b4d";
    const UTILITIES: &str = "giro-8c01d4e9a2f7";
    const BAKERY: &str = "giro-aaaaaaaaaaaa";
    const TRANSFER: &str = "giro-bbbbbbbbbbbb";
    const CAFE: &str = "card-cccccccccccc";

    fn config() -> Config {
        Config::parse(CONFIG).unwrap()
    }

    fn snapshot() -> Snapshot {
        Snapshot::from_files([
            ("main.beancount", MAIN),
            ("accounts.beancount", ACCOUNTS),
            (RUN_FILE, RUN),
        ])
    }

    fn date(s: &str) -> Date {
        s.parse().unwrap()
    }

    fn confirm(id: &str) -> Intent {
        Intent::Confirm { id: id.into() }
    }

    fn dismiss(id: &str) -> Intent {
        Intent::Dismiss { id: id.into() }
    }

    fn set(id: &str, posting: usize, old: &str, new: &str) -> Intent {
        Intent::SetAccount {
            id: id.into(),
            posting,
            old: old.into(),
            new: new.into(),
        }
    }

    fn payee(id: &str, old: Option<&str>, new: &str) -> Intent {
        Intent::SetPayee {
            id: id.into(),
            old: old.map(Into::into),
            new: new.into(),
        }
    }

    fn open(account: &str, day: &str) -> Intent {
        Intent::OpenAccount {
            account: account.into(),
            date: date(day),
        }
    }

    fn run_file(file: &str, content: &str) -> Intent {
        Intent::AddImportRun {
            source: "giro".into(),
            file: file.into(),
            content: content.into(),
        }
    }

    fn pending(intents: &[Intent]) -> Vec<PendingIntent> {
        intents
            .iter()
            .enumerate()
            .map(|(i, intent)| PendingIntent {
                key: format!("k{i}"),
                intent: intent.clone(),
            })
            .collect()
    }

    /// Replays `intents` and returns the new snapshot and the outcomes.
    fn replay_on(snapshot: &Snapshot, intents: &[Intent]) -> (Snapshot, Vec<Outcome>) {
        let result = replay(snapshot, &config(), &pending(intents));
        let keys: Vec<_> = result.outcomes.iter().map(|(k, _)| k.clone()).collect();
        let expected: Vec<_> = (0..intents.len()).map(|i| format!("k{i}")).collect();
        assert_eq!(keys, expected);
        (
            result.snapshot,
            result.outcomes.into_iter().map(|(_, o)| o).collect(),
        )
    }

    /// Replays `intents` on a hand-made ledger, to reach states the parser
    /// never produces.
    fn replay_with(
        snapshot: &Snapshot,
        ledger: &Ledger,
        intents: &[Intent],
    ) -> (Snapshot, Vec<Outcome>) {
        let config = config();
        let mut replay = Replay::new(snapshot, &config, ledger);
        let mut outcomes: Vec<_> = intents
            .iter()
            .enumerate()
            .map(|(i, intent)| (format!("k{i}"), replay.apply(i, intent)))
            .collect();
        let snapshot = replay.finish(&mut outcomes);
        (snapshot, outcomes.into_iter().map(|(_, o)| o).collect())
    }

    fn reload(snapshot: &Snapshot) -> Ledger {
        let ledger = ledger::load(snapshot, "main.beancount");
        assert!(ledger.errors.is_empty(), "{:?}", ledger.errors);
        ledger
    }

    fn commented(block: &str) -> String {
        block
            .split_inclusive('\n')
            .map(|l| format!("; {l}"))
            .collect()
    }

    fn changed(text: &str, from: &str, to: &str) -> String {
        assert!(text.contains(from), "{from}");
        text.replacen(from, to, 1)
    }

    use Outcome::{AlreadyApplied, Applied};

    #[test]
    fn intents_name_their_operation_and_transaction() {
        let cases = [
            (confirm(GROCERY), "confirm", Some(GROCERY)),
            (set(TRANSFER, 1, "a", "b"), "set-account", Some(TRANSFER)),
            (payee(BAKERY, None, "x"), "set-payee", Some(BAKERY)),
            (dismiss(CAFE), "dismiss", Some(CAFE)),
            (open("Expenses:Fresh", "2026-01-01"), "open-account", None),
            (run_file("x", "y"), "add-import-run", None),
            (add_tag(GROCERY, "a"), "add-tag", Some(GROCERY)),
            (remove_tag(BAKERY, "food"), "remove-tag", Some(BAKERY)),
        ];
        for (intent, name, id) in cases {
            assert_eq!((intent.op_name(), intent.transaction_id()), (name, id));
        }
    }

    #[test]
    fn confirm_changes_only_the_flag() {
        let (after, outcomes) = replay_on(&snapshot(), &[confirm(GROCERY)]);
        assert_eq!(outcomes, vec![Applied]);
        assert_eq!(
            after.text(RUN_FILE).unwrap(),
            changed(RUN, "2026-10-18 ! ", "2026-10-18 * ")
        );
        assert_eq!(after.text("accounts.beancount"), Some(ACCOUNTS));
        assert_eq!(after.text("main.beancount"), Some(MAIN));
        assert_eq!(reload(&after).find(GROCERY).unwrap().flag, '*');
    }

    #[test]
    fn confirm_is_done_on_a_reviewed_transaction() {
        let (after, outcomes) = replay_on(
            &snapshot(),
            &[confirm(BAKERY), confirm(GROCERY), confirm(GROCERY)],
        );
        assert_eq!(outcomes, vec![AlreadyApplied, Applied, AlreadyApplied]);
        assert_eq!(
            after.text(RUN_FILE).unwrap(),
            changed(RUN, "2026-10-18 ! ", "2026-10-18 * ")
        );
    }

    #[test]
    fn confirm_conflicts_on_dismissed_and_unknown_transactions() {
        let (after, outcomes) = replay_on(
            &snapshot(),
            &[confirm(UTILITIES), confirm("giro-000000000000")],
        );
        assert_eq!(
            outcomes,
            vec![
                conflict(ConflictCode::TransactionDismissed),
                conflict(ConflictCode::TransactionNotFound)
            ]
        );
        assert_eq!(after, snapshot());
    }

    #[test]
    fn set_account_changes_only_the_account() {
        let (after, outcomes) = replay_on(
            &snapshot(),
            &[set(
                GROCERY,
                1,
                "Expenses:Uncategorized",
                "Expenses:Food:Groceries",
            )],
        );
        assert_eq!(outcomes, vec![Applied]);
        assert_eq!(
            after.text(RUN_FILE).unwrap(),
            changed(
                RUN,
                "  Expenses:Uncategorized\n\n; 2026-10-19",
                "  Expenses:Food:Groceries\n\n; 2026-10-19"
            )
        );
        let ledger = reload(&after);
        let txn = ledger.find(GROCERY).unwrap();
        assert_eq!(txn.postings[1].account, "Expenses:Food:Groceries");
        assert_eq!(txn.flag, '!');
    }

    #[test]
    fn set_account_is_done_when_the_account_is_already_new() {
        let intent = set(
            GROCERY,
            1,
            "Expenses:Uncategorized",
            "Expenses:Food:Groceries",
        );
        let (after, outcomes) = replay_on(&snapshot(), &[intent.clone(), intent.clone()]);
        assert_eq!(outcomes, vec![Applied, AlreadyApplied]);
        let (again, outcomes) = replay_on(&after, &[intent]);
        assert_eq!(outcomes, vec![AlreadyApplied]);
        assert_eq!(again, after);
    }

    #[test]
    fn set_account_reports_the_current_account_on_conflict() {
        let (after, outcomes) = replay_on(
            &snapshot(),
            &[set(GROCERY, 1, "Expenses:Food:Groceries", "Expenses:Late")],
        );
        assert_eq!(
            outcomes,
            vec![Outcome::Conflict(Conflict {
                code: ConflictCode::AccountChanged,
                current: Some("Expenses:Uncategorized".into()),
            })]
        );
        assert_eq!(after, snapshot());
    }

    #[test]
    fn set_account_never_changes_a_source_posting() {
        let (after, outcomes) = replay_on(
            &snapshot(),
            &[set(
                GROCERY,
                0,
                "Assets:Bank:Giro",
                "Expenses:Uncategorized",
            )],
        );
        assert_eq!(outcomes, vec![conflict(ConflictCode::SourcePosting)]);
        assert_eq!(after, snapshot());
    }

    #[test]
    fn set_account_never_moves_a_posting_onto_a_source_account() {
        // A transfer goes through the transfer account on both sides, so each
        // side keeps its bank's date (spec section 8).
        for account in ["Liabilities:Card", "Assets:Bank:Giro"] {
            let (after, outcomes) = replay_on(
                &snapshot(),
                &[set(TRANSFER, 1, "Expenses:Uncategorized", account)],
            );
            assert_eq!(outcomes, vec![conflict(ConflictCode::SourcePosting)]);
            assert_eq!(after, snapshot());
        }
    }

    #[test]
    fn set_account_needs_an_existing_posting() {
        let (_, outcomes) = replay_on(
            &snapshot(),
            &[set(GROCERY, 2, "Expenses:Uncategorized", "Expenses:Late")],
        );
        assert_eq!(outcomes, vec![conflict(ConflictCode::PostingNotFound)]);
    }

    #[test]
    fn set_account_needs_an_account_open_on_the_transaction_date() {
        let uncategorized = "Expenses:Uncategorized";
        let (after, outcomes) = replay_on(
            &snapshot(),
            &[
                set(GROCERY, 1, uncategorized, "Expenses:Nope"),
                set(GROCERY, 1, uncategorized, "Expenses:Old"),
                set(GROCERY, 1, uncategorized, "Expenses:Late"),
                set(TRANSFER, 1, uncategorized, "Expenses:Late"),
            ],
        );
        let not_open = conflict(ConflictCode::AccountNotOpen);
        assert_eq!(
            outcomes,
            vec![not_open.clone(), not_open.clone(), not_open, Applied]
        );
        assert_eq!(
            reload(&after).find(TRANSFER).unwrap().postings[1].account,
            "Expenses:Late"
        );
    }

    #[test]
    fn set_account_counts_accounts_opened_earlier_in_the_replay() {
        let uncategorized = "Expenses:Uncategorized";
        let (after, outcomes) = replay_on(
            &snapshot(),
            &[
                set(GROCERY, 1, uncategorized, "Expenses:Fresh"),
                open("Expenses:Fresh", "2026-01-01"),
                set(GROCERY, 1, uncategorized, "Expenses:Fresh"),
                open("Expenses:Future", "2026-12-01"),
                set(TRANSFER, 1, uncategorized, "Expenses:Future"),
            ],
        );
        let not_open = conflict(ConflictCode::AccountNotOpen);
        assert_eq!(
            outcomes,
            vec![not_open.clone(), Applied, Applied, Applied, not_open]
        );
        let ledger = reload(&after);
        assert_eq!(
            ledger.find(GROCERY).unwrap().postings[1].account,
            "Expenses:Fresh"
        );
    }

    #[test]
    fn set_account_conflicts_on_a_dismissed_transaction() {
        let (_, outcomes) = replay_on(
            &snapshot(),
            &[set(UTILITIES, 1, "Expenses:Uncategorized", "Expenses:Late")],
        );
        assert_eq!(outcomes, vec![conflict(ConflictCode::TransactionDismissed)]);
    }

    #[test]
    fn set_payee_changes_only_the_payee() {
        let (after, outcomes) = replay_on(
            &snapshot(),
            &[payee(
                GROCERY,
                Some("Corner Grocery"),
                r#"Corner "Shop" \ 1"#,
            )],
        );
        assert_eq!(outcomes, vec![Applied]);
        assert_eq!(
            after.text(RUN_FILE).unwrap(),
            changed(
                RUN,
                "\"Corner Grocery\" \"Card",
                r#""Corner \"Shop\" \\ 1" "Card"#
            )
        );
        assert_eq!(after.text("accounts.beancount"), Some(ACCOUNTS));
        let ledger = reload(&after);
        let txn = ledger.find(GROCERY).unwrap();
        assert_eq!(txn.payee.as_deref(), Some(r#"Corner "Shop" \ 1"#));
        assert_eq!(txn.narration, "Card payment");
        assert_eq!(txn.flag, '!');
    }

    #[test]
    fn set_payee_inserts_a_payee_before_the_narration() {
        let before = without_transfer_payee();
        let (after, outcomes) = replay_on(&before, &[payee(TRANSFER, None, "Transfer")]);
        assert_eq!(outcomes, vec![Applied]);
        assert_eq!(after.text(RUN_FILE).unwrap(), RUN);
        assert_eq!(
            reload(&after).find(TRANSFER).unwrap().payee.as_deref(),
            Some("Transfer")
        );
    }

    /// The snapshot with the transfer's header reduced to its narration.
    fn without_transfer_payee() -> Snapshot {
        let mut before = snapshot();
        before.insert(
            RUN_FILE,
            changed(RUN, "\"Transfer\" \"To card\"", "\"To card\""),
        );
        before
    }

    #[test]
    fn set_payee_works_on_a_txn_header() {
        let (after, outcomes) = replay_on(&snapshot(), &[payee(BAKERY, Some("Bakery"), "Baker")]);
        assert_eq!(outcomes, vec![Applied]);
        assert_eq!(
            after.text(RUN_FILE).unwrap(),
            changed(
                RUN,
                "txn \"Bakery\" \"Bread\" #food",
                "txn \"Baker\" \"Bread\" #food"
            )
        );
        let ledger = reload(&after);
        let txn = ledger.find(BAKERY).unwrap();
        assert_eq!((txn.payee.as_deref(), txn.flag), (Some("Baker"), '*'));
        assert_eq!(txn.tags, vec!["food"]);
    }

    #[test]
    fn set_payee_is_done_when_the_payee_is_already_new() {
        let intent = payee(TRANSFER, None, "Card Company");
        let (after, outcomes) =
            replay_on(&without_transfer_payee(), &[intent.clone(), intent.clone()]);
        assert_eq!(outcomes, vec![Applied, AlreadyApplied]);
        let (again, outcomes) = replay_on(&after, &[intent]);
        assert_eq!(outcomes, vec![AlreadyApplied]);
        assert_eq!(again, after);
    }

    #[test]
    fn set_payee_builds_on_earlier_intents() {
        let (after, outcomes) = replay_on(
            &without_transfer_payee(),
            &[
                payee(TRANSFER, None, "Card"),
                payee(TRANSFER, Some("Card"), "Card Company"),
                payee(TRANSFER, None, "Other"),
            ],
        );
        assert_eq!(
            outcomes,
            vec![
                Applied,
                Applied,
                Outcome::Conflict(Conflict {
                    code: ConflictCode::PayeeChanged,
                    current: Some("Card Company".into()),
                })
            ]
        );
        assert_eq!(
            after.text(RUN_FILE).unwrap(),
            RUN.replace("\"Transfer\"", "\"Card Company\"")
        );
    }

    #[test]
    fn set_payee_reports_the_current_payee_on_conflict() {
        let (after, outcomes) = replay_on(
            &without_transfer_payee(),
            &[
                payee(GROCERY, Some("Corner Shop"), "Grocer"),
                payee(GROCERY, None, "Grocer"),
                payee(TRANSFER, Some("Transfer"), "Card Company"),
            ],
        );
        let changed_from = |current: Option<&str>| {
            Outcome::Conflict(Conflict {
                code: ConflictCode::PayeeChanged,
                current: current.map(Into::into),
            })
        };
        assert_eq!(
            outcomes,
            vec![
                changed_from(Some("Corner Grocery")),
                changed_from(Some("Corner Grocery")),
                changed_from(None)
            ]
        );
        assert_eq!(after, without_transfer_payee());
    }

    #[test]
    fn set_payee_conflicts_on_dismissed_and_unknown_transactions() {
        let (after, outcomes) = replay_on(
            &snapshot(),
            &[
                payee(UTILITIES, Some("City Utilities"), "Utility"),
                payee("giro-000000000000", None, "Utility"),
                dismiss(GROCERY),
                payee(GROCERY, Some("Corner Grocery"), "Grocer"),
            ],
        );
        assert_eq!(
            outcomes,
            vec![
                conflict(ConflictCode::TransactionDismissed),
                conflict(ConflictCode::TransactionNotFound),
                Applied,
                conflict(ConflictCode::TransactionDismissed),
            ]
        );
        assert!(
            after
                .text(RUN_FILE)
                .unwrap()
                .contains("; 2026-10-18 ! \"Corner Grocery\"")
        );
    }

    #[test]
    fn set_payee_needs_strings_written_as_lentil_writes_them() {
        let cases = [
            // A payee with an escape the core does not write.
            (
                "\"Corner Grocery\" \"Card",
                "\"Corner\\tGrocery\" \"Card",
                payee(GROCERY, Some("Corner\tGrocery"), "Grocer"),
            ),
            // No narration to insert the payee before.
            (
                "\"Transfer\" \"To card\"",
                "",
                payee(TRANSFER, None, "Card"),
            ),
            // A narration with an escape the core does not write.
            (
                "\"Transfer\" \"To card\"",
                "\"To\\tcard\"",
                payee(TRANSFER, None, "Card"),
            ),
        ];
        for (from, to, intent) in cases {
            let mut before = snapshot();
            before.insert(RUN_FILE, changed(RUN, from, to));
            let (after, outcomes) = replay_on(&before, &[intent]);
            assert_eq!(outcomes, vec![conflict(ConflictCode::NotEditable)], "{to}");
            assert_eq!(after, before);
        }
    }

    #[test]
    fn dismiss_comments_out_every_line_of_the_transaction() {
        let (after, outcomes) = replay_on(&snapshot(), &[dismiss(GROCERY)]);
        assert_eq!(outcomes, vec![Applied]);
        let expected = r#"; 2026-10-18 ! "Corner Grocery" "Card payment"
;   lentil_id: "giro-3f9a1c2e7b4d"
;   lentil_counterparty: "CORNER GROCERY 4711"
;   lentil_purpose: "CARD PAYMENT"
;   Assets:Bank:Giro        -42.17 EUR
;   Expenses:Uncategorized"#;
        assert_eq!(
            after.text(RUN_FILE).unwrap(),
            changed(RUN, GROCERY_BLOCK, expected)
        );
        let ledger = reload(&after);
        assert!(ledger.find(GROCERY).is_none());
        assert!(ledger.dismissed_ids.contains(GROCERY));
        // The run's balance assertion now fails, which is reported, not hidden.
        assert_eq!(ledger.balance_failures.len(), 1);
    }

    #[test]
    fn dismiss_keeps_indented_comments_and_posting_metadata() {
        let (after, outcomes) = replay_on(&snapshot(), &[dismiss(BAKERY)]);
        assert_eq!(outcomes, vec![Applied]);
        let block = "2026-10-19 txn \"Bakery\" \"Bread\" #food\n  lentil_id: \"giro-aaaaaaaaaaaa\"\n  Assets:Bank:Giro        -3.10 EUR ; trailing\n    posting_meta: \"x\"\n  ! Expenses:Uncategorized   3.10 EUR\n  ; trailing comment in txn";
        assert_eq!(
            after.text(RUN_FILE).unwrap(),
            changed(RUN, block, &commented(block))
        );
        assert!(reload(&after).dismissed_ids.contains(BAKERY));
    }

    #[test]
    fn dismiss_is_done_on_a_dismissed_transaction() {
        let (after, outcomes) = replay_on(
            &snapshot(),
            &[
                dismiss(UTILITIES),
                dismiss("giro-000000000000"),
                dismiss(GROCERY),
                dismiss(GROCERY),
            ],
        );
        assert_eq!(
            outcomes,
            vec![
                AlreadyApplied,
                conflict(ConflictCode::TransactionNotFound),
                Applied,
                AlreadyApplied
            ]
        );
        let (again, outcomes) = replay_on(&after, &[dismiss(GROCERY)]);
        assert_eq!(outcomes, vec![AlreadyApplied]);
        assert_eq!(again, after);
    }

    #[test]
    fn intents_after_a_dismiss_see_a_dismissed_transaction() {
        let (_, outcomes) = replay_on(
            &snapshot(),
            &[
                dismiss(GROCERY),
                confirm(GROCERY),
                set(GROCERY, 1, "Expenses:Uncategorized", "Expenses:Late"),
                dismiss(GROCERY),
            ],
        );
        let dismissed = conflict(ConflictCode::TransactionDismissed);
        assert_eq!(
            outcomes,
            vec![Applied, dismissed.clone(), dismissed, AlreadyApplied]
        );
    }

    #[test]
    fn edits_before_a_dismiss_stay_in_the_comment() {
        let (after, outcomes) = replay_on(
            &snapshot(),
            &[
                confirm(GROCERY),
                set(
                    GROCERY,
                    1,
                    "Expenses:Uncategorized",
                    "Expenses:Food:Groceries",
                ),
                payee(GROCERY, Some("Corner Grocery"), "Grocer"),
                dismiss(GROCERY),
            ],
        );
        assert_eq!(outcomes, vec![Applied, Applied, Applied, Applied]);
        let edited = GROCERY_BLOCK
            .replace("18 ! ", "18 * ")
            .replace("\"Corner Grocery\"", "\"Grocer\"")
            .replace("Expenses:Uncategorized", "Expenses:Food:Groceries");
        assert_eq!(
            after.text(RUN_FILE).unwrap(),
            changed(RUN, GROCERY_BLOCK, &commented(&edited))
        );
        assert!(reload(&after).dismissed_ids.contains(GROCERY));
    }

    #[test]
    fn edits_land_in_the_right_place_across_transactions_and_files() {
        let mut before = snapshot();
        before.insert(CARD_FILE, CARD_RUN);
        let (after, outcomes) = replay_on(
            &before,
            &[
                set(TRANSFER, 1, "Expenses:Uncategorized", "Assets:Transfer"),
                confirm(CAFE),
                payee(TRANSFER, Some("Transfer"), "Card Company"),
                confirm(TRANSFER),
                set(CAFE, 1, "Expenses:Uncategorized", "Expenses:Food:Groceries"),
                payee(CAFE, Some("Café"), "Café Ünïcode"),
                confirm(GROCERY),
                dismiss(CAFE),
                set(GROCERY, 1, "Expenses:Uncategorized", "Expenses:Late"),
                payee(BAKERY, Some("Bakery"), "Bäckerei"),
            ],
        );
        assert_eq!(
            outcomes,
            vec![
                Applied,
                Applied,
                Applied,
                Applied,
                Applied,
                Applied,
                Applied,
                Applied,
                conflict(ConflictCode::AccountNotOpen),
                Applied,
            ]
        );
        let run = changed(RUN, "2026-10-18 ! ", "2026-10-18 * ");
        let run = changed(&run, "txn \"Bakery\"", "txn \"Bäckerei\"");
        let run = changed(
            &run,
            "2026-10-20 ! \"Transfer\"",
            "2026-10-20 * \"Card Company\"",
        );
        let run = changed(
            &run,
            "  Expenses:Uncategorized\n\n2026-10-21",
            "  Assets:Transfer\n\n2026-10-21",
        );
        assert_eq!(after.text(RUN_FILE).unwrap(), run);
        let card = CARD_RUN
            .replace(" ! \"Café\"", " * \"Café Ünïcode\"")
            .replace("Expenses:Uncategorized", "Expenses:Food:Groceries");
        assert_eq!(after.text(CARD_FILE).unwrap(), commented(&card));
        let ledger = reload(&after);
        assert!(ledger.dismissed_ids.contains(CAFE));
        assert_eq!(ledger.find(TRANSFER).unwrap().flag, '*');
    }

    #[test]
    fn crlf_files_stay_crlf() {
        let crlf = |s: &Snapshot| {
            Snapshot::from_files(s.files().map(|(p, c)| {
                (
                    p,
                    String::from_utf8(c.to_vec()).unwrap().replace('\n', "\r\n"),
                )
            }))
        };
        let intents = [
            confirm(GROCERY),
            set(TRANSFER, 1, "Expenses:Uncategorized", "Expenses:Late"),
            payee(TRANSFER, Some("Transfer"), "Card Company"),
            dismiss(BAKERY),
            open("Expenses:Fresh", "2026-01-01"),
        ];
        let (lf_after, lf_outcomes) = replay_on(&snapshot(), &intents);
        let (crlf_after, crlf_outcomes) = replay_on(&crlf(&snapshot()), &intents);
        assert_eq!(crlf_outcomes, lf_outcomes);
        assert_eq!(crlf_outcomes, vec![Applied; 5]);
        assert_eq!(crlf_after, crlf(&lf_after));
        let ledger = reload(&crlf_after);
        assert!(ledger.dismissed_ids.contains(BAKERY));
        assert!(ledger.is_open_on("Expenses:Fresh", date("2026-01-01")));
    }

    #[test]
    fn open_account_appends_to_accounts_beancount() {
        let (after, outcomes) = replay_on(
            &snapshot(),
            &[
                open("Expenses:Fresh", "2026-10-01"),
                open("Expenses:Fresh", "2026-01-01"),
                open("Expenses:Food:Groceries", "2026-01-01"),
                open("Expenses:Old", "2026-01-01"),
                open("Income:Gifts", "2026-10-02"),
            ],
        );
        assert_eq!(
            outcomes,
            vec![
                Applied,
                AlreadyApplied,
                AlreadyApplied,
                AlreadyApplied,
                Applied
            ]
        );
        assert_eq!(
            after.text("accounts.beancount").unwrap(),
            format!("{ACCOUNTS}2026-10-01 open Expenses:Fresh\n2026-10-02 open Income:Gifts\n")
        );
        assert_eq!(after.text(RUN_FILE), Some(RUN));
        let ledger = reload(&after);
        assert!(ledger.is_open_on("Expenses:Fresh", date("2026-10-01")));
        let (again, outcomes) = replay_on(&after, &[open("Expenses:Fresh", "2026-10-01")]);
        assert_eq!(outcomes, vec![AlreadyApplied]);
        assert_eq!(again, after);
    }

    #[test]
    fn open_account_adds_a_missing_final_line_break() {
        let mut before = snapshot();
        before.insert("accounts.beancount", ACCOUNTS.trim_end());
        let (after, outcomes) = replay_on(&before, &[open("Expenses:Fresh", "2026-10-01")]);
        assert_eq!(outcomes, vec![Applied]);
        assert_eq!(
            after.text("accounts.beancount").unwrap(),
            format!("{ACCOUNTS}2026-10-01 open Expenses:Fresh\n")
        );
    }

    #[test]
    fn open_account_rejects_invalid_names() {
        for name in [
            "Expenses",
            "Foo:Bar",
            "expenses:food",
            "Expenses:food",
            "Expenses:Bad Name",
            "Expenses:X\n2020-01-01 open Assets:Y",
            "",
        ] {
            let (after, outcomes) = replay_on(&snapshot(), &[open(name, "2026-01-01")]);
            assert_eq!(
                outcomes,
                vec![conflict(ConflictCode::InvalidAccount)],
                "{name:?}"
            );
            assert_eq!(after, snapshot());
        }
        let (_, outcomes) = replay_on(&snapshot(), &[open("Equity:Ünïcode:X1", "2026-01-01")]);
        assert_eq!(outcomes, vec![Applied]);
    }

    #[test]
    fn open_account_creates_a_missing_accounts_file() {
        let mut before = snapshot();
        before.remove("accounts.beancount");
        let (after, outcomes) = replay_on(&before, &[open("Expenses:Fresh", "2026-10-01")]);
        assert_eq!(outcomes, vec![Applied]);
        assert_eq!(
            after.text("accounts.beancount"),
            Some("2026-10-01 open Expenses:Fresh\n")
        );
    }

    #[test]
    fn open_account_leaves_a_non_utf8_accounts_file_alone() {
        let mut before = snapshot();
        before.insert("accounts.beancount", b"\xff\n".to_vec());
        let (after, outcomes) = replay_on(&before, &[open("Expenses:Fresh", "2026-10-01")]);
        assert_eq!(outcomes, vec![conflict(ConflictCode::NotEditable)]);
        assert_eq!(after, before);
    }

    #[test]
    fn add_import_run_writes_only_new_files() {
        let file = "imports/giro/2026/20261101T000000Z.beancount";
        let content = "2026-10-30 ! \"New\" \"x\"\n  lentil_id: \"giro-dddddddddddd\"\n  lentil_counterparty: \"NEW\"\n  Assets:Bank:Giro  -1.00 EUR\n  Expenses:Uncategorized\n";
        let (after, outcomes) = replay_on(
            &snapshot(),
            &[
                run_file(file, content),
                run_file(file, content),
                run_file(file, "other"),
                run_file(RUN_FILE, RUN),
                run_file(RUN_FILE, "other"),
            ],
        );
        let exists = conflict(ConflictCode::FileExists);
        assert_eq!(
            outcomes,
            vec![
                Applied,
                AlreadyApplied,
                exists.clone(),
                AlreadyApplied,
                exists
            ]
        );
        assert_eq!(after.text(file), Some(content));
        assert_eq!(after.text(RUN_FILE), Some(RUN));
        assert_eq!(after.len(), snapshot().len() + 1);
        assert!(reload(&after).find("giro-dddddddddddd").is_some());
        let (again, outcomes) = replay_on(&after, &[run_file(file, content)]);
        assert_eq!(outcomes, vec![AlreadyApplied]);
        assert_eq!(again, after);
    }

    #[test]
    fn add_import_run_does_not_overwrite_a_new_accounts_file() {
        let mut before = snapshot();
        before.remove("accounts.beancount");
        let (after, outcomes) = replay_on(
            &before,
            &[
                open("Expenses:Fresh", "2026-10-01"),
                run_file("accounts.beancount", "x\n"),
            ],
        );
        assert_eq!(outcomes, vec![Applied, conflict(ConflictCode::FileExists)]);
        assert_eq!(
            after.text("accounts.beancount"),
            Some("2026-10-01 open Expenses:Fresh\n")
        );
    }

    #[test]
    fn unchanged_replay_keeps_the_snapshot() {
        let (after, outcomes) = replay_on(&snapshot(), &[]);
        assert!(outcomes.is_empty());
        assert_eq!(after, snapshot());
        let (after, _) = replay_on(&snapshot(), &[confirm(BAKERY), dismiss(UTILITIES)]);
        assert_eq!(after, snapshot());
    }

    #[test]
    fn unexpected_header_forms_are_not_editable() {
        let before = snapshot();
        let mut ledger = ledger::load(&before, "main.beancount");
        let grocery = ledger
            .transactions
            .iter()
            .position(|t| t.id.as_deref() == Some(GROCERY))
            .unwrap();
        let transfer = ledger
            .transactions
            .iter()
            .position(|t| t.id.as_deref() == Some(TRANSFER))
            .unwrap();
        let bakery = ledger
            .transactions
            .iter()
            .position(|t| t.id.as_deref() == Some(BAKERY))
            .unwrap();
        ledger.transactions[grocery].flag_offset = None;
        ledger.transactions[grocery].payee_span = None;
        let span = &ledger.transactions[transfer].postings[1].account_span;
        ledger.transactions[transfer].postings[1].account_span = span.start..span.start;
        ledger.transactions[transfer].span.start += 1;
        let header = ledger.transactions[bakery].narration_span.clone().unwrap();
        ledger.transactions[bakery].payee_span = Some(header);
        let (after, outcomes) = replay_with(
            &before,
            &ledger,
            &[
                confirm(GROCERY),
                set(TRANSFER, 1, "Expenses:Uncategorized", "Expenses:Late"),
                dismiss(TRANSFER),
                payee(GROCERY, Some("Corner Grocery"), "Grocer"),
                payee(BAKERY, Some("Bakery"), "Baker"),
            ],
        );
        assert_eq!(outcomes, vec![conflict(ConflictCode::NotEditable); 5]);
        assert_eq!(after, before);
    }

    #[test]
    fn a_failing_edit_keeps_the_bytes_and_becomes_a_conflict() {
        // Overlapping spans never come from the parser; if they did, the later
        // transaction keeps its bytes and its intents conflict instead of
        // being reported as applied.
        let before = snapshot();
        let mut ledger = ledger::load(&before, "main.beancount");
        let grocery = ledger.find(GROCERY).unwrap().span.clone();
        let transfer = ledger
            .transactions
            .iter_mut()
            .find(|t| t.id.as_deref() == Some(TRANSFER))
            .unwrap();
        transfer.span.start = grocery.end - 1;
        let (after, outcomes) = replay_with(
            &before,
            &ledger,
            &[confirm(GROCERY), confirm(TRANSFER), confirm(TRANSFER)],
        );
        let not_editable = conflict(ConflictCode::NotEditable);
        assert_eq!(outcomes, vec![Applied, not_editable.clone(), not_editable]);
        assert_eq!(
            after.text(RUN_FILE).unwrap(),
            changed(RUN, "2026-10-18 ! ", "2026-10-18 * ")
        );
    }

    #[test]
    fn set_payee_needs_its_string_inside_the_transaction() {
        // Spans that end inside the header never come from the parser; the
        // payee edit conflicts while the flag before it can still change.
        for (before, id, old) in [
            (snapshot(), GROCERY, Some("Corner Grocery")),
            (without_transfer_payee(), TRANSFER, None),
        ] {
            let mut ledger = ledger::load(&before, "main.beancount");
            let txn = ledger
                .transactions
                .iter_mut()
                .find(|t| t.id.as_deref() == Some(id))
                .unwrap();
            let string = txn
                .payee_span
                .clone()
                .or(txn.narration_span.clone())
                .unwrap();
            txn.span.end = string.end - 1;
            let (after, outcomes) =
                replay_with(&before, &ledger, &[payee(id, old, "Card"), confirm(id)]);
            assert_eq!(outcomes, vec![conflict(ConflictCode::NotEditable), Applied]);
            let text = before.text(RUN_FILE).unwrap();
            let flag = ledger.find(id).unwrap().flag_offset.unwrap();
            let mut expected = text.to_owned();
            expected.replace_range(flag..=flag, "*");
            assert_eq!(after.text(RUN_FILE).unwrap(), expected);
        }
    }

    #[test]
    fn set_payee_without_a_payee_needs_the_narration_string() {
        let before = without_transfer_payee();
        let mut ledger = ledger::load(&before, "main.beancount");
        let transfer = ledger
            .transactions
            .iter_mut()
            .find(|t| t.id.as_deref() == Some(TRANSFER))
            .unwrap();
        transfer.narration_span = None;
        let (after, outcomes) = replay_with(&before, &ledger, &[payee(TRANSFER, None, "Card")]);
        assert_eq!(outcomes, vec![conflict(ConflictCode::NotEditable)]);
        assert_eq!(after, before);
    }

    #[test]
    fn render_fails_without_a_narration_position() {
        let before = without_transfer_payee();
        let ledger = ledger::load(&before, "main.beancount");
        let mut txn = ledger.find(TRANSFER).unwrap().clone();
        let text = before.text(RUN_FILE).unwrap();
        let mut state = TxnState::new(&txn);
        state.payee = Some("Card".into());
        assert!(render(text, &txn, &state).unwrap().is_some());
        txn.narration_span = None;
        assert!(render(text, &txn, &state).is_err());
    }

    #[test]
    fn render_fails_without_a_flag_position() {
        let ledger = ledger::load(&snapshot(), "main.beancount");
        let mut txn = ledger.find(GROCERY).unwrap().clone();
        txn.flag_offset = None;
        let mut state = TxnState::new(&txn);
        state.flag = REVIEWED;
        assert!(render(RUN, &txn, &state).is_err());
        let state = TxnState::new(&txn);
        assert_eq!(render(RUN, &txn, &state), Ok(None));
    }

    #[test]
    fn summary_counts_applied_intents_in_spec_order() {
        let intents = pending(&[
            dismiss(GROCERY),
            open("Expenses:Fresh", "2026-01-01"),
            confirm(BAKERY),
            confirm(TRANSFER),
            set(TRANSFER, 1, "a", "b"),
            set(TRANSFER, 1, "Expenses:Uncategorized", "Expenses:Late"),
            confirm(CAFE),
            run_file("x", "y"),
            payee(GROCERY, None, "Grocer"),
        ]);
        let outcomes = vec![
            ("k0".to_owned(), Applied),
            ("k1".to_owned(), Applied),
            ("k8".to_owned(), Applied),
            ("k2".to_owned(), AlreadyApplied),
            ("k3".to_owned(), Applied),
            ("k4".to_owned(), conflict(ConflictCode::AccountChanged)),
            ("k5".to_owned(), Applied),
            ("k6".to_owned(), Applied),
            ("unknown".to_owned(), Applied),
        ];
        assert_eq!(
            summary(&intents, &outcomes),
            "confirm 2, set-account 1, set-payee 1, dismiss 1, open-account 1"
        );
        let outcomes = vec![("k7".to_owned(), Applied)];
        assert_eq!(summary(&intents, &outcomes), "add-import-run 1");
    }

    #[test]
    fn summary_is_empty_when_nothing_was_applied() {
        let intents = pending(&[confirm(BAKERY), confirm("giro-000000000000")]);
        let (_, outcomes) = replay_on(&snapshot(), &[confirm(BAKERY), confirm("x")]);
        let outcomes: Vec<_> = outcomes
            .into_iter()
            .enumerate()
            .map(|(i, o)| (format!("k{i}"), o))
            .collect();
        assert_eq!(summary(&intents, &outcomes), "");
        assert_eq!(summary(&[], &[]), "");
    }

    fn add_tag(id: &str, tag: &str) -> Intent {
        Intent::AddTag {
            id: id.into(),
            tag: tag.into(),
        }
    }

    fn remove_tag(id: &str, tag: &str) -> Intent {
        Intent::RemoveTag {
            id: id.into(),
            tag: tag.into(),
        }
    }

    const GROCERY_HEADER: &str = "2026-10-18 ! \"Corner Grocery\" \"Card payment\"\n";

    /// [`RUN`] with the grocery header line replaced by `header`, which may
    /// add tag lines after it.
    fn grocery_run(header: &str) -> String {
        changed(RUN, GROCERY_HEADER, header)
    }

    fn with_run(run: &str) -> Snapshot {
        Snapshot::from_files([
            ("main.beancount", MAIN),
            ("accounts.beancount", ACCOUNTS),
            (RUN_FILE, run),
        ])
    }

    /// Replays `intents` on `run`, checks the outcomes and that the result
    /// reloads, and returns the new run file.
    fn tag_replay(run: &str, intents: &[Intent], expected: &[Outcome]) -> String {
        let (after, outcomes) = replay_on(&with_run(run), intents);
        assert_eq!(outcomes, expected);
        reload(&after);
        after.text(RUN_FILE).unwrap().to_owned()
    }

    #[test]
    fn add_tag_inserts_into_the_header_and_keeps_the_flag() {
        let run = grocery_run("2026-10-18 ! \"Corner Grocery\" \"Card payment\" ; note\n");
        let intents = [add_tag(GROCERY, "recurring")];
        let result = replay(&with_run(&run), &config(), &pending(&intents));
        assert_eq!(result.outcomes, vec![("k0".to_owned(), Applied)]);
        assert_eq!(
            result.snapshot.text(RUN_FILE).unwrap(),
            changed(
                &run,
                "\"Card payment\" ; note",
                "\"Card payment\" #recurring ; note"
            )
        );
        let grocery = reload(&result.snapshot).find(GROCERY).unwrap().clone();
        assert_eq!(
            (grocery.flag, grocery.tags),
            ('!', vec!["recurring".into()])
        );
        assert_eq!(summary(&pending(&intents), &result.outcomes), "add-tag 1");
    }

    #[test]
    fn tag_intents_in_order() {
        let run = grocery_run("2026-10-18 ! \"Corner Grocery\" \"Card payment\" #c\n");
        let intents = [
            add_tag(GROCERY, "a"),
            add_tag(GROCERY, "b"),
            remove_tag(GROCERY, "c"),
        ];
        let result = replay(&with_run(&run), &config(), &pending(&intents));
        assert_eq!(
            result.snapshot.text(RUN_FILE).unwrap(),
            changed(&run, "\"Card payment\" #c", "\"Card payment\" #a #b")
        );
        assert_eq!(
            summary(&pending(&intents), &result.outcomes),
            "add-tag 2, remove-tag 1"
        );
    }

    #[test]
    fn add_tag_is_done_when_present_anywhere() {
        for header in [
            "2026-10-18 ! \"Corner Grocery\" \"Card payment\" #recurring\n",
            "2026-10-18 ! \"Corner Grocery\" \"Card payment\"\n  #recurring\n",
        ] {
            let run = grocery_run(header);
            let after = tag_replay(&run, &[add_tag(GROCERY, "recurring")], &[AlreadyApplied]);
            assert_eq!(after, run, "{header}");
        }
    }

    #[test]
    fn remove_tag_deletes_it_where_it_stands() {
        let run =
            grocery_run("2026-10-18 ! \"Corner Grocery\" \"Card payment\" #a #b\n  #c\n  #d ^l\n");
        assert_eq!(
            tag_replay(&run, &[remove_tag(GROCERY, "a")], &[Applied]),
            changed(&run, "\"Card payment\" #a #b", "\"Card payment\" #b")
        );
        assert_eq!(
            tag_replay(&run, &[remove_tag(GROCERY, "c")], &[Applied]),
            changed(&run, "  #c\n", "")
        );
        assert_eq!(
            tag_replay(&run, &[remove_tag(GROCERY, "d")], &[Applied]),
            changed(&run, "  #d ^l\n", "  ^l\n")
        );
        assert_eq!(
            tag_replay(RUN, &[remove_tag(BAKERY, "food")], &[Applied]),
            changed(RUN, "\"Bread\" #food", "\"Bread\"")
        );
        assert_eq!(
            tag_replay(&run, &[remove_tag(GROCERY, "x")], &[AlreadyApplied]),
            run
        );
    }

    #[test]
    fn remove_tag_removes_every_occurrence() {
        let run = grocery_run("2026-10-18 ! \"Corner Grocery\" \"Card payment\" #a #a\n  #a ^l\n");
        let after = tag_replay(&run, &[remove_tag(GROCERY, "a")], &[Applied]);
        assert_eq!(
            after,
            changed(
                &run,
                "\"Card payment\" #a #a\n  #a ^l\n",
                "\"Card payment\"\n  ^l\n"
            )
        );
        assert!(!after.contains("#a"));
    }

    #[test]
    fn tag_intents_conflict_on_missing_or_dismissed_transactions() {
        for intent in [add_tag, remove_tag] {
            let (after, outcomes) = replay_on(
                &snapshot(),
                &[
                    intent(UTILITIES, "food"),
                    intent("giro-000000000000", "food"),
                ],
            );
            assert_eq!(
                outcomes,
                vec![
                    conflict(ConflictCode::TransactionDismissed),
                    conflict(ConflictCode::TransactionNotFound)
                ]
            );
            assert_eq!(after, snapshot());
        }
    }

    #[test]
    fn add_tag_rejects_an_invalid_tag() {
        for id in [GROCERY, "giro-000000000000"] {
            let (after, outcomes) = replay_on(&snapshot(), &[add_tag(id, "a b")]);
            assert_eq!(outcomes, vec![conflict(ConflictCode::InvalidTag)]);
            assert_eq!(after, snapshot());
        }
    }

    #[test]
    fn tags_that_cannot_be_located_are_not_editable() {
        let run = changed(
            RUN,
            GROCERY_BLOCK,
            &format!("pushtag #trip\n{GROCERY_BLOCK}\npoptag #trip"),
        );
        let (after, outcomes) = replay_on(
            &with_run(&run),
            &[add_tag(GROCERY, "x"), remove_tag(GROCERY, "trip")],
        );
        assert_eq!(
            outcomes,
            vec![
                conflict(ConflictCode::NotEditable),
                conflict(ConflictCode::NotEditable)
            ]
        );
        assert_eq!(after, with_run(&run));

        // A tag edit that cannot be made never takes another edit of the
        // same transaction down with it.
        let (after, outcomes) =
            replay_on(&with_run(&run), &[add_tag(GROCERY, "x"), confirm(GROCERY)]);
        assert_eq!(outcomes, vec![conflict(ConflictCode::NotEditable), Applied]);
        assert_eq!(reload(&after).find(GROCERY).unwrap().flag, '*');
    }

    #[test]
    fn tags_whose_positions_lie_outside_the_transaction_are_not_editable() {
        let snapshot = snapshot();
        let mut ledger = reload(&snapshot);
        let grocery = ledger
            .transactions
            .iter_mut()
            .find(|t| t.id.as_deref() == Some(GROCERY))
            .unwrap();
        grocery.tag_spans.as_mut().unwrap().insert_at = grocery.span.end + 5;
        let (after, outcomes) = replay_with(
            &snapshot,
            &ledger,
            &[add_tag(GROCERY, "x"), confirm(GROCERY)],
        );
        assert_eq!(outcomes, vec![conflict(ConflictCode::NotEditable), Applied]);
        assert_eq!(
            reload(&after).find(GROCERY).unwrap().tags,
            Vec::<String>::new()
        );
    }

    #[test]
    fn tag_intents_serialize_with_their_op_names() {
        for (intent, op) in [
            (add_tag(GROCERY, "recurring"), "\"op\":\"add-tag\""),
            (remove_tag(GROCERY, "recurring"), "\"op\":\"remove-tag\""),
        ] {
            let json = serde_json::to_string(&intent).unwrap();
            assert!(json.contains(op), "{json}");
            assert!(json.contains("\"tag\":\"recurring\""), "{json}");
            assert_eq!(serde_json::from_str::<Intent>(&json).unwrap(), intent);
        }
    }

    #[test]
    fn summary_matches_a_replay() {
        let intents = pending(&[
            confirm(GROCERY),
            confirm(TRANSFER),
            confirm(BAKERY),
            dismiss(UTILITIES),
            set(TRANSFER, 1, "Expenses:Uncategorized", "Expenses:Late"),
        ]);
        let result = replay(&snapshot(), &config(), &intents);
        assert_eq!(
            summary(&intents, &result.outcomes),
            "confirm 2, set-account 1"
        );
    }
}

/// Property-based tests: random ledgers in varied formatting, and random
/// intent sequences as a client would record them.
#[cfg(test)]
mod props {
    use std::cmp::Reverse;
    use std::collections::BTreeSet;

    use proptest::prelude::*;

    use super::*;

    const CONFIG: &str = r#"
spec_version = 0
main_file = "main.beancount"
fallback_account = "Expenses:Uncategorized"
unattributed_account = "Expenses:Unattributed"

[[source]]
id = "giro"
account = "Assets:Bank:Giro"
currency = "EUR"
importer = "csv"
mapping = "sources/giro.toml"
"#;

    const MAIN: &str = "option \"title\" \"Ledger\"\noption \"operating_currency\" \"EUR\"\n\ninclude \"accounts.beancount\"\ninclude \"imports/*/*/*.beancount\"\n";
    const SOURCE: &str = "Assets:Bank:Giro";
    const COUNTERS: [&str; 4] = [
        "Expenses:Uncategorized",
        "Expenses:Food",
        "Expenses:Rent",
        "Income:Salary",
    ];
    const PAYEES: [&str; 3] = ["Corner Grocery", "Café Ünïcode", "Shop"];
    /// Open accounts that `set-account` intents move postings to, each at most once.
    const POOL: usize = 48;
    const CLOSED: &str = "Expenses:Old";
    const EARLY: &str = "2025-01-01";
    const LATE: &str = "2027-01-01";

    #[derive(Clone, Debug)]
    struct GenTxn {
        has_id: bool,
        /// 0: `!`, 1: `*`, 2: `txn`.
        keyword: u8,
        /// 0: two spaces, 1: four spaces, 2: a tab.
        indent: u8,
        cents: u32,
        counter: usize,
        explicit: bool,
        comment: bool,
        posting_flag: bool,
        meta: bool,
        /// What follows the transaction: 0: a blank line, 1: two, 2: a comment.
        gap: u8,
        month: u8,
        day: u8,
        /// Index into [`PAYEES`], or no payee.
        payee: Option<usize>,
    }

    #[derive(Clone, Debug)]
    struct GenLedger {
        crlf: bool,
        files: Vec<Vec<GenTxn>>,
    }

    fn gen_txn() -> impl Strategy<Value = GenTxn> {
        (
            prop::bool::weighted(0.9),
            0..3_u8,
            0..3_u8,
            1..100_000_u32,
            0..COUNTERS.len(),
            any::<bool>(),
            any::<bool>(),
            any::<bool>(),
            any::<bool>(),
            0..3_u8,
            (1..=12_u8, 1..=28_u8),
            prop::option::of(0..PAYEES.len()),
        )
            .prop_map(
                |(
                    has_id,
                    keyword,
                    indent,
                    cents,
                    counter,
                    explicit,
                    comment,
                    posting_flag,
                    meta,
                    gap,
                    (month, day),
                    payee,
                )| GenTxn {
                    has_id,
                    keyword,
                    indent,
                    cents,
                    counter,
                    explicit,
                    comment,
                    posting_flag,
                    meta,
                    gap,
                    month,
                    day,
                    payee,
                },
            )
    }

    fn gen_ledger() -> impl Strategy<Value = GenLedger> {
        (
            any::<bool>(),
            prop::collection::vec(prop::collection::vec(gen_txn(), 1..7), 1..4),
        )
            .prop_map(|(crlf, files)| GenLedger { crlf, files })
    }

    /// An intent as a client records it, with the transaction picked by index.
    #[derive(Clone, Debug)]
    enum Op {
        Confirm(usize),
        Dismiss(usize),
        /// Move the counter posting to the next unused pool account.
        SetPool(usize),
        /// Move the counter posting to an account opened by an earlier intent.
        SetOpened(usize),
        /// Move the counter posting to a closed account.
        SetClosed(usize),
        /// Move the counter posting to an account opened after the transaction.
        SetLate(usize),
        /// An intent recorded against a value the posting never had.
        SetStale(usize),
        /// Try to move the source posting.
        SetSource(usize),
        /// Set the payee to a new one with quotes and a backslash.
        SetPayee(usize),
        /// A payee intent recorded against a payee the transaction never had.
        SetPayeeStale(usize),
        /// Open a new account, before (`true`) or after all transactions.
        Open(bool),
        /// Confirm an ID that does not exist.
        Unknown,
    }

    fn gen_op() -> impl Strategy<Value = Op> {
        prop_oneof![
            4 => any::<usize>().prop_map(Op::Confirm),
            2 => any::<usize>().prop_map(Op::Dismiss),
            4 => any::<usize>().prop_map(Op::SetPool),
            2 => any::<usize>().prop_map(Op::SetOpened),
            1 => any::<usize>().prop_map(Op::SetClosed),
            1 => any::<usize>().prop_map(Op::SetLate),
            1 => any::<usize>().prop_map(Op::SetStale),
            1 => any::<usize>().prop_map(Op::SetSource),
            3 => any::<usize>().prop_map(Op::SetPayee),
            1 => any::<usize>().prop_map(Op::SetPayeeStale),
            2 => any::<bool>().prop_map(Op::Open),
            1 => Just(Op::Unknown),
        ]
    }

    /// A rendered ledger and the initial state of its addressable transactions.
    struct Model {
        snapshot: Snapshot,
        eol: &'static str,
        run_files: Vec<String>,
        ids: Vec<String>,
        flags: Vec<char>,
        counters: Vec<String>,
        payees: Vec<Option<String>>,
    }

    fn render_ledger(ledger: &GenLedger) -> Model {
        let eol = if ledger.crlf { "\r\n" } else { "\n" };
        let fix = |s: String| {
            if ledger.crlf {
                s.replace('\n', "\r\n")
            } else {
                s
            }
        };
        let mut accounts = format!("2020-01-01 open {SOURCE} EUR\n");
        for account in COUNTERS {
            accounts.push_str(&format!("2020-01-01 open {account}\n"));
        }
        for n in 0..POOL {
            accounts.push_str(&format!("2020-01-01 open Expenses:Pool:P{n}\n"));
        }
        accounts.push_str(&format!(
            "2020-01-01 open {CLOSED}\n2021-01-01 close {CLOSED}\n"
        ));

        let mut model = Model {
            snapshot: Snapshot::from_files([
                ("main.beancount".to_owned(), fix(MAIN.to_owned())),
                ("accounts.beancount".to_owned(), fix(accounts)),
            ]),
            eol,
            run_files: Vec::new(),
            ids: Vec::new(),
            flags: Vec::new(),
            counters: Vec::new(),
            payees: Vec::new(),
        };
        let mut n = 0;
        for (f, txns) in ledger.files.iter().enumerate() {
            let mut text = String::from("; lentil import run\n\n");
            for t in txns {
                n += 1;
                let ind = ["  ", "    ", "\t"][usize::from(t.indent)];
                let date = format!("2026-{:02}-{:02}", t.month, t.day);
                let payee = t.payee.map(|p| PAYEES[p]);
                let strings = match payee {
                    Some(payee) => format!("\"{payee}\" \"Narration {n}\""),
                    None => format!("\"Narration {n}\""),
                };
                let flag = match t.keyword {
                    0 => "!",
                    1 => "*",
                    _ => "txn",
                };
                let amount = format!("{}.{:02}", t.cents / 100, t.cents % 100);
                let counter = COUNTERS[t.counter];
                text.push_str(&format!("{date} {flag} {strings}\n"));
                if t.has_id {
                    let id = format!("giro-{n:012x}");
                    text.push_str(&format!("{ind}lentil_id: \"{id}\"\n"));
                    model.ids.push(id);
                    model.flags.push(if t.keyword == 0 { '!' } else { '*' });
                    model.counters.push(counter.to_owned());
                    model.payees.push(payee.map(ToOwned::to_owned));
                }
                text.push_str(&format!("{ind}lentil_counterparty: \"COUNTERPARTY {n}\"\n"));
                if t.meta {
                    text.push_str(&format!("{ind}note: \"m{n}\"\n"));
                }
                let comment = if t.comment { " ; source" } else { "" };
                text.push_str(&format!("{ind}{SOURCE}  -{amount} EUR{comment}\n"));
                let posting_flag = if t.posting_flag { "! " } else { "" };
                let units = if t.explicit {
                    format!("   {amount} EUR")
                } else {
                    String::new()
                };
                text.push_str(&format!("{ind}{posting_flag}{counter}{units}\n"));
                text.push_str(["\n", "\n\n", "\n; between\n\n"][usize::from(t.gap)]);
            }
            let path = format!("imports/giro/2026/2026010{f}T000000Z.beancount");
            model.snapshot.insert(path.clone(), fix(text));
            model.run_files.push(path);
        }
        model
    }

    /// What a client expects: the intents, their outcomes on the state they
    /// were recorded on, and the resulting state.
    struct Plan {
        intents: Vec<PendingIntent>,
        outcomes: Vec<Outcome>,
        flags: Vec<char>,
        counters: Vec<String>,
        payees: Vec<Option<String>>,
        dismissed: Vec<bool>,
        open_lines: Vec<String>,
    }

    fn plan(model: &Model, ops: &[Op]) -> Plan {
        let mut plan = Plan {
            intents: Vec::new(),
            outcomes: Vec::new(),
            flags: model.flags.clone(),
            counters: model.counters.clone(),
            payees: model.payees.clone(),
            dismissed: vec![false; model.ids.len()],
            open_lines: Vec::new(),
        };
        let mut pool = (0..POOL).map(|n| format!("Expenses:Pool:P{n}"));
        let mut early: Vec<String> = Vec::new();
        let mut late: Vec<String> = Vec::new();
        let mut opened = 0;
        let mut renamed = 0;
        let dismissed = conflict(ConflictCode::TransactionDismissed);
        for op in ops {
            let count = model.ids.len();
            let pick = |i: &usize| (count > 0).then(|| i % count);
            let (intent, outcome) = match op {
                Op::Open(is_early) => {
                    opened += 1;
                    let account = format!("Expenses:New:N{opened}");
                    let day = if *is_early { EARLY } else { LATE };
                    plan.open_lines.push(format!("{day} open {account}"));
                    if *is_early {
                        early.push(account.clone());
                    } else {
                        late.push(account.clone());
                    }
                    let intent = Intent::OpenAccount {
                        account,
                        date: day.parse().unwrap(),
                    };
                    (intent, Outcome::Applied)
                }
                Op::Unknown => (
                    Intent::Confirm {
                        id: "giro-ffffffffffff".into(),
                    },
                    conflict(ConflictCode::TransactionNotFound),
                ),
                Op::Confirm(i) => {
                    let Some(k) = pick(i) else { continue };
                    let outcome = if plan.dismissed[k] {
                        dismissed.clone()
                    } else if plan.flags[k] == '*' {
                        Outcome::AlreadyApplied
                    } else {
                        plan.flags[k] = '*';
                        Outcome::Applied
                    };
                    let id = model.ids[k].clone();
                    (Intent::Confirm { id }, outcome)
                }
                Op::Dismiss(i) => {
                    let Some(k) = pick(i) else { continue };
                    let outcome = if plan.dismissed[k] {
                        Outcome::AlreadyApplied
                    } else {
                        plan.dismissed[k] = true;
                        Outcome::Applied
                    };
                    let id = model.ids[k].clone();
                    (Intent::Dismiss { id }, outcome)
                }
                Op::SetPool(i) | Op::SetOpened(i) => {
                    let Some(k) = pick(i) else { continue };
                    let new = if matches!(op, Op::SetPool(_)) {
                        pool.next()
                    } else {
                        early.pop()
                    };
                    let Some(new) = new else { continue };
                    let old = plan.counters[k].clone();
                    let outcome = if plan.dismissed[k] {
                        dismissed.clone()
                    } else {
                        plan.counters[k].clone_from(&new);
                        Outcome::Applied
                    };
                    let id = model.ids[k].clone();
                    let posting = 1;
                    (
                        Intent::SetAccount {
                            id,
                            posting,
                            old,
                            new,
                        },
                        outcome,
                    )
                }
                Op::SetClosed(i) | Op::SetLate(i) => {
                    let Some(k) = pick(i) else { continue };
                    let new = if matches!(op, Op::SetClosed(_)) {
                        CLOSED.to_owned()
                    } else {
                        let Some(account) = late.last() else { continue };
                        account.clone()
                    };
                    let outcome = if plan.dismissed[k] {
                        dismissed.clone()
                    } else {
                        conflict(ConflictCode::AccountNotOpen)
                    };
                    let id = model.ids[k].clone();
                    let old = plan.counters[k].clone();
                    let posting = 1;
                    (
                        Intent::SetAccount {
                            id,
                            posting,
                            old,
                            new,
                        },
                        outcome,
                    )
                }
                Op::SetStale(i) | Op::SetSource(i) => {
                    let Some(k) = pick(i) else { continue };
                    let Some(new) = pool.next() else { continue };
                    let stale = matches!(op, Op::SetStale(_));
                    let outcome = if plan.dismissed[k] {
                        dismissed.clone()
                    } else if stale {
                        Outcome::Conflict(Conflict {
                            code: ConflictCode::AccountChanged,
                            current: Some(plan.counters[k].clone()),
                        })
                    } else {
                        conflict(ConflictCode::SourcePosting)
                    };
                    let (posting, old) = if stale {
                        (1, "Expenses:Stale".to_owned())
                    } else {
                        (0, SOURCE.to_owned())
                    };
                    let id = model.ids[k].clone();
                    (
                        Intent::SetAccount {
                            id,
                            posting,
                            old,
                            new,
                        },
                        outcome,
                    )
                }
                Op::SetPayee(i) | Op::SetPayeeStale(i) => {
                    let Some(k) = pick(i) else { continue };
                    renamed += 1;
                    let new = format!("Payee \"{renamed}\" \\");
                    let stale = matches!(op, Op::SetPayeeStale(_));
                    let old = if stale {
                        Some("Stale".to_owned())
                    } else {
                        plan.payees[k].clone()
                    };
                    let outcome = if plan.dismissed[k] {
                        dismissed.clone()
                    } else if stale {
                        Outcome::Conflict(Conflict {
                            code: ConflictCode::PayeeChanged,
                            current: plan.payees[k].clone(),
                        })
                    } else {
                        plan.payees[k] = Some(new.clone());
                        Outcome::Applied
                    };
                    let id = model.ids[k].clone();
                    (Intent::SetPayee { id, old, new }, outcome)
                }
            };
            plan.intents.push(PendingIntent {
                key: format!("k{}", plan.intents.len()),
                intent,
            });
            plan.outcomes.push(outcome);
        }
        plan
    }

    /// The expected bytes of a run file, built independently from the
    /// original text: element bytes replaced at the parsed positions, dismissed
    /// transactions prefixed line by line, everything else copied.
    fn expected_run(text: &str, file: &str, ledger: &Ledger, model: &Model, plan: &Plan) -> String {
        let mut text = text.to_owned();
        let mut txns: Vec<&Txn> = ledger
            .transactions
            .iter()
            .filter(|t| t.file == file)
            .collect();
        txns.sort_by_key(|t| Reverse(t.span.start));
        for txn in txns {
            let Some(k) = txn
                .id
                .as_ref()
                .and_then(|id| model.ids.iter().position(|x| x == id))
            else {
                continue;
            };
            let base = txn.span.start;
            let mut block = text[txn.span.clone()].to_owned();
            if plan.counters[k] != model.counters[k] {
                let span = &txn.postings[1].account_span;
                block.replace_range(span.start - base..span.end - base, &plan.counters[k]);
            }
            if let Some(payee) = plan.payees[k]
                .as_ref()
                .filter(|_| plan.payees[k] != model.payees[k])
            {
                let quoted = format!("\"{}\"", payee.replace('\\', "\\\\").replace('"', "\\\""));
                match &txn.payee_span {
                    Some(span) => block.replace_range(span.start - base..span.end - base, &quoted),
                    None => {
                        let at = txn.narration_span.as_ref().unwrap().start - base;
                        block.insert_str(at, &format!("{quoted} "));
                    }
                }
            }
            if plan.flags[k] != model.flags[k] {
                let offset = txn.flag_offset.unwrap() - base;
                block.replace_range(offset..=offset, "*");
            }
            if plan.dismissed[k] {
                block = block
                    .split_inclusive('\n')
                    .map(|l| format!("; {l}"))
                    .collect();
            }
            text.replace_range(txn.span.clone(), &block);
        }
        text
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(96))]

        /// Replay matches the client's expectation byte for byte, reloads
        /// without errors, and a second replay changes nothing.
        #[test]
        fn replay_is_byte_precise_parse_clean_and_idempotent(
            generated in gen_ledger(),
            ops in prop::collection::vec(gen_op(), 0..24),
        ) {
            let config = Config::parse(CONFIG).unwrap();
            let model = render_ledger(&generated);
            let original = ledger::load(&model.snapshot, "main.beancount");
            prop_assert!(original.errors.is_empty(), "{:?}", original.errors);
            let plan = plan(&model, &ops);

            let first = replay(&model.snapshot, &config, &plan.intents);
            let outcomes: Vec<Outcome> = first.outcomes.iter().map(|(_, o)| o.clone()).collect();
            prop_assert_eq!(&outcomes, &plan.outcomes);

            // Bytes outside the edited elements stay identical.
            prop_assert_eq!(first.snapshot.len(), model.snapshot.len());
            for (path, content) in model.snapshot.files() {
                let text = std::str::from_utf8(content).unwrap();
                let expected = if model.run_files.iter().any(|f| f == path) {
                    expected_run(text, path, &original, &model, &plan)
                } else if path == ACCOUNTS_FILE {
                    plan.open_lines.iter().fold(text.to_owned(), |acc, line| acc + line + model.eol)
                } else {
                    text.to_owned()
                };
                prop_assert_eq!(first.snapshot.text(path).unwrap(), expected, "{}", path);
            }

            // The result parses and holds the expected state.
            let reloaded = ledger::load(&first.snapshot, "main.beancount");
            prop_assert!(reloaded.errors.is_empty(), "{:?}", reloaded.errors);
            prop_assert_eq!(reloaded.all_ids(), original.all_ids());
            for (k, id) in model.ids.iter().enumerate() {
                if plan.dismissed[k] {
                    prop_assert!(reloaded.find(id).is_none());
                    prop_assert!(reloaded.dismissed_ids.contains(id));
                } else {
                    let txn = reloaded.find(id).unwrap();
                    prop_assert_eq!(txn.flag, plan.flags[k]);
                    prop_assert_eq!(&txn.postings[1].account, &plan.counters[k]);
                    prop_assert_eq!(&txn.payee, &plan.payees[k]);
                }
            }

            // Replaying the same intents again changes nothing.
            let second = replay(&first.snapshot, &config, &plan.intents);
            for (key, outcome) in &second.outcomes {
                prop_assert!(*outcome != Outcome::Applied, "{} applied twice", key);
            }
            prop_assert_eq!(&second.snapshot, &first.snapshot);
        }
    }

    const TAGS: [&str; 4] = ["a", "b", "c", "trip-x"];
    const TAG_ID: &str = "giro-000000000001";

    /// Tags written somewhere in the transaction: a subset of [`TAGS`] and
    /// whether a link follows them.
    #[derive(Clone, Debug)]
    struct GenTagLine {
        tags: Vec<bool>,
        link: bool,
    }

    #[derive(Clone, Debug)]
    struct GenTagged {
        crlf: bool,
        /// Separate tags and links by a tab instead of a space, and indent
        /// tag lines by one.
        tabs: bool,
        header: GenTagLine,
        /// A tag line right after the header.
        line: Option<GenTagLine>,
        /// A tag line after the postings.
        late: Option<GenTagLine>,
        /// 0: no comment on the header, 1: ` ; note`, 2: `;note` right after
        /// the last token.
        comment: u8,
        /// Whether the file goes on after the transaction; otherwise it ends
        /// with the transaction's last line, without a line break.
        tail: bool,
        /// Add (`true`) or remove, and an index into [`TAGS`].
        ops: Vec<(bool, usize)>,
        /// Where a `set-payee` goes among the ops, if anywhere.
        payee: Option<usize>,
    }

    fn gen_tag_line() -> impl Strategy<Value = GenTagLine> {
        (prop::collection::vec(any::<bool>(), 4), any::<bool>())
            .prop_map(|(tags, link)| GenTagLine { tags, link })
    }

    fn gen_tagged() -> impl Strategy<Value = GenTagged> {
        (
            (any::<bool>(), any::<bool>(), gen_tag_line()),
            prop::option::of(gen_tag_line()),
            prop::option::of(gen_tag_line()),
            (0..3u8, any::<bool>()),
            prop::collection::vec((any::<bool>(), 0..TAGS.len()), 0..8),
            prop::option::of(0..9usize),
        )
            .prop_map(
                |((crlf, tabs, header), line, late, (comment, tail), ops, payee)| GenTagged {
                    crlf,
                    tabs,
                    header,
                    line,
                    late,
                    comment,
                    tail,
                    payee: payee.map(|p| p.min(ops.len())),
                    ops,
                },
            )
    }

    /// The tags and the link of `line`, each after `sep`.
    fn tag_tokens(line: &GenTagLine, sep: &str, link: &str) -> String {
        let mut text: String = TAGS
            .iter()
            .zip(&line.tags)
            .filter(|(_, on)| **on)
            .map(|(tag, _)| format!("{sep}#{tag}"))
            .collect();
        if line.link {
            text.push_str(&format!("{sep}^{link}"));
        }
        text
    }

    /// A tag line with the tokens of `line`, or `None` if it has none.
    fn tag_line(line: Option<&GenTagLine>, indent: &str, sep: &str, link: &str) -> Option<String> {
        let tokens = tag_tokens(line?, sep, link);
        let tokens = tokens.strip_prefix(sep)?;
        Some(format!("{indent}{tokens}"))
    }

    /// The run file of `g`, with one transaction after a head and, with
    /// [`GenTagged::tail`], before a tail.
    fn tagged_run(g: &GenTagged) -> String {
        let eol = if g.crlf { "\r\n" } else { "\n" };
        let (sep, indent) = if g.tabs { ("\t", "\t") } else { (" ", "  ") };
        let mut header = format!(
            "2026-10-18 ! \"Shop\" \"x\"{}",
            tag_tokens(&g.header, sep, "l1")
        );
        header.push_str(match g.comment {
            1 => " ; note",
            2 => ";note",
            _ => "",
        });
        let mut lines = vec!["; head".to_owned(), String::new(), header];
        lines.extend(tag_line(g.line.as_ref(), indent, sep, "l2"));
        lines.extend([
            format!("  lentil_id: \"{TAG_ID}\""),
            "  Assets:Bank:Giro  -1.00 EUR".to_owned(),
            "  Expenses:Uncategorized".to_owned(),
        ]);
        lines.extend(tag_line(g.late.as_ref(), indent, sep, "l3"));
        if g.tail {
            lines.extend([String::new(), "; tail".to_owned(), String::new()]);
        }
        lines.join(eol)
    }

    fn is_tag_line(line: &str) -> bool {
        line.trim_start().starts_with(['#', '^'])
    }

    /// The lines of a transaction's text other than its header and its tag
    /// and link lines, byte for byte apart from the line break, which a
    /// removed last line takes from the line before it.
    fn other_lines(block: &str) -> Vec<&str> {
        block.lines().skip(1).filter(|l| !is_tag_line(l)).collect()
    }

    /// Everything of a transaction's text but its tags: the header up to the
    /// narration byte for byte, then the links and other tokens of the header
    /// and of the tag lines in order, and the header's comment byte for byte.
    fn without_tags(block: &str) -> (String, Vec<String>, Option<String>) {
        let header = block.lines().next().unwrap();
        let at = header.find("\"x\"").unwrap() + 3;
        let (prefix, rest) = header.split_at(at);
        let (code, comment) = match rest.find(';') {
            Some(i) => (&rest[..i], Some(rest[i..].to_owned())),
            None => (rest, None),
        };
        let tokens = std::iter::once(code)
            .chain(block.lines().skip(1).filter(|l| is_tag_line(l)))
            .flat_map(str::split_whitespace)
            .filter(|t| !t.starts_with('#'))
            .map(str::to_owned)
            .collect();
        (prefix.to_owned(), tokens, comment)
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(512))]

        /// Tag intents, mixed with a `set-payee`, reach the intended tags in
        /// every position, separator, comment and line ending, change nothing
        /// but the tags and the payee, and are done on a second replay.
        #[test]
        fn tag_replay_reaches_the_intended_tags_and_is_idempotent(g in gen_tagged()) {
            let config = Config::parse(CONFIG).unwrap();
            let run = tagged_run(&g);
            let file = "imports/giro/2026/20261020T071502Z.beancount";
            let snapshot = Snapshot::from_files([
                ("main.beancount", MAIN),
                ("accounts.beancount", "2020-01-01 open Assets:Bank:Giro EUR\n2020-01-01 open Expenses:Uncategorized\n"),
                (file, run.as_str()),
            ]);
            let original = ledger::load(&snapshot, "main.beancount");
            prop_assert!(original.errors.is_empty(), "{:?}\n{}", original.errors, run);
            let before = original.find(TAG_ID).unwrap().clone();

            let mut model: BTreeSet<&str> = before.tags.iter().map(String::as_str).collect();
            let mut expected = Vec::new();
            let mut intents: Vec<Intent> = Vec::new();
            for (k, &(add, t)) in g.ops.iter().enumerate() {
                if g.payee == Some(k) {
                    intents.push(Intent::SetPayee { id: TAG_ID.into(), old: Some("Shop".into()), new: "Market".into() });
                    expected.push(Outcome::Applied);
                }
                let tag = TAGS[t];
                let changed = if add { model.insert(tag) } else { model.remove(tag) };
                expected.push(if changed { Outcome::Applied } else { Outcome::AlreadyApplied });
                let (id, tag) = (TAG_ID.to_owned(), tag.to_owned());
                intents.push(if add { Intent::AddTag { id, tag } } else { Intent::RemoveTag { id, tag } });
            }
            if g.payee == Some(g.ops.len()) {
                intents.push(Intent::SetPayee { id: TAG_ID.into(), old: Some("Shop".into()), new: "Market".into() });
                expected.push(Outcome::Applied);
            }
            let intents: Vec<PendingIntent> = intents.into_iter().enumerate()
                .map(|(k, intent)| PendingIntent { key: format!("k{k}"), intent })
                .collect();

            let first = replay(&snapshot, &config, &intents);
            let outcomes: Vec<Outcome> = first.outcomes.iter().map(|(_, o)| o.clone()).collect();
            prop_assert_eq!(outcomes, expected);
            let text = first.snapshot.text(file).unwrap();
            let reloaded = ledger::load(&first.snapshot, "main.beancount");
            prop_assert!(reloaded.errors.is_empty(), "{:?}\n{}", reloaded.errors, text);
            let after = reloaded.find(TAG_ID).unwrap();
            let tags: BTreeSet<&str> = after.tags.iter().map(String::as_str).collect();
            prop_assert_eq!(tags, model, "{}", text);
            prop_assert_eq!(after.flag, '!');
            let payee = if g.payee.is_some() { "Market" } else { "Shop" };
            prop_assert_eq!(after.payee.as_deref(), Some(payee));
            prop_assert_eq!(&text[..after.span.start], &run[..before.span.start]);
            prop_assert_eq!(&text[after.span.end..], &run[before.span.end..]);
            let (old, new) = (&run[before.span.clone()], &text[after.span.clone()]);
            prop_assert_eq!(other_lines(new), other_lines(old));
            let breaks = if g.crlf { text.matches("\r\n").count() } else { 0 };
            prop_assert_eq!(text.matches('\r').count(), breaks);
            let (prefix, tokens, comment) = without_tags(old);
            prop_assert_eq!(without_tags(new), (prefix.replacen("Shop", payee, 1), tokens, comment), "{}", text);

            let second = replay(&first.snapshot, &config, &intents);
            let once = g.ops.iter().map(|(_, t)| t).collect::<BTreeSet<_>>().len() == g.ops.len();
            if once {
                for (key, outcome) in &second.outcomes {
                    prop_assert_eq!(outcome, &Outcome::AlreadyApplied, "{}", key);
                }
            }
            prop_assert_eq!(&second.snapshot, &first.snapshot);
        }
    }
}
