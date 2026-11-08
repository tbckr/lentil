//! The app's handle on its clone.
//!
//! Reading works offline on the local `main`: the app's pending intents are
//! replayed on it, so its edits show before they are synced (PRD: FR-D5).

use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};

use jiff::civil::Date;
use lentil_core::config::{CONFIG_FILE, Config, ConfigError};
use lentil_core::correction::{self, CorrectionRequest, RuleWords};
use lentil_core::finding::{Finding, FindingCode};
use lentil_core::intent::{self, PendingIntent};
use lentil_core::ledger::{self, Ledger, Txn};
use lentil_core::snapshot::Snapshot;
use lentil_core::sync::git::Git2Backend;
use lentil_core::sync::{self, BRANCH, CommitId, Credentials, GitBackend, Identity, ThreadSleeper};
use lentil_core::tags::{self, Taxonomy, TaxonomyError};
use lentil_core::trips::Trips;
use lentil_core::validate::{self, Role};

use crate::error::LentilError;
use crate::types::{
    AccountItem, FfiAuthor, FfiCorrection, FfiCredentials, FfiFinding, FfiIdentity, FfiIntent,
    FfiPendingIntent, FfiRuleWords, FfiTagChoices, QueueItem, SyncReport, count, findings,
    outcomes, pending_intents,
};

/// A local clone of a Lentil repository.
#[derive(Debug, uniffi::Object)]
pub struct LentilRepo {
    path: PathBuf,
    /// The last loaded view; the app asks for queue, accounts and opening
    /// date of the same state in a row, so the ledger loads once.
    cache: Mutex<Option<Arc<View>>>,
    /// One sync at a time per clone.
    sync_lock: Mutex<()>,
}

/// The ledger of the local `main` with pending intents replayed.
#[derive(Debug)]
struct View {
    head: Option<CommitId>,
    pending: Vec<PendingIntent>,
    config: Config,
    /// The files of the local `main` with `pending` replayed, which
    /// `ledger` indexes; corrections run on them.
    snapshot: Snapshot,
    ledger: Ledger,
    /// `tags.toml`; `None` without the file.
    taxonomy: Option<Result<Taxonomy, TaxonomyError>>,
    trips: Trips,
}

#[uniffi::export]
impl LentilRepo {
    /// Opens the clone at `path`.
    ///
    /// # Errors
    ///
    /// Fails if `path` is not a Git repository.
    #[uniffi::constructor]
    pub fn open(path: String) -> Result<Arc<Self>, LentilError> {
        let path = PathBuf::from(path);
        Git2Backend::open(&path, Credentials::Default)?;
        Ok(Arc::new(Self::new(path)))
    }

    /// Clones `url` into `path` without a working tree; `path` must not exist
    /// or be empty.
    ///
    /// # Errors
    ///
    /// Fails if the remote is unreachable, rejects the credentials or has no
    /// `main` branch.
    #[uniffi::constructor]
    pub fn clone_repo(
        url: &str,
        path: String,
        credentials: FfiCredentials,
    ) -> Result<Arc<Self>, LentilError> {
        let path = PathBuf::from(path);
        Git2Backend::clone_bare(url, &path, credentials.into())?;
        Ok(Arc::new(Self::new(path)))
    }

    /// The review queue with `pending` applied: open transactions, newest
    /// first (PRD: FR-D1).
    ///
    /// # Errors
    ///
    /// Fails if the clone cannot be read or `lentil.toml` is invalid.
    pub fn queue(&self, pending: Vec<FfiPendingIntent>) -> Result<Vec<QueueItem>, LentilError> {
        let view = self.view(pending)?;
        Ok(queue_items(&view.config, &view.ledger, &view.trips))
    }

    /// What the tag picker offers for transaction `id`, with `pending`
    /// applied: the taxonomy's tags it does not carry, each with the tags it
    /// would replace, and the families; without a readable `tags.toml`, the
    /// tags the ledger uses (PRD: FR-D12).
    ///
    /// # Errors
    ///
    /// As [`Self::queue`].
    pub fn tag_choices(
        &self,
        pending: Vec<FfiPendingIntent>,
        id: &str,
    ) -> Result<FfiTagChoices, LentilError> {
        let view = self.view(pending)?;
        Ok(tag_choices_of(&view.ledger, view.taxonomy.as_ref(), id))
    }

    /// The intents of picking `tag` for transaction `id`: a `RemoveTag` for
    /// each exclusive rival it replaces, then the `AddTag`. The app records
    /// them for picks and for accepted trip suggestions alike.
    ///
    /// # Errors
    ///
    /// As [`Self::queue`].
    pub fn pick_tag(
        &self,
        pending: Vec<FfiPendingIntent>,
        id: &str,
        tag: &str,
    ) -> Result<Vec<FfiIntent>, LentilError> {
        let view = self.view(pending)?;
        pick_of(&view.ledger, view.taxonomy.as_ref(), id, tag)
    }

    /// How many other transactions the rule from `rule` would move when the
    /// user corrects transaction `id` to `account`, with `pending` applied;
    /// `open_date` is the date to open `account` on, if it is new. The app
    /// shows the count before the user confirms (PRD: FR-D14). It is
    /// computed on the loaded ledger without a replay, so it is cheap enough
    /// for every choice of words.
    ///
    /// # Errors
    ///
    /// As [`Self::queue`]; also [`LentilError::NotCorrectable`] if no active
    /// transaction has `id` or it has no single counter posting, and
    /// [`LentilError::Other`] if `open_date` is not a `YYYY-MM-DD` date.
    pub fn rule_preview(
        &self,
        pending: Vec<FfiPendingIntent>,
        id: &str,
        account: &str,
        open_date: Option<String>,
        rule: FfiRuleWords,
    ) -> Result<u32, LentilError> {
        let view = self.view(pending)?;
        let rule = RuleWords::from(rule);
        let request = correction_request(id, account, open_date, Some(&rule))?;
        count(correction::preview(&view.ledger, &view.config, &request)?)
    }

    /// The intents of correcting transaction `id` to `account`, with
    /// `pending` applied: the user's own, an `AddRule` for the words in
    /// `rule`, and the `SuggestAccount` of the other open transactions that
    /// move with them. The app records them all and syncs them like any
    /// other (PRD: FR-D14, FR-D15).
    ///
    /// # Errors
    ///
    /// As [`Self::rule_preview`].
    pub fn correct(
        &self,
        pending: Vec<FfiPendingIntent>,
        id: &str,
        account: &str,
        open_date: Option<String>,
        rule: Option<FfiRuleWords>,
    ) -> Result<FfiCorrection, LentilError> {
        let view = self.view(pending)?;
        let rule = rule.map(RuleWords::from);
        let request = correction_request(id, account, open_date, rule.as_ref())?;
        correction::correct(&view.snapshot, &view.config, &request)?.try_into()
    }

    /// The name of the running trip with `pending` applied, `None` if no trip
    /// runs: the app offers "End trip" for it, otherwise "Start trip".
    ///
    /// # Errors
    ///
    /// As [`Self::queue`].
    pub fn running_trip(
        &self,
        pending: Vec<FfiPendingIntent>,
    ) -> Result<Option<String>, LentilError> {
        Ok(self.view(pending)?.trips.running().map(str::to_owned))
    }

    /// Every opened account, with `pending` applied, sorted by name, with the
    /// date from which a counter posting may move to it.
    ///
    /// # Errors
    ///
    /// As [`Self::queue`].
    pub fn accounts(
        &self,
        pending: Vec<FfiPendingIntent>,
    ) -> Result<Vec<AccountItem>, LentilError> {
        let view = self.view(pending)?;
        Ok(account_items(&view.config, &view.ledger))
    }

    /// The opening date for accounts the user opens during triage: the date
    /// of the oldest transaction, as `YYYY-MM-DD`; `None` without transactions.
    ///
    /// # Errors
    ///
    /// As [`Self::queue`].
    pub fn default_open_date(
        &self,
        pending: Vec<FfiPendingIntent>,
    ) -> Result<Option<String>, LentilError> {
        Ok(oldest_transaction_date(&self.view(pending)?.ledger))
    }

    /// Why the local `main` is read-only for the app (spec section 13); empty
    /// if the app may write.
    ///
    /// # Errors
    ///
    /// Fails if the clone cannot be read.
    pub fn read_only_reasons(&self) -> Result<Vec<FfiFinding>, LentilError> {
        let head = self.local_head()?;
        let git = Git2Backend::open(&self.path, Credentials::Default)?;
        let snapshot = git.snapshot(head.as_ref())?;
        Ok(findings(validate::read_only_reasons(&snapshot)))
    }

    /// Pushes `pending` as editor (spec section 11). On success, the caller
    /// removes applied and already-applied intents and moves conflicts to the
    /// user's conflict list; on error it keeps every intent.
    ///
    /// # Errors
    ///
    /// See [`LentilError`].
    pub fn sync(
        &self,
        pending: Vec<FfiPendingIntent>,
        identity: FfiIdentity,
        author: FfiAuthor,
        credentials: FfiCredentials,
    ) -> Result<SyncReport, LentilError> {
        let pending = pending_intents(pending)?;
        let _syncing = self
            .sync_lock
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let mut git = Git2Backend::open(&self.path, credentials.into())?;
        let identity = Identity {
            role: Role::Editor,
            client: identity.client,
            device: identity.device,
        };
        let result = sync::sync_intents(
            &mut git,
            &mut ThreadSleeper,
            &identity,
            &author.into(),
            &pending,
        );
        // Even a failed sync may have moved the local `main`.
        *self.cache.lock().unwrap_or_else(PoisonError::into_inner) = None;
        let result = result?;
        Ok(SyncReport {
            outcomes: outcomes(result.outcomes),
            committed: result.commit.is_some(),
            warnings: findings(result.warnings),
        })
    }
}

impl LentilRepo {
    fn new(path: PathBuf) -> Self {
        Self {
            path,
            cache: Mutex::new(None),
            sync_lock: Mutex::new(()),
        }
    }

    /// The commit of the local `main`, `None` in an empty repository.
    fn local_head(&self) -> Result<Option<CommitId>, LentilError> {
        let repo = git2::Repository::open(&self.path)?;
        match repo.refname_to_id(&format!("refs/heads/{BRANCH}")) {
            Ok(oid) => Ok(Some(CommitId(oid.to_string()))),
            Err(err) if err.code() == git2::ErrorCode::NotFound => Ok(None),
            Err(err) => Err(err.into()),
        }
    }

    fn view(&self, pending: Vec<FfiPendingIntent>) -> Result<Arc<View>, LentilError> {
        let pending = pending_intents(pending)?;
        let head = self.local_head()?;
        let mut cache = self.cache.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(view) = cache.as_ref()
            && view.head == head
            && view.pending == pending
        {
            return Ok(Arc::clone(view));
        }

        let git = Git2Backend::open(&self.path, Credentials::Default)?;
        let snapshot = git.snapshot(head.as_ref())?;
        let config = parse_config(snapshot.text(CONFIG_FILE))?;
        let snapshot = if pending.is_empty() {
            snapshot
        } else {
            intent::replay(&snapshot, &config, &pending).snapshot
        };
        let ledger = ledger::load(&snapshot, &config.main_file);
        let taxonomy = Taxonomy::load(&snapshot);
        let trips = Trips::new(&ledger.trip_events);
        let view = Arc::new(View {
            head,
            pending,
            config,
            snapshot,
            ledger,
            taxonomy,
            trips,
        });
        *cache = Some(Arc::clone(&view));
        Ok(view)
    }
}

/// Parses `lentil.toml`; a missing or invalid file makes the repository
/// read-only, like any other blocking finding.
fn parse_config(text: Option<&str>) -> Result<Config, LentilError> {
    let finding = match text.map(Config::parse) {
        Some(Ok(config)) => return Ok(config),
        Some(Err(ConfigError::UnsupportedSpecVersion { found, supported })) => {
            Finding::blocking(FindingCode::SpecVersionUnsupported)
                .arg("found", found.to_string())
                .arg("supported", supported.to_string())
        }
        Some(Err(err)) => {
            Finding::blocking(FindingCode::ConfigInvalid).arg("detail", err.to_string())
        }
        None => Finding::blocking(FindingCode::ConfigInvalid)
            .arg("detail", format!("{CONFIG_FILE} is missing")),
    };
    Err(LentilError::ReadOnly {
        findings: vec![finding.at(CONFIG_FILE, None).into()],
    })
}

/// The queue items of `ledger`, newest first.
fn queue_items(config: &Config, ledger: &Ledger, trips: &Trips) -> Vec<QueueItem> {
    ledger
        .open_queue()
        .into_iter()
        .map(|txn| queue_item(config, txn, trips))
        .collect()
}

/// The tags transaction `id` carries; none if the ledger does not hold it.
fn carried<'l>(ledger: &'l Ledger, id: &str) -> &'l [String] {
    ledger.find(id).map_or(&[], |txn| txn.tags.as_slice())
}

/// The picker's choices for transaction `id`.
fn tag_choices_of(
    ledger: &Ledger,
    taxonomy: Option<&Result<Taxonomy, TaxonomyError>>,
    id: &str,
) -> FfiTagChoices {
    tags::tag_choices(taxonomy, ledger, carried(ledger, id)).into()
}

/// The intents of picking `tag` for transaction `id`.
fn pick_of(
    ledger: &Ledger,
    taxonomy: Option<&Result<Taxonomy, TaxonomyError>>,
    id: &str,
    tag: &str,
) -> Result<Vec<FfiIntent>, LentilError> {
    let taxonomy = taxonomy.and_then(|t| t.as_ref().ok());
    tags::pick_tag(taxonomy, id, tag, carried(ledger, id))
        .into_iter()
        .map(FfiIntent::try_from)
        .collect()
}

/// The request to correct transaction `id` to `account`, opening `account`
/// on `open_date` (`YYYY-MM-DD`) if given.
fn correction_request<'a>(
    id: &'a str,
    account: &'a str,
    open_date: Option<String>,
    rule: Option<&'a RuleWords>,
) -> Result<CorrectionRequest<'a>, LentilError> {
    let open_date = open_date
        .map(|date| {
            date.parse::<Date>().map_err(|_| LentilError::Other {
                detail: format!("invalid open date {date:?}"),
            })
        })
        .transpose()?;
    Ok(CorrectionRequest {
        id,
        account,
        open_date,
        rule,
    })
}

fn queue_item(config: &Config, txn: &Txn, trips: &Trips) -> QueueItem {
    let source = source_posting(config, txn);
    // Only an unambiguous counter posting is offered for editing; splits and
    // postings on another source account stay read-only in the app (PRD:
    // FR-D10, spec section 7).
    let counter = source.and_then(|source| {
        let mut others = (0..txn.postings.len()).filter(|&i| i != source);
        match (others.next(), others.next()) {
            (Some(only), None) => Some(only),
            _ => None,
        }
    });
    let counter = counter.filter(|&i| {
        txn.postings
            .get(i)
            .is_some_and(|p| !config.is_source_account(&p.account))
    });
    let source = source.and_then(|i| txn.postings.get(i));
    let counter = counter.and_then(|i| Some((i, txn.postings.get(i)?)));
    let units = source.and_then(|p| p.units.as_ref());
    QueueItem {
        id: txn.id.clone(),
        date: txn.date.to_string(),
        payee: txn.payee.clone(),
        narration: txn.narration.clone(),
        desc: txn.original_text(),
        amount: units.map(|(number, _)| number.to_string()),
        currency: units.map(|(_, currency)| currency.clone()),
        source_account: source.map(|p| p.account.clone()),
        counter_account: counter.map(|(_, p)| p.account.clone()),
        counter_posting: counter.and_then(|(i, _)| u32::try_from(i).ok()),
        file: txn.file.clone(),
        line: txn.line,
        // A header may repeat a tag; the row shows each tag once.
        tags: txn.tags.iter().fold(Vec::new(), |mut tags, tag| {
            if !tags.contains(tag) {
                tags.push(tag.clone());
            }
            tags
        }),
        suggested_tag: trips.suggestion(config, txn),
        rule_words: correction::rule_words(txn, config).into(),
    }
}

/// The index of the source posting shown in the queue: in a run file, the
/// posting on that file's own source account; elsewhere the first posting on
/// any source account.
fn source_posting(config: &Config, txn: &Txn) -> Option<usize> {
    match config.own_source_account(&txn.file) {
        Some(own) => txn.postings.iter().position(|p| p.account == own),
        None => txn
            .postings
            .iter()
            .position(|p| config.is_source_account(&p.account)),
    }
}

/// Every opened account. A counter posting may move to one that is not
/// closed and not a source account (spec section 7), from its open date on,
/// because `set-account` needs the account open on the transaction's date.
/// The others are listed too, so the app does not offer to open them again.
fn account_items(config: &Config, ledger: &Ledger) -> Vec<AccountItem> {
    ledger
        .accounts
        .iter()
        .filter_map(|(account, state)| {
            let opened = state.opened?;
            let usable = state.closed.is_none() && !config.is_source_account(account);
            Some(AccountItem {
                name: account.clone(),
                open_from: usable.then(|| opened.to_string()),
            })
        })
        .collect()
}

fn oldest_transaction_date(ledger: &Ledger) -> Option<String> {
    ledger
        .transactions
        .iter()
        .map(|txn| txn.date)
        .min()
        .map(|date| date.to_string())
}

#[cfg(test)]
mod tests {
    use lentil_core::snapshot::Snapshot;
    use lentil_core::tags::Taxonomy;
    use lentil_core::trips::Trips;

    use crate::types::{FfiChoiceSource, FfiIntent, FfiTagChoice, FfiTagChoices};

    use super::*;

    const CONFIG: &str = r#"spec_version = 0
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

    const MAIN: &str = r#"option "operating_currency" "EUR"
include "accounts.beancount"
include "imports/*/*/*.beancount"
include "manual/*/*.beancount"
"#;

    const ACCOUNTS: &str = "2026-01-01 open Assets:Bank:Giro EUR
2026-01-01 open Liabilities:Card EUR
2026-01-01 open Expenses:Groceries EUR
2026-03-15 open Expenses:Uncategorized EUR
2026-01-01 open Expenses:Travel EUR
2026-02-01 close Expenses:Travel
";

    const GIRO_RUN: &str = r#"2026-10-18 ! "Corner Grocery" "Weekly shopping"
  lentil_id: "giro-3f9a1c2e7b4d"
  lentil_counterparty: "CORNER GROCERY 4711"
  lentil_purpose: "CARD PAYMENT"
  Assets:Bank:Giro  -42.10 EUR
  Expenses:Groceries

2026-10-19 * "Card settlement"
  lentil_id: "giro-0a1b2c3d4e5f"
  lentil_counterparty: "CARD SETTLEMENT"
  Assets:Bank:Giro  -100.00 EUR
  Liabilities:Card  100.00 EUR

2026-10-20 ! "Split purchase"
  lentil_id: "giro-111111111111"
  Assets:Bank:Giro  -30.00 EUR
  Expenses:Groceries  20.00 EUR
  Expenses:Uncategorized  10.00 EUR
"#;

    const CARD_RUN: &str = r#"2026-10-19 ! "Card settlement"
  lentil_id: "card-222222222222"
  lentil_counterparty: "PAYMENT RECEIVED"
  Assets:Bank:Giro  -100.00 EUR
  Liabilities:Card  100.00 EUR
"#;

    const GIRO_FILE: &str = "imports/giro/2026/20261020T071502Z.beancount";
    const TRIP_FILE: &str = "manual/k7p2q9xd/2026-10.beancount";
    const TRIP_START: &str = "2026-10-17 event \"trip\" \"lakeside\"\n  lentil_id: \"k7p2q9xd-01jc8m2w9z4f7q3r5t6v8x0y1a\"\n";
    const GROCERY_HEADER: &str = "2026-10-18 ! \"Corner Grocery\" \"Weekly shopping\"";

    fn sample_files() -> Snapshot {
        Snapshot::from_files([
            ("lentil.toml", CONFIG),
            ("main.beancount", MAIN),
            ("accounts.beancount", ACCOUNTS),
            (GIRO_FILE, GIRO_RUN),
            ("imports/card/2026/20261020T071503Z.beancount", CARD_RUN),
        ])
    }

    fn loaded(snapshot: &Snapshot) -> (Config, Ledger) {
        let config = parse_config(snapshot.text(CONFIG_FILE)).unwrap();
        let ledger = ledger::load(snapshot, &config.main_file);
        (config, ledger)
    }

    fn sample() -> (Config, Ledger) {
        loaded(&sample_files())
    }

    /// The sample with `#friends` on the grocery transaction of 10-18 and,
    /// with `trip`, the start of a trip on 10-17.
    fn tagged(trip: bool) -> (Config, Ledger) {
        let mut files = sample_files();
        let run = GIRO_RUN.replace(GROCERY_HEADER, &format!("{GROCERY_HEADER} #friends"));
        files.insert(GIRO_FILE, run);
        if trip {
            files.insert(TRIP_FILE, TRIP_START);
        }
        loaded(&files)
    }

    #[test]
    fn queue_lists_open_transactions_newest_first() {
        let (config, ledger) = sample();

        let ids: Vec<_> = queue_items(&config, &ledger, &Trips::default())
            .into_iter()
            .map(|item| item.id)
            .collect();

        assert_eq!(
            ids,
            vec![
                Some("giro-111111111111".to_owned()),
                Some("card-222222222222".to_owned()),
                Some("giro-3f9a1c2e7b4d".to_owned()),
            ]
        );
    }

    #[test]
    fn queue_item_names_source_and_counter_posting() {
        let (config, ledger) = sample();
        let items = queue_items(&config, &ledger, &Trips::default());

        let grocery = items
            .iter()
            .find(|item| item.id.as_deref() == Some("giro-3f9a1c2e7b4d"))
            .unwrap();
        assert_eq!(
            grocery,
            &QueueItem {
                id: Some("giro-3f9a1c2e7b4d".to_owned()),
                date: "2026-10-18".to_owned(),
                payee: Some("Corner Grocery".to_owned()),
                narration: "Weekly shopping".to_owned(),
                desc: Some("CORNER GROCERY 4711 CARD PAYMENT".to_owned()),
                amount: Some("-42.10".to_owned()),
                currency: Some("EUR".to_owned()),
                source_account: Some("Assets:Bank:Giro".to_owned()),
                counter_account: Some("Expenses:Groceries".to_owned()),
                counter_posting: Some(1),
                file: "imports/giro/2026/20261020T071502Z.beancount".to_owned(),
                line: 1,
                tags: vec![],
                suggested_tag: None,
                rule_words: FfiRuleWords {
                    counterparty: vec!["corner".to_owned(), "grocery".to_owned()],
                    purpose: vec!["card".to_owned(), "payment".to_owned()],
                },
            }
        );
    }

    #[test]
    fn queue_items_carry_the_words_a_rule_can_match() {
        // A transaction outside the run files of the sources offers none,
        // and so does one without original fields.
        let mut files = sample_files();
        files.insert(
            "manual/k7p2q9xd/2026-10.beancount",
            "2026-10-21 ! \"Corner Grocery\" \"Cash\"\n  lentil_id: \"k7p2q9xd-01jc8m2w9z4f7q3r5t6v8x0y1a\"\n  lentil_counterparty: \"CORNER GROCERY\"\n  Assets:Bank:Giro  -5.00 EUR\n  Expenses:Groceries\n",
        );
        let (config, ledger) = loaded(&files);
        let words: Vec<(Option<String>, FfiRuleWords)> =
            queue_items(&config, &ledger, &Trips::default())
                .into_iter()
                .map(|item| (item.id, item.rule_words))
                .collect();
        let none = || FfiRuleWords {
            counterparty: vec![],
            purpose: vec![],
        };
        let id = |id: &str| Some(id.to_owned());
        assert_eq!(
            words,
            vec![
                (id("k7p2q9xd-01jc8m2w9z4f7q3r5t6v8x0y1a"), none()),
                (id("giro-111111111111"), none()),
                (
                    id("card-222222222222"),
                    FfiRuleWords {
                        counterparty: vec!["payment".to_owned(), "received".to_owned()],
                        purpose: vec![],
                    }
                ),
                (
                    id("giro-3f9a1c2e7b4d"),
                    FfiRuleWords {
                        counterparty: vec!["corner".to_owned(), "grocery".to_owned()],
                        purpose: vec!["card".to_owned(), "payment".to_owned()],
                    }
                ),
            ]
        );
    }

    #[test]
    fn queue_items_carry_tags_and_the_trip_suggestion() {
        let (config, ledger) = tagged(true);
        let trips = Trips::new(&ledger.trip_events);
        let items = queue_items(&config, &ledger, &trips);
        let item = |id: &str| items.iter().find(|i| i.id.as_deref() == Some(id)).unwrap();
        let grocery = item("giro-3f9a1c2e7b4d");
        assert_eq!(grocery.tags, ["friends"]);
        assert_eq!(grocery.suggested_tag.as_deref(), Some("trip-lakeside"));
        assert_eq!(
            item("giro-111111111111").suggested_tag.as_deref(),
            Some("trip-lakeside")
        );

        let (config, ledger) = tagged(false);
        let trips = Trips::new(&ledger.trip_events);
        assert!(
            queue_items(&config, &ledger, &trips)
                .iter()
                .all(|i| i.suggested_tag.is_none())
        );
    }

    #[test]
    fn a_tag_written_twice_shows_once() {
        let mut files = sample_files();
        let run = GIRO_RUN.replace(
            GROCERY_HEADER,
            &format!("{GROCERY_HEADER} #friends #solo #friends"),
        );
        files.insert(GIRO_FILE, run);
        let (config, ledger) = loaded(&files);
        let items = queue_items(&config, &ledger, &Trips::default());
        let grocery = items
            .iter()
            .find(|i| i.id.as_deref() == Some("giro-3f9a1c2e7b4d"))
            .unwrap();
        assert_eq!(grocery.tags, ["friends", "solo"]);
    }

    /// A clone of a remote whose `main` holds `files`.
    fn clone_of(files: &Snapshot) -> (tempfile::TempDir, Arc<LentilRepo>) {
        use lentil_core::sync::{Author, GitBackend};
        let dir = tempfile::TempDir::new().unwrap();
        let remote = dir.path().join("remote.git");
        let mut options = git2::RepositoryInitOptions::new();
        options.bare(true).initial_head("main");
        git2::Repository::init_opts(&remote, &options).unwrap();
        let url = remote.to_string_lossy().into_owned();
        let mut seed =
            Git2Backend::clone_bare(&url, &dir.path().join("seed"), Credentials::Default).unwrap();
        seed.fetch().unwrap();
        let author = Author {
            name: "Admin".to_owned(),
            email: "admin@example.org".to_owned(),
        };
        let commit = seed
            .commit(None, &Snapshot::new(), files, "admin: init\n", &author)
            .unwrap();
        seed.push(&commit).unwrap();
        let clone = dir.path().join("clone").to_string_lossy().into_owned();
        let repo = LentilRepo::clone_repo(&url, clone, FfiCredentials::Default).unwrap();
        (dir, repo)
    }

    #[test]
    fn the_view_shows_a_pending_trip_before_it_syncs() {
        let (_dir, repo) = clone_of(&sample_files());
        assert_eq!(repo.running_trip(vec![]).unwrap(), None);
        let start = FfiPendingIntent {
            key: "t1".to_owned(),
            intent: FfiIntent::StartTrip {
                id: "k7p2q9xd-01jc8m2w9z4f7q3r5t6v8x0y1a".to_owned(),
                date: "2026-10-17".to_owned(),
                name: "lakeside".to_owned(),
            },
        };
        assert_eq!(
            repo.running_trip(vec![start.clone()]).unwrap().as_deref(),
            Some("lakeside")
        );
        let queue = repo.queue(vec![start]).unwrap();
        assert!(!queue.is_empty());
        assert!(
            queue
                .iter()
                .all(|i| i.suggested_tag.as_deref() == Some("trip-lakeside")),
            "{queue:?}"
        );
    }

    #[test]
    fn tag_choices_and_picks_follow_the_taxonomy() {
        let (_, ledger) = tagged(false);
        let taxonomy = Taxonomy::parse(
            "tags = [\"friends\", \"solo\"]\nexclusive = [[\"friends\", \"solo\"]]\n",
        );
        let grocery = "giro-3f9a1c2e7b4d";
        assert_eq!(
            tag_choices_of(&ledger, Some(&taxonomy), grocery),
            FfiTagChoices {
                tags: vec![FfiTagChoice {
                    tag: "solo".to_owned(),
                    replaces: vec!["friends".to_owned()],
                }],
                families: vec![],
                source: FfiChoiceSource::Taxonomy,
            }
        );
        let tag = |intent: fn(String, String) -> FfiIntent, id: &str, tag: &str| {
            intent(id.to_owned(), tag.to_owned())
        };
        let add = |id: String, tag: String| FfiIntent::AddTag { id, tag };
        let remove = |id: String, tag: String| FfiIntent::RemoveTag { id, tag };
        assert_eq!(
            pick_of(&ledger, Some(&taxonomy), grocery, "solo").unwrap(),
            vec![tag(remove, grocery, "friends"), tag(add, grocery, "solo")]
        );
        assert_eq!(
            pick_of(&ledger, Some(&taxonomy), "giro-ffffffffffff", "solo").unwrap(),
            vec![tag(add, "giro-ffffffffffff", "solo")]
        );
        assert_eq!(
            tag_choices_of(&ledger, None, grocery).source,
            FfiChoiceSource::Missing
        );
    }

    #[test]
    fn running_trip_is_the_last_started_one() {
        let (_, ledger) = tagged(true);
        assert_eq!(Trips::new(&ledger.trip_events).running(), Some("lakeside"));

        let mut files = sample_files();
        files.insert(
            TRIP_FILE,
            format!("{TRIP_START}2026-10-20 event \"trip\" \"\"\n  lentil_id: \"k7p2q9xd-01jcfh3k5n7p9r1s3t5v7w9y2b\"\n"),
        );
        let (_, ledger) = loaded(&files);
        assert_eq!(Trips::new(&ledger.trip_events).running(), None);
    }

    #[test]
    fn source_posting_follows_the_run_files_source() {
        let (config, ledger) = sample();
        let items = queue_items(&config, &ledger, &Trips::default());

        // Imported from the card: the card posting is the source posting,
        // although the giro posting comes first. The giro posting is a source
        // posting too, so the app offers no counter posting to edit.
        let settlement = items
            .iter()
            .find(|item| item.id.as_deref() == Some("card-222222222222"))
            .unwrap();
        assert_eq!(
            settlement.source_account.as_deref(),
            Some("Liabilities:Card")
        );
        assert_eq!(settlement.amount.as_deref(), Some("100.00"));
        assert_eq!(settlement.counter_account, None);
        assert_eq!(settlement.counter_posting, None);
    }

    #[test]
    fn split_has_no_counter_posting() {
        let (config, ledger) = sample();
        let items = queue_items(&config, &ledger, &Trips::default());

        let split = items
            .iter()
            .find(|item| item.id.as_deref() == Some("giro-111111111111"))
            .unwrap();
        assert_eq!(split.source_account.as_deref(), Some("Assets:Bank:Giro"));
        assert_eq!(split.counter_account, None);
        assert_eq!(split.counter_posting, None);
    }

    #[test]
    fn counter_postings_may_move_to_open_accounts_from_their_open_date() {
        let (config, ledger) = sample();

        let item = |name: &str, open_from: Option<&str>| AccountItem {
            name: name.to_owned(),
            open_from: open_from.map(str::to_owned),
        };
        assert_eq!(
            account_items(&config, &ledger),
            vec![
                // Source accounts and closed accounts are listed, but never usable.
                item("Assets:Bank:Giro", None),
                item("Expenses:Groceries", Some("2026-01-01")),
                item("Expenses:Travel", None),
                item("Expenses:Uncategorized", Some("2026-03-15")),
                item("Liabilities:Card", None),
            ]
        );
    }

    #[test]
    fn default_open_date_is_the_oldest_transaction() {
        let (_, ledger) = sample();

        assert_eq!(
            oldest_transaction_date(&ledger).as_deref(),
            Some("2026-10-18")
        );
        assert_eq!(oldest_transaction_date(&Ledger::default()), None);
    }

    #[test]
    fn missing_or_newer_config_makes_the_repository_read_only() {
        let Err(LentilError::ReadOnly { findings }) = parse_config(None) else {
            panic!("expected ReadOnly");
        };
        assert_eq!(findings[0].code, "ConfigInvalid");
        assert_eq!(findings[0].file.as_deref(), Some("lentil.toml"));

        let newer = CONFIG.replace("spec_version = 0", "spec_version = 7");
        let Err(LentilError::ReadOnly { findings }) = parse_config(Some(&newer)) else {
            panic!("expected ReadOnly");
        };
        assert_eq!(findings[0].code, "SpecVersionUnsupported");
        assert_eq!(findings[0].args.get("found").map(String::as_str), Some("7"));
    }
}
