//! Re-applying the rules to imported transactions (spec section 16, PRD
//! FR-C19).
//!
//! The plan is a list of editor intents: `set-payee` and `set-account`, never
//! a flag change. [`apply`] replays them on the state the plan was made from,
//! and [`sync`] plans anew on every state it fetches, so a concurrent push
//! never turns its changes into conflicts of their own.

use jiff::civil::Date;
use thiserror::Error;

use crate::config::{CONFIG_FILE, Config, ConfigError};
use crate::import::rules::RULES_FILE;
use crate::import::{Original, Rules, RulesError, opening_id};
use crate::intent::{self, Intent, Outcome, PendingIntent};
use crate::ledger::{self, Ledger, Txn};
use crate::snapshot::Snapshot;
use crate::status::display_text;
use crate::sync::{Author, GitBackend, Identity, Sleeper, SyncError, SyncOutcome, Work, run_sync};

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
    /// Payee and counter account.
    #[default]
    Both,
    /// Only the payee.
    PayeeOnly,
    /// Only the counter account.
    AccountOnly,
}

impl ReapplyFields {
    /// Whether the rules may set the payee.
    fn payee(self) -> bool {
        self != Self::AccountOnly
    }

    /// Whether the rules may set the counter account.
    fn account(self) -> bool {
        self != Self::PayeeOnly
    }
}

/// A field a rule sets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    /// The payee.
    Payee,
    /// The counter account.
    Account,
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

/// The changes the rules make to the imported transactions of `ledger`, in
/// ledger order, the payee before the account of the same transaction.
///
/// Only active transactions with a `lentil_id` in the run file of a
/// configured source count, except the source's opening balance transaction.
/// Of those, open (`!`) ones always, and reviewed (`*`) ones only with
/// [`ReapplyOptions::reviewed`]. The rules see the transaction's original
/// fields and the run file's source, as at import. A field changes only if a
/// rule sets it to another value, and the account only if exactly one posting
/// is not on a source account.
#[must_use]
pub fn plan(
    ledger: &Ledger,
    config: &Config,
    rules: &Rules,
    options: &ReapplyOptions,
) -> Vec<RuleChange> {
    let mut changes = Vec::new();
    for txn in &ledger.transactions {
        let Some(id) = txn.id.as_deref() else {
            continue;
        };
        let Some(source) = config.run_source(&txn.file) else {
            continue;
        };
        let flag_selected = txn.flag == '!' || (options.reviewed && txn.flag == '*');
        if !flag_selected || id == opening_id(&source.id) {
            continue;
        }
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
    }
    changes
}

/// The rule changes of one repository state, replayed as editor intents.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Applied {
    /// The state with every change whose replay applied.
    pub snapshot: Snapshot,
    /// Commit subject: the applied operations, such as
    /// `set-account 3, set-payee 2` (spec section 12).
    pub subject: String,
    /// Each planned change with the outcome of its replay, in plan order.
    pub changes: Vec<(RuleChange, Outcome)>,
}

/// Plans the rule changes on `snapshot` and replays them (spec section 10).
///
/// A change whose replay conflicts, such as a rule account that is not open
/// on the transaction's date, keeps its conflict in [`Applied::changes`] and
/// is left out of the snapshot; the others apply.
#[must_use]
pub fn apply(
    snapshot: &Snapshot,
    config: &Config,
    rules: &Rules,
    options: &ReapplyOptions,
) -> Applied {
    let ledger = ledger::load(snapshot, &config.main_file);
    let changes = plan(&ledger, config, rules, options);
    let intents: Vec<PendingIntent> = changes
        .iter()
        .enumerate()
        .map(|(key, change)| PendingIntent {
            key: key.to_string(),
            intent: change.intent.clone(),
        })
        .collect();
    let replayed = intent::replay(snapshot, config, &intents);
    Applied {
        subject: intent::summary(&intents, &replayed.outcomes),
        snapshot: replayed.snapshot,
        changes: changes
            .into_iter()
            .zip(replayed.outcomes.into_iter().map(|(_, outcome)| outcome))
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
}

/// Re-applies the rules as an editor commit (spec sections 11 and 16).
///
/// Every attempt of the sync reads `lentil.toml` and `rules.toml` from the
/// freshly fetched state and [`apply`]s the rules to it, so the changes and
/// outcomes returned are those of the state that was pushed. If every change
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
) -> Result<SyncOutcome<Vec<(RuleChange, Outcome)>>, ReapplyError>
where
    G: GitBackend + ?Sized,
{
    let synced = run_sync(git, sleeper, identity, author, |fresh| {
        let inputs = Config::parse(fresh.text(CONFIG_FILE).unwrap_or_default())
            .map_err(ReapplyError::Config)
            .and_then(|config| {
                Rules::parse(fresh.text(RULES_FILE).unwrap_or_default())
                    .and_then(|rules| rules.check_accounts(&config).map(|()| rules))
                    .map(|rules| (config, rules))
                    .map_err(ReapplyError::Rules)
            });
        match inputs {
            Ok((config, rules)) => {
                let Applied {
                    snapshot,
                    subject,
                    changes,
                } = apply(fresh, &config, &rules, options);
                Work::Commit {
                    after: snapshot,
                    subject,
                    result: Ok(changes),
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
fn counter_posting<'t>(txn: &'t Txn, config: &Config) -> Option<(usize, &'t str)> {
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
        let changes = planned(&snapshot(), false, ReapplyFields::Both);
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
        let changes = planned(&snapshot(), true, ReapplyFields::Both);
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
        let changes = planned(&snapshot, true, ReapplyFields::Both);
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
            &ReapplyOptions {
                reviewed: true,
                fields: ReapplyFields::Both,
            },
        );
        assert_eq!(changes, Vec::new());
    }

    fn applied(snapshot: &Snapshot, rules: &str) -> Applied {
        let options = ReapplyOptions {
            reviewed: true,
            fields: ReapplyFields::Both,
        };
        apply(
            snapshot,
            &Config::parse(CONFIG).unwrap(),
            &Rules::parse(rules).unwrap(),
            &options,
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
            planned(&before, true, ReapplyFields::Both)
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
            planned(&applied.snapshot, true, ReapplyFields::Both),
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
                changes: Vec::new(),
            }
        );
    }
}
