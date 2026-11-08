//! Re-applying the rules to imported transactions (spec section 16, PRD
//! FR-C19).
//!
//! The plan is a list of editor intents: `set-payee`, `set-account` and
//! `add-tag`, never a flag change. [`apply`] replays them on the state the
//! plan was made from, and [`sync`] plans anew on every state it fetches, so a
//! concurrent push never turns its changes into conflicts of their own.
//!
//! Tags the rules no longer yield are only listed, by
//! [`removal_candidates`]. A removal the user confirms becomes a `remove-tag`
//! intent that every attempt replays before it plans, and that is never
//! planned again on a newly fetched state (PRD FR-C21).

use jiff::civil::Date;
use thiserror::Error;

use crate::config::SourceConfig;
use crate::config::{CONFIG_FILE, Config, ConfigError};
use crate::import::{Original, Rules, RulesError, opening_id};
use crate::intent::{self, Intent, Outcome, PendingIntent};
use crate::ledger::{self, Ledger, Txn};
use crate::snapshot::Snapshot;
use crate::status::display_text;
use crate::sync::{Author, GitBackend, Identity, Sleeper, SyncError, SyncOutcome, Work, run_sync};
use crate::tags::{Taxonomy, TaxonomyError};

/// Which transactions and fields to re-apply the rules to.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ReapplyOptions {
    /// Also reviewed (`*`) transactions, not only open (`!`) ones.
    pub reviewed: bool,
    /// The fields the rules may set.
    pub fields: ReapplyFields,
}

/// The fields the rules may set.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ReapplyFields {
    /// Payee, counter account and tags.
    #[default]
    All,
    /// Only the payee.
    PayeeOnly,
    /// Only the counter account.
    AccountOnly,
    /// Only the tags.
    TagsOnly,
}

impl ReapplyFields {
    /// Whether the rules may set the payee.
    fn payee(self) -> bool {
        matches!(self, Self::All | Self::PayeeOnly)
    }

    /// Whether the rules may set the counter account.
    fn account(self) -> bool {
        matches!(self, Self::All | Self::AccountOnly)
    }

    /// Whether the rules may add tags, and stale tags may be listed.
    fn tags(self) -> bool {
        matches!(self, Self::All | Self::TagsOnly)
    }
}

/// A field a rule sets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    /// The payee.
    Payee,
    /// The counter account.
    Account,
    /// A tag the rules add: [`RuleChange::new`] is the tag, `old` is `None`
    /// and the intent is `add-tag`.
    Tag,
}

/// One change the rules make to one transaction.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuleChange {
    /// `lentil_id` of the transaction.
    pub id: String,
    /// The run file.
    pub file: String,
    /// 1-based line of the transaction's header.
    pub line: u32,
    /// Transaction date.
    pub date: Date,
    /// Payee and narration as `lentil status` shows them, before the change.
    pub text: String,
    /// The field that changes.
    pub field: Field,
    /// The current value; `None` for a transaction without a payee.
    pub old: Option<String>,
    /// The rule's value.
    pub new: String,
    /// The editor intent that makes the change.
    pub intent: Intent,
}

/// The transactions the rules are re-applied to (spec section 16), in ledger
/// order, with their ID and source: active ones with a `lentil_id` in the run
/// file of a configured source, except the source's opening balance
/// transaction; open (`!`) ones always, and reviewed (`*`) ones only with
/// [`ReapplyOptions::reviewed`].
pub(crate) fn considered<'l>(
    ledger: &'l Ledger,
    config: &'l Config,
    options: &ReapplyOptions,
) -> impl Iterator<Item = (&'l Txn, &'l str, &'l SourceConfig)> {
    let reviewed = options.reviewed;
    ledger.transactions.iter().filter_map(move |txn| {
        let id = txn.id.as_deref()?;
        let source = config.run_source(&txn.file)?;
        let flag_selected = txn.flag == '!' || (reviewed && txn.flag == '*');
        (flag_selected && id != opening_id(&source.id)).then_some((txn, id, source))
    })
}

/// Whether a rule may not add `tag` to `txn`: `tags.toml` has an exclusive
/// group of which `txn` carries another member (spec section 16).
fn held_back(taxonomy: Option<&Taxonomy>, tag: &str, txn: &Txn) -> bool {
    taxonomy.is_some_and(|t| t.holds_back(tag, &txn.tags))
}

/// The changes the rules make to the imported transactions of `ledger`, in
/// ledger order; per transaction the payee, then the account, then each tag
/// in the order the evaluation adds them.
///
/// Only the transactions of spec section 16 count: active ones with a
/// `lentil_id` in the run file of a configured source, except the source's
/// opening balance transaction. Of those, open (`!`) ones always, and
/// reviewed (`*`) ones only with [`ReapplyOptions::reviewed`]. The rules see
/// the transaction's original fields, tags and the run file's source, as at
/// import. A field changes only if a rule sets it to another value, and the
/// account only if exactly one posting is not on a source account. The tags
/// are those the evaluation adds to the ones the transaction carries; the
/// exclusive groups of `taxonomy` hold a tag back if the transaction carries
/// another member.
#[must_use]
pub fn plan(
    ledger: &Ledger,
    config: &Config,
    rules: &Rules,
    taxonomy: Option<&Taxonomy>,
    options: &ReapplyOptions,
) -> Vec<RuleChange> {
    let mut changes = Vec::new();
    for (txn, id, source) in considered(ledger, config, options) {
        let found = rules.suggest(&source.id, &Original::from(txn));
        let change = |field, old: Option<&str>, new: &str, intent| RuleChange {
            id: id.to_owned(),
            file: txn.file.clone(),
            line: txn.line,
            date: txn.date,
            text: display_text(txn.payee.as_deref(), &txn.narration),
            field,
            old: old.map(ToOwned::to_owned),
            new: new.to_owned(),
            intent,
        };
        if options.fields.payee()
            && let Some(payee) = found.payee
            && txn.payee.as_deref() != Some(payee)
        {
            let intent = Intent::SetPayee {
                id: id.to_owned(),
                old: txn.payee.clone(),
                new: payee.to_owned(),
            };
            changes.push(change(Field::Payee, txn.payee.as_deref(), payee, intent));
        }
        if options.fields.account()
            && let Some(account) = found.account
            && let Some((posting, current)) = counter_posting(txn, config)
            && current != account
        {
            let intent = Intent::SetAccount {
                id: id.to_owned(),
                posting,
                old: current.to_owned(),
                new: account.to_owned(),
            };
            changes.push(change(Field::Account, Some(current), account, intent));
        }
        if options.fields.tags() {
            let carried: Vec<&str> = txn.tags.iter().map(String::as_str).collect();
            let added = rules.tags(&source.id, &Original::from(txn), &carried, |tag| {
                held_back(taxonomy, tag, txn)
            });
            for tag in added {
                let intent = Intent::AddTag {
                    id: id.to_owned(),
                    tag: tag.to_owned(),
                };
                changes.push(change(Field::Tag, None, tag, intent));
            }
        }
    }
    changes
}

/// A tag the rules no longer yield for a transaction, which the user may
/// confirm for removal (spec section 16).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TagRemoval {
    /// `lentil_id` of the transaction.
    pub id: String,
    /// The run file.
    pub file: String,
    /// 1-based line of the transaction's header.
    pub line: u32,
    /// Transaction date.
    pub date: Date,
    /// Payee and narration as `lentil status` shows them.
    pub text: String,
    /// The tag, without `#`.
    pub tag: String,
}

impl TagRemoval {
    /// The `remove-tag` intent that removes the tag once confirmed.
    #[must_use]
    pub fn intent(&self) -> Intent {
        Intent::RemoveTag {
            id: self.id.clone(),
            tag: self.tag.clone(),
        }
    }
}

/// The removal candidates of the transactions [`plan`] considers, in ledger
/// order and then in the order the tags are written (spec section 16).
///
/// A candidate is a tag the transaction carries that appears in some rule's
/// `add_tags`, but that the evaluation does not yield when it starts from the
/// transaction's tags that no rule adds. The exclusive groups of `taxonomy`
/// hold tags back by all the tags the transaction carries. Empty unless
/// [`ReapplyOptions::fields`] includes the tags.
#[must_use]
pub fn removal_candidates(
    ledger: &Ledger,
    config: &Config,
    rules: &Rules,
    taxonomy: Option<&Taxonomy>,
    options: &ReapplyOptions,
) -> Vec<TagRemoval> {
    if !options.fields.tags() {
        return Vec::new();
    }
    let mut candidates = Vec::new();
    for (txn, id, source) in considered(ledger, config, options) {
        let own: Vec<&str> = txn
            .tags
            .iter()
            .map(String::as_str)
            .filter(|tag| !rules.adds(tag))
            .collect();
        let yielded = rules.tags(&source.id, &Original::from(txn), &own, |tag| {
            held_back(taxonomy, tag, txn)
        });
        let mut listed: Vec<&str> = Vec::new();
        for tag in txn.tags.iter().map(String::as_str) {
            if rules.adds(tag) && !yielded.contains(&tag) && !listed.contains(&tag) {
                listed.push(tag);
                candidates.push(TagRemoval {
                    id: id.to_owned(),
                    file: txn.file.clone(),
                    line: txn.line,
                    date: txn.date,
                    text: display_text(txn.payee.as_deref(), &txn.narration),
                    tag: tag.to_owned(),
                });
            }
        }
    }
    candidates
}

/// The rule changes of one repository state, replayed as editor intents.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Applied {
    /// The state with every removal and change whose replay applied.
    pub snapshot: Snapshot,
    /// Commit subject: the applied operations of both replays, such as
    /// `set-account 3, set-payee 2, remove-tag 1` (spec section 12).
    pub subject: String,
    /// Each confirmed removal with the outcome of its replay, in input order.
    pub removals: Vec<(TagRemoval, Outcome)>,
    /// Each planned change with the outcome of its replay, in plan order.
    pub changes: Vec<(RuleChange, Outcome)>,
}

/// What re-applying the rules did on the state that was pushed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Reapplied {
    /// Each confirmed removal with its outcome, as in [`Applied::removals`].
    pub removals: Vec<(TagRemoval, Outcome)>,
    /// Each planned change with its outcome, as in [`Applied::changes`].
    pub changes: Vec<(RuleChange, Outcome)>,
}

/// Replays `intents` on `snapshot`, keyed by `prefix` and their position.
fn replay_all(
    snapshot: &Snapshot,
    config: &Config,
    prefix: &str,
    intents: impl Iterator<Item = Intent>,
) -> (Vec<PendingIntent>, intent::ReplayResult) {
    let pending: Vec<PendingIntent> = intents
        .enumerate()
        .map(|(key, intent)| PendingIntent {
            key: format!("{prefix}{key}"),
            intent,
        })
        .collect();
    let replayed = intent::replay(snapshot, config, &pending);
    (pending, replayed)
}

/// Replays the confirmed `removals` on `snapshot`, then plans the rule
/// changes on that state and replays them (spec sections 10 and 16), so a
/// removed tag never justifies an added one.
///
/// A removal or change whose replay conflicts, such as a rule account that is
/// not open on the transaction's date, keeps its conflict in
/// [`Applied::removals`] or [`Applied::changes`] and is left out of the
/// snapshot; the others apply.
#[must_use]
pub fn apply(
    snapshot: &Snapshot,
    config: &Config,
    rules: &Rules,
    taxonomy: Option<&Taxonomy>,
    options: &ReapplyOptions,
    removals: &[TagRemoval],
) -> Applied {
    let (removed, first) = replay_all(
        snapshot,
        config,
        "r",
        removals.iter().map(TagRemoval::intent),
    );
    let ledger = ledger::load(&first.snapshot, &config.main_file);
    let changes = plan(&ledger, config, rules, taxonomy, options);
    let (planned, second) = replay_all(
        &first.snapshot,
        config,
        "c",
        changes.iter().map(|change| change.intent.clone()),
    );
    let intents: Vec<PendingIntent> = removed.into_iter().chain(planned).collect();
    let outcomes: Vec<(String, Outcome)> = first
        .outcomes
        .iter()
        .chain(&second.outcomes)
        .cloned()
        .collect();
    Applied {
        subject: intent::summary(&intents, &outcomes),
        snapshot: second.snapshot,
        removals: removals
            .iter()
            .cloned()
            .zip(first.outcomes.into_iter().map(|(_, outcome)| outcome))
            .collect(),
        changes: changes
            .into_iter()
            .zip(second.outcomes.into_iter().map(|(_, outcome)| outcome))
            .collect(),
    }
}

/// Why the rules cannot be re-applied.
#[derive(Clone, Debug, Error)]
pub enum ReapplyError {
    /// The sync failed; nothing was committed.
    #[error(transparent)]
    Sync(#[from] SyncError),
    /// The fetched `lentil.toml` is invalid. Only reachable for the admin
    /// role, which skips the read-only check of the sync.
    #[error(transparent)]
    Config(ConfigError),
    /// The fetched `rules.toml` is invalid or books to a source account.
    #[error(transparent)]
    Rules(RulesError),
    /// The fetched `tags.toml` is invalid (spec section 16).
    #[error(transparent)]
    Taxonomy(TaxonomyError),
}

/// Re-applies the rules as an editor commit (spec sections 11 and 16).
///
/// Every attempt of the sync reads `lentil.toml`, `rules.toml` and
/// `tags.toml` from the freshly fetched state and [`apply`]s the confirmed
/// `removals` and the rules to it, so the changes and outcomes returned are
/// those of the state that was pushed. The removals are replayed as they
/// were confirmed and never planned again. If every removal and change
/// conflicts, or there is none, no commit is made.
///
/// # Errors
///
/// See [`ReapplyError`].
pub fn sync<G>(
    git: &mut G,
    sleeper: &mut dyn Sleeper,
    identity: &Identity,
    author: &Author,
    options: &ReapplyOptions,
    removals: &[TagRemoval],
) -> Result<SyncOutcome<Reapplied>, ReapplyError>
where
    G: GitBackend + ?Sized,
{
    let synced = run_sync(git, sleeper, identity, author, |fresh| {
        let inputs = Config::parse(fresh.text(CONFIG_FILE).unwrap_or_default())
            .map_err(ReapplyError::Config)
            .and_then(|config| {
                Rules::load(fresh, &config)
                    .map(|rules| (config, rules))
                    .map_err(ReapplyError::Rules)
            })
            .and_then(|(config, rules)| {
                Taxonomy::load(fresh)
                    .transpose()
                    .map(|taxonomy| (config, rules, taxonomy))
                    .map_err(ReapplyError::Taxonomy)
            });
        match inputs {
            Ok((config, rules, taxonomy)) => {
                let Applied {
                    snapshot,
                    subject,
                    removals,
                    changes,
                } = apply(fresh, &config, &rules, taxonomy.as_ref(), options, removals);
                Work::Commit {
                    after: snapshot,
                    subject,
                    result: Ok(Reapplied { removals, changes }),
                }
            }
            Err(error) => Work::Nothing(Err(error)),
        }
    })?;
    Ok(SyncOutcome {
        result: synced.result?,
        commit: synced.commit,
        attempts: synced.attempts,
        warnings: synced.warnings,
    })
}

/// Index and account of the only posting of `txn` that is not on a source
/// account, or `None` if there is none or more than one.
pub(crate) fn counter_posting<'t>(txn: &'t Txn, config: &Config) -> Option<(usize, &'t str)> {
    let mut counters = txn
        .postings
        .iter()
        .enumerate()
        .filter(|(_, p)| !config.is_source_account(&p.account));
    match (counters.next(), counters.next()) {
        (Some((index, posting)), None) => Some((index, posting.account.as_str())),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::intent::{Conflict, ConflictCode};
    use crate::ledger;
    use crate::snapshot::{Change, Snapshot};
    use crate::tags::Taxonomy;

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

    const MAIN: &str = "option \"operating_currency\" \"EUR\"\ninclude \"accounts.beancount\"\ninclude \"imports/*/*/*.beancount\"\ninclude \"manual/*/*.beancount\"\n";

    const ACCOUNTS: &str = "2020-01-01 open Assets:Bank:Giro EUR
2020-01-01 open Liabilities:Card EUR
2020-01-01 open Assets:Transfer
2020-01-01 open Equity:OpeningBalances
2020-01-01 open Expenses:Uncategorized
2020-01-01 open Expenses:Food
2020-01-01 open Expenses:Home
";

    const RULES: &str = r#"
[[rule]]
match = { source = "giro", counterparty = "^CORNER GROCERY" }
payee = "Corner Grocery"
account = "Expenses:Food"

[[rule]]
match = { purpose = "(?i)rent" }
account = "Expenses:Home"

[[rule]]
match = { counterparty = "^$" }
payee = "Unknown"
"#;

    const GIRO_RUN: &str = "imports/giro/2026/20261020T071502Z.beancount";
    const CARD_RUN: &str = "imports/card/2026/20261020T071502Z.beancount";
    const CASH_RUN: &str = "imports/cash/2026/20261020T071502Z.beancount";
    const MANUAL: &str = "manual/k7p2q9xd/2026-10.beancount";

    /// A transaction with the given header strings, ID and original fields.
    fn txn(header: &str, id: Option<&str>, original: &[(&str, &str)], postings: &str) -> String {
        let id = id.map_or(String::new(), |id| format!("  lentil_id: \"{id}\"\n"));
        let original: String = original
            .iter()
            .map(|(key, value)| format!("  lentil_{key}: \"{value}\"\n"))
            .collect();
        format!("{header}\n{id}{original}{postings}\n")
    }

    const GIRO_TO_UNSORTED: &str = "  Assets:Bank:Giro  -10.00 EUR\n  Expenses:Uncategorized\n";

    fn giro_run() -> String {
        [
            // Line 1: the opening transaction, which the third rule would match.
            txn(
                "2026-10-01 ! \"Opening balance\"",
                Some(&opening_id("giro")),
                &[],
                "  Assets:Bank:Giro  100.00 EUR\n  Equity:OpeningBalances\n",
            ),
            // Line 6: open, payee and account change.
            txn(
                "2026-10-02 ! \"CORNER GROCERY 4711\" \"Card payment\"",
                Some("giro-000000000002"),
                &[
                    ("counterparty", "CORNER GROCERY 4711"),
                    ("purpose", "Card payment"),
                ],
                GIRO_TO_UNSORTED,
            ),
            // Line 13: reviewed, changes only with `reviewed`.
            txn(
                "2026-10-03 * \"Bakery\" \"\"",
                Some("giro-000000000003"),
                &[("counterparty", "CORNER GROCERY 12")],
                GIRO_TO_UNSORTED,
            ),
            // Line 19: the rule sets only the account, which is right already.
            txn(
                "2026-10-04 ! \"Housing Co-op\" \"Rent October\"",
                Some("giro-000000000004"),
                &[("counterparty", "HOUSING"), ("purpose", "Rent October")],
                "  Assets:Bank:Giro  -500.00 EUR\n  Expenses:Home\n",
            ),
            // Line 26: the counter posting comes first.
            txn(
                "2026-10-05 ! \"Landlord\" \"RENT\"",
                Some("giro-000000000005"),
                &[("counterparty", "LANDLORD"), ("purpose", "RENT")],
                "  Expenses:Food  500.00 EUR\n  Assets:Bank:Giro\n",
            ),
            // Line 33: split over two counter postings, so only the payee changes.
            txn(
                "2026-10-06 ! \"CORNER GROCERY 4711\" \"\"",
                Some("giro-000000000006"),
                &[("counterparty", "CORNER GROCERY 4711")],
                "  Assets:Bank:Giro  -20.00 EUR\n  Expenses:Food  10.00 EUR\n  Expenses:Home\n",
            ),
            // Line 40: no payee and no counterparty.
            txn(
                "2026-10-07 ! \"Salary\"",
                Some("giro-000000000007"),
                &[("purpose", "Salary")],
                "  Assets:Bank:Giro  1000.00 EUR\n  Expenses:Uncategorized\n",
            ),
            // No `lentil_id`.
            txn(
                "2026-10-08 ! \"CORNER GROCERY 4711\" \"\"",
                None,
                &[("counterparty", "CORNER GROCERY 4711")],
                GIRO_TO_UNSORTED,
            ),
            // Dismissed.
            "; 2026-10-09 ! \"CORNER GROCERY 4711\" \"\"\n;   lentil_id: \"giro-000000000009\"\n;   lentil_counterparty: \"CORNER GROCERY 4711\"\n;   Assets:Bank:Giro  -10.00 EUR\n;   Expenses:Uncategorized\n\n".to_owned(),
            // A transfer, whose counter posting is `Assets:Transfer`.
            txn(
                "2026-10-10 ! \"Card\" \"RENT\"",
                Some("giro-000000000010"),
                &[("counterparty", "CARD"), ("purpose", "RENT")],
                "  Assets:Bank:Giro  -50.00 EUR\n  Assets:Transfer\n",
            ),
        ]
        .concat()
    }

    fn snapshot() -> Snapshot {
        // The first rule is limited to giro, and the third does not match a
        // counterparty; neither applies to the card.
        let card = txn(
            "2026-10-02 ! \"CORNER GROCERY 4711\" \"\"",
            Some("card-000000000001"),
            &[("counterparty", "CORNER GROCERY 4711")],
            "  Liabilities:Card  -10.00 EUR\n  Expenses:Uncategorized\n",
        );
        // No source `cash` is configured, and manual entries are no imports.
        let other = txn(
            "2026-10-02 ! \"Corner\" \"\"",
            Some("cash-000000000001"),
            &[],
            "  Assets:Transfer  -10.00 EUR\n  Expenses:Uncategorized\n",
        );
        let manual = txn(
            "2026-10-02 ! \"Corner\" \"\"",
            Some("k7p2q9xd-01JBQ0000000000000000000000"),
            &[],
            "  Assets:Transfer  -10.00 EUR\n  Expenses:Uncategorized\n",
        );
        Snapshot::from_files([
            ("lentil.toml", CONFIG.to_owned()),
            ("main.beancount", MAIN.to_owned()),
            ("accounts.beancount", ACCOUNTS.to_owned()),
            (GIRO_RUN, giro_run()),
            (CARD_RUN, card),
            (CASH_RUN, other),
            (MANUAL, manual),
        ])
    }

    fn load(snapshot: &Snapshot) -> Ledger {
        let ledger = ledger::load(snapshot, "main.beancount");
        assert!(ledger.errors.is_empty(), "{:?}", ledger.errors);
        ledger
    }

    fn planned(snapshot: &Snapshot, reviewed: bool, fields: ReapplyFields) -> Vec<RuleChange> {
        plan(
            &load(snapshot),
            &Config::parse(CONFIG).unwrap(),
            &Rules::parse(RULES).unwrap(),
            None,
            &ReapplyOptions { reviewed, fields },
        )
    }

    /// ID, line, field, old and new value of each change.
    fn summary(changes: &[RuleChange]) -> Vec<(&str, u32, Field, Option<&str>, &str)> {
        changes
            .iter()
            .map(|c| {
                (
                    c.id.as_str(),
                    c.line,
                    c.field,
                    c.old.as_deref(),
                    c.new.as_str(),
                )
            })
            .collect()
    }

    const GROCERY_PAYEE: (&str, u32, Field, Option<&str>, &str) = (
        "giro-000000000002",
        6,
        Field::Payee,
        Some("CORNER GROCERY 4711"),
        "Corner Grocery",
    );
    const GROCERY_ACCOUNT: (&str, u32, Field, Option<&str>, &str) = (
        "giro-000000000002",
        6,
        Field::Account,
        Some("Expenses:Uncategorized"),
        "Expenses:Food",
    );
    const REVIEWED_PAYEE: (&str, u32, Field, Option<&str>, &str) = (
        "giro-000000000003",
        13,
        Field::Payee,
        Some("Bakery"),
        "Corner Grocery",
    );
    const REVIEWED_ACCOUNT: (&str, u32, Field, Option<&str>, &str) = (
        "giro-000000000003",
        13,
        Field::Account,
        Some("Expenses:Uncategorized"),
        "Expenses:Food",
    );
    const RENT_ACCOUNT: (&str, u32, Field, Option<&str>, &str) = (
        "giro-000000000005",
        26,
        Field::Account,
        Some("Expenses:Food"),
        "Expenses:Home",
    );
    const SPLIT_PAYEE: (&str, u32, Field, Option<&str>, &str) = (
        "giro-000000000006",
        33,
        Field::Payee,
        Some("CORNER GROCERY 4711"),
        "Corner Grocery",
    );
    const SALARY_PAYEE: (&str, u32, Field, Option<&str>, &str) =
        ("giro-000000000007", 40, Field::Payee, None, "Unknown");
    const TRANSFER_ACCOUNT: (&str, u32, Field, Option<&str>, &str) = (
        "giro-000000000010",
        57,
        Field::Account,
        Some("Assets:Transfer"),
        "Expenses:Home",
    );

    #[test]
    fn changes_open_imported_transactions_by_default() {
        let changes = planned(&snapshot(), false, ReapplyFields::All);
        assert_eq!(
            summary(&changes),
            vec![
                GROCERY_PAYEE,
                GROCERY_ACCOUNT,
                RENT_ACCOUNT,
                SPLIT_PAYEE,
                SALARY_PAYEE,
                TRANSFER_ACCOUNT,
            ]
        );
        assert_eq!(
            changes[0],
            RuleChange {
                id: "giro-000000000002".into(),
                file: GIRO_RUN.into(),
                line: 6,
                date: "2026-10-02".parse().unwrap(),
                text: "CORNER GROCERY 4711 | Card payment".into(),
                field: Field::Payee,
                old: Some("CORNER GROCERY 4711".into()),
                new: "Corner Grocery".into(),
                intent: Intent::SetPayee {
                    id: "giro-000000000002".into(),
                    old: Some("CORNER GROCERY 4711".into()),
                    new: "Corner Grocery".into(),
                },
            }
        );
        let intents: Vec<&Intent> = changes.iter().map(|c| &c.intent).collect();
        assert_eq!(
            intents[1],
            &Intent::SetAccount {
                id: "giro-000000000002".into(),
                posting: 1,
                old: "Expenses:Uncategorized".into(),
                new: "Expenses:Food".into(),
            }
        );
        assert_eq!(
            intents[2],
            &Intent::SetAccount {
                id: "giro-000000000005".into(),
                posting: 0,
                old: "Expenses:Food".into(),
                new: "Expenses:Home".into(),
            }
        );
        assert_eq!(
            intents[4],
            &Intent::SetPayee {
                id: "giro-000000000007".into(),
                old: None,
                new: "Unknown".into(),
            }
        );
    }

    #[test]
    fn reviewed_transactions_change_only_when_asked() {
        let changes = planned(&snapshot(), true, ReapplyFields::All);
        assert_eq!(
            summary(&changes),
            vec![
                GROCERY_PAYEE,
                GROCERY_ACCOUNT,
                REVIEWED_PAYEE,
                REVIEWED_ACCOUNT,
                RENT_ACCOUNT,
                SPLIT_PAYEE,
                SALARY_PAYEE,
                TRANSFER_ACCOUNT,
            ]
        );
    }

    #[test]
    fn changes_only_the_chosen_field() {
        let payees = planned(&snapshot(), true, ReapplyFields::PayeeOnly);
        assert_eq!(
            summary(&payees),
            vec![GROCERY_PAYEE, REVIEWED_PAYEE, SPLIT_PAYEE, SALARY_PAYEE]
        );
        let accounts = planned(&snapshot(), false, ReapplyFields::AccountOnly);
        assert_eq!(
            summary(&accounts),
            vec![GROCERY_ACCOUNT, RENT_ACCOUNT, TRANSFER_ACCOUNT]
        );
    }

    #[test]
    fn other_flags_never_change() {
        let mut snapshot = snapshot();
        let run = giro_run().replace("2026-10-02 ! ", "2026-10-02 P ");
        snapshot.insert(GIRO_RUN, run);
        let changes = planned(&snapshot, true, ReapplyFields::All);
        assert!(
            changes.iter().all(|c| c.id != "giro-000000000002"),
            "{:?}",
            summary(&changes)
        );
    }

    #[test]
    fn no_rules_change_nothing() {
        let snapshot = snapshot();
        let changes = plan(
            &load(&snapshot),
            &Config::parse(CONFIG).unwrap(),
            &Rules::default(),
            None,
            &ReapplyOptions {
                reviewed: true,
                fields: ReapplyFields::All,
            },
        );
        assert_eq!(changes, Vec::new());
    }

    fn applied(snapshot: &Snapshot, rules: &str) -> Applied {
        let options = ReapplyOptions {
            reviewed: true,
            fields: ReapplyFields::All,
        };
        apply(
            snapshot,
            &Config::parse(CONFIG).unwrap(),
            &Rules::parse(rules).unwrap(),
            None,
            &options,
            &[],
        )
    }

    #[test]
    fn applying_the_plan_changes_only_the_run_file_and_a_second_plan_is_empty() {
        let before = snapshot();
        let applied = applied(&before, RULES);
        assert_eq!(
            applied
                .changes
                .iter()
                .map(|(c, _)| c.clone())
                .collect::<Vec<_>>(),
            planned(&before, true, ReapplyFields::All)
        );
        assert!(
            applied
                .changes
                .iter()
                .all(|(_, outcome)| *outcome == Outcome::Applied),
            "{:?}",
            applied.changes
        );
        assert_eq!(applied.subject, "set-account 4, set-payee 4");
        let after = load(&applied.snapshot);
        let grocery = after.find("giro-000000000002").unwrap();
        assert_eq!(grocery.payee.as_deref(), Some("Corner Grocery"));
        assert_eq!(grocery.postings[1].account, "Expenses:Food");
        assert_eq!(grocery.flag, '!', "rules apply never confirms");
        assert_eq!(after.find("giro-000000000003").unwrap().flag, '*');
        assert_eq!(
            planned(&applied.snapshot, true, ReapplyFields::All),
            Vec::new()
        );
        assert_eq!(
            applied.snapshot.changes_from(&before),
            vec![Change::Modified(GIRO_RUN.into())]
        );
    }

    #[test]
    fn a_conflicting_change_is_kept_out_and_the_others_apply() {
        // `Expenses:Closed` is never opened.
        let rules = "[[rule]]\nmatch = { purpose = \"(?i)rent\" }\naccount = \"Expenses:Closed\"\n\n[[rule]]\nmatch = { counterparty = \"^$\" }\npayee = \"Unknown\"\n";
        let before = snapshot();
        let applied = applied(&before, rules);
        let outcomes: Vec<(&str, Field, &Outcome)> = applied
            .changes
            .iter()
            .map(|(c, outcome)| (c.id.as_str(), c.field, outcome))
            .collect();
        let not_open = Outcome::Conflict(Conflict {
            code: ConflictCode::AccountNotOpen,
            current: None,
        });
        assert_eq!(
            outcomes,
            vec![
                ("giro-000000000004", Field::Account, &not_open),
                ("giro-000000000005", Field::Account, &not_open),
                ("giro-000000000007", Field::Payee, &Outcome::Applied),
                ("giro-000000000010", Field::Account, &not_open),
            ]
        );
        assert_eq!(applied.subject, "set-payee 1");
        let after = load(&applied.snapshot);
        assert_eq!(
            after.find("giro-000000000004").unwrap().postings[1].account,
            "Expenses:Home"
        );
        assert_eq!(
            after.find("giro-000000000007").unwrap().payee.as_deref(),
            Some("Unknown")
        );
    }

    #[test]
    fn without_changes_the_state_stays_as_it_is() {
        let before = snapshot();
        let applied = applied(&before, "");
        assert_eq!(
            applied,
            Applied {
                snapshot: before,
                subject: String::new(),
                removals: Vec::new(),
                changes: Vec::new(),
            }
        );
    }

    const TAG_RULES: &str = "[[rule]]\nmatch = { counterparty = \"^STREAMFLIX\" }\nadd_tags = [\"recurring\"]\n\n[[rule]]\nmatch = { tags = [\"recurring\"] }\nadd_tags = [\"subscription\"]\n\n[[rule]]\nmatch = { counterparty = \"^CORNER GROCERY\" }\npayee = \"Corner Grocery\"\nadd_tags = [\"groceries\"]\n";
    const TAXONOMY: &str = "exclusive = [[\"recurring\", \"irregular\"]]\n";

    /// The transactions of [`tag_snapshot`], by name.
    const TAGGED: [(&str, &str); 6] = [
        ("G", "giro-000000000101"),
        ("S1", "giro-000000000102"),
        ("S2", "giro-000000000103"),
        ("F", "giro-000000000104"),
        ("O", "giro-000000000105"),
        ("C", "giro-000000000106"),
    ];

    fn tagged(name: &str) -> &'static str {
        TAGGED.iter().find(|(n, _)| *n == name).unwrap().1
    }

    fn name_of(id: &str) -> &'static str {
        TAGGED.iter().find(|(_, i)| *i == id).unwrap().0
    }

    fn tag_snapshot() -> Snapshot {
        let run: String = [
            (
                "2026-10-02 ! \"CORNER GROCERY 4711\" \"Card payment\"",
                "CORNER GROCERY 4711",
            ),
            ("2026-10-03 ! \"STREAMFLIX\" \"\" #irregular", "STREAMFLIX"),
            ("2026-10-04 * \"STREAMFLIX\" \"\"", "STREAMFLIX"),
            (
                "2026-10-05 ! \"CORNER GROCERY 12\" \"\" #subscription #friends",
                "CORNER GROCERY 12",
            ),
            ("2026-10-06 ! \"OLDSTREAM\" \"\" #recurring", "OLDSTREAM"),
            (
                "2026-10-07 ! \"STREAMFLIX\" \"\" #recurring #irregular",
                "STREAMFLIX",
            ),
        ]
        .iter()
        .zip(TAGGED)
        .map(|((header, counterparty), (_, id))| {
            txn(
                header,
                Some(id),
                &[("counterparty", counterparty)],
                GIRO_TO_UNSORTED,
            )
        })
        .collect();
        Snapshot::from_files([
            ("lentil.toml", CONFIG.to_owned()),
            ("main.beancount", MAIN.to_owned()),
            ("accounts.beancount", ACCOUNTS.to_owned()),
            (GIRO_RUN, run),
            (MANUAL, String::new()),
        ])
    }

    fn options(reviewed: bool, fields: ReapplyFields) -> ReapplyOptions {
        ReapplyOptions { reviewed, fields }
    }

    fn taxonomy() -> Taxonomy {
        Taxonomy::parse(TAXONOMY).unwrap()
    }

    /// Name, field and new value of each change the tag rules plan.
    fn tag_plan(
        snapshot: &Snapshot,
        taxonomy: Option<&Taxonomy>,
        options: &ReapplyOptions,
    ) -> Vec<(&'static str, Field, String)> {
        plan(
            &load(snapshot),
            &Config::parse(CONFIG).unwrap(),
            &Rules::parse(TAG_RULES).unwrap(),
            taxonomy,
            options,
        )
        .into_iter()
        .map(|c| (name_of(&c.id), c.field, c.new))
        .collect()
    }

    fn expect(changes: &[(&'static str, Field, &str)]) -> Vec<(&'static str, Field, String)> {
        changes
            .iter()
            .map(|(name, field, new)| (*name, *field, (*new).to_owned()))
            .collect()
    }

    #[test]
    fn adds_the_tags_of_the_evaluation_after_payee_and_account() {
        let changes = plan(
            &load(&tag_snapshot()),
            &Config::parse(CONFIG).unwrap(),
            &Rules::parse(TAG_RULES).unwrap(),
            Some(&taxonomy()),
            &options(false, ReapplyFields::All),
        );
        assert_eq!(
            changes
                .iter()
                .map(|c| (name_of(&c.id), c.field, c.new.clone()))
                .collect::<Vec<_>>(),
            expect(&[
                ("G", Field::Payee, "Corner Grocery"),
                ("G", Field::Tag, "groceries"),
                ("F", Field::Payee, "Corner Grocery"),
                ("F", Field::Tag, "groceries"),
                ("O", Field::Tag, "subscription"),
                ("C", Field::Tag, "subscription"),
            ])
        );
        let groceries = &changes[1];
        assert_eq!(groceries.old, None);
        assert_eq!(
            groceries.intent,
            Intent::AddTag {
                id: tagged("G").into(),
                tag: "groceries".into()
            }
        );
        assert_eq!(
            (groceries.line, groceries.text.as_str()),
            (1, "CORNER GROCERY 4711 | Card payment")
        );
    }

    #[test]
    fn reviewed_transactions_get_tags_only_when_asked() {
        assert_eq!(
            tag_plan(
                &tag_snapshot(),
                Some(&taxonomy()),
                &options(true, ReapplyFields::All)
            ),
            expect(&[
                ("G", Field::Payee, "Corner Grocery"),
                ("G", Field::Tag, "groceries"),
                ("S2", Field::Tag, "recurring"),
                ("S2", Field::Tag, "subscription"),
                ("F", Field::Payee, "Corner Grocery"),
                ("F", Field::Tag, "groceries"),
                ("O", Field::Tag, "subscription"),
                ("C", Field::Tag, "subscription"),
            ])
        );
    }

    #[test]
    fn without_tags_toml_nothing_is_held_back() {
        assert_eq!(
            tag_plan(&tag_snapshot(), None, &options(false, ReapplyFields::All)),
            expect(&[
                ("G", Field::Payee, "Corner Grocery"),
                ("G", Field::Tag, "groceries"),
                ("S1", Field::Tag, "recurring"),
                ("S1", Field::Tag, "subscription"),
                ("F", Field::Payee, "Corner Grocery"),
                ("F", Field::Tag, "groceries"),
                ("O", Field::Tag, "subscription"),
                ("C", Field::Tag, "subscription"),
            ])
        );
    }

    #[test]
    fn only_tags_changes_only_tags() {
        let snapshot = tag_snapshot();
        assert_eq!(
            tag_plan(
                &snapshot,
                Some(&taxonomy()),
                &options(false, ReapplyFields::TagsOnly)
            ),
            expect(&[
                ("G", Field::Tag, "groceries"),
                ("F", Field::Tag, "groceries"),
                ("O", Field::Tag, "subscription"),
                ("C", Field::Tag, "subscription"),
            ])
        );
        for fields in [ReapplyFields::PayeeOnly, ReapplyFields::AccountOnly] {
            assert!(
                tag_plan(&snapshot, Some(&taxonomy()), &options(true, fields))
                    .iter()
                    .all(|(_, field, _)| *field != Field::Tag),
                "{fields:?}"
            );
        }
    }

    fn candidates(fields: ReapplyFields) -> Vec<TagRemoval> {
        removal_candidates(
            &load(&tag_snapshot()),
            &Config::parse(CONFIG).unwrap(),
            &Rules::parse(TAG_RULES).unwrap(),
            Some(&taxonomy()),
            &options(false, fields),
        )
    }

    #[test]
    fn removal_candidates_are_rule_tags_the_evaluation_no_longer_yields() {
        let found = candidates(ReapplyFields::All);
        assert_eq!(
            found
                .iter()
                .map(|r| (name_of(&r.id), r.tag.as_str()))
                .collect::<Vec<_>>(),
            vec![
                ("F", "subscription"),
                ("O", "recurring"),
                ("C", "recurring")
            ]
        );
        assert_eq!(
            found[1],
            TagRemoval {
                id: tagged("O").into(),
                file: GIRO_RUN.into(),
                line: 25,
                date: "2026-10-06".parse().unwrap(),
                text: "OLDSTREAM".into(),
                tag: "recurring".into(),
            }
        );
        assert_eq!(
            found[1].intent(),
            Intent::RemoveTag {
                id: tagged("O").into(),
                tag: "recurring".into()
            }
        );
        assert_eq!(candidates(ReapplyFields::TagsOnly), found);
        assert_eq!(candidates(ReapplyFields::PayeeOnly), Vec::new());
        assert_eq!(candidates(ReapplyFields::AccountOnly), Vec::new());
    }

    #[test]
    fn rule_tags_the_rules_still_yield_are_no_candidates() {
        let mut snapshot = tag_snapshot();
        let run = snapshot.text(GIRO_RUN).unwrap().replacen(
            "\"STREAMFLIX\" \"\" #irregular",
            "\"STREAMFLIX\" \"\" #recurring #subscription",
            1,
        );
        snapshot.insert(GIRO_RUN, run);
        let found = removal_candidates(
            &load(&snapshot),
            &Config::parse(CONFIG).unwrap(),
            &Rules::parse(TAG_RULES).unwrap(),
            Some(&taxonomy()),
            &options(false, ReapplyFields::All),
        );
        assert_eq!(
            found
                .iter()
                .map(|r| (name_of(&r.id), r.tag.as_str()))
                .collect::<Vec<_>>(),
            vec![
                ("F", "subscription"),
                ("O", "recurring"),
                ("C", "recurring")
            ]
        );
    }

    #[test]
    fn tags_to_add_are_computed_after_the_confirmed_removals() {
        let before = tag_snapshot();
        let config = Config::parse(CONFIG).unwrap();
        let rules = Rules::parse(TAG_RULES).unwrap();
        let options = options(false, ReapplyFields::All);
        let removal = candidates(ReapplyFields::All).remove(1);
        let applied = apply(
            &before,
            &config,
            &rules,
            Some(&taxonomy()),
            &options,
            std::slice::from_ref(&removal),
        );
        assert_eq!(applied.removals, vec![(removal, Outcome::Applied)]);
        assert!(
            !applied
                .changes
                .iter()
                .any(|(c, _)| c.id == tagged("O") && c.new == "subscription"),
            "{:?}",
            applied.changes
        );
        assert!(
            applied
                .snapshot
                .text(GIRO_RUN)
                .unwrap()
                .contains("2026-10-06 ! \"OLDSTREAM\" \"\"\n")
        );
        assert_eq!(applied.subject, "set-payee 2, add-tag 3, remove-tag 1");

        let without = apply(&before, &config, &rules, Some(&taxonomy()), &options, &[]);
        assert!(
            without
                .changes
                .iter()
                .any(|(c, _)| c.id == tagged("O") && c.new == "subscription")
        );
        assert_eq!(without.removals, Vec::new());
    }

    #[test]
    fn applying_tag_changes_then_planning_again_finds_nothing() {
        let before = tag_snapshot();
        let config = Config::parse(CONFIG).unwrap();
        let rules = Rules::parse(TAG_RULES).unwrap();
        let options = options(true, ReapplyFields::All);
        let applied = apply(&before, &config, &rules, Some(&taxonomy()), &options, &[]);
        assert!(
            applied
                .changes
                .iter()
                .all(|(_, outcome)| *outcome == Outcome::Applied)
        );
        assert_eq!(
            tag_plan(&applied.snapshot, Some(&taxonomy()), &options),
            Vec::new()
        );
        let flags = |snapshot: &Snapshot| {
            load(snapshot)
                .transactions
                .iter()
                .map(|t| t.flag)
                .collect::<Vec<_>>()
        };
        assert_eq!(flags(&applied.snapshot), flags(&before));
    }
}
