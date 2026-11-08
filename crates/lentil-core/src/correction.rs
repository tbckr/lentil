//! The intents of one account correction (spec sections 10 and 16, PRD
//! FR-D3, FR-D14, FR-D15).
//!
//! When the user changes a transaction's account, [`correct`] computes, in
//! one call, the user's own `set-account` and `confirm`, an `add-rule` from
//! the words the user picked, and `suggest-account` for the other open
//! transactions the new rule matches and for the open transactions on
//! `fallback_account` that now get a learned suggestion. The suggestions are
//! computed once, on the state after the user's own intents, and the client
//! stores them as intents like any other. [`preview`] counts what the rule
//! would move while the user still picks its words.
//!
//! The learned refresh sees only the account, so it also moves an open
//! transaction off `fallback_account` where a rule of `rules.toml` books to
//! `fallback_account` on purpose.

use std::collections::{HashMap, HashSet};

use jiff::civil::Date;
use thiserror::Error;

use crate::config::Config;
use crate::import::{EditorRule, Original, Rules};
use crate::intent::{self, Intent, PendingIntent};
use crate::learn::{Learner, words};
use crate::ledger::{self, Ledger, Txn};
use crate::reapply::{self, ReapplyFields, ReapplyOptions};
use crate::snapshot::Snapshot;

/// The words of a transaction's original fields that a rule from a
/// correction may match (spec section 16).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RuleWords {
    /// The words of `lentil_counterparty`, in text order, each once.
    pub counterparty: Vec<String>,
    /// The words of `lentil_purpose`, in text order, each once.
    pub purpose: Vec<String>,
}

/// The words of `txn`'s counterparty and purpose that the user can pick for
/// a rule: those of [`words`], each field's in text order and each word only
/// at its first occurrence. None for a transaction without a `lentil_id` or
/// outside the run files of the sources of `config`, which no rule matches
/// (spec section 16).
#[must_use]
pub fn rule_words(txn: &Txn, config: &Config) -> RuleWords {
    if txn.id.is_none() || config.run_source(&txn.file).is_none() {
        return RuleWords::default();
    }
    let original = Original::from(txn);
    RuleWords {
        counterparty: distinct_words(original.counterparty),
        purpose: distinct_words(original.purpose),
    }
}

/// The words of `text` in text order, each at its first occurrence.
fn distinct_words(text: &str) -> Vec<String> {
    let mut seen = HashSet::new();
    words(text)
        .into_iter()
        .filter(|word| seen.insert(word.clone()))
        .collect()
}

/// One account correction.
#[derive(Clone, Copy, Debug)]
pub struct CorrectionRequest<'a> {
    /// `lentil_id` of the corrected transaction.
    pub id: &'a str,
    /// The account the user chose.
    pub account: &'a str,
    /// If set, `account` is opened on this date first.
    pub open_date: Option<Date>,
    /// The words the user picked for a rule that sets `account`, if any.
    pub rule: Option<&'a RuleWords>,
}

impl CorrectionRequest<'_> {
    /// The rule of [`Self::rule`] that sets [`Self::account`]; `None`
    /// without words.
    fn editor_rule(&self) -> Option<EditorRule> {
        self.rule.and_then(|words| {
            EditorRule::from_words(&words.counterparty, &words.purpose, self.account)
        })
    }
}

/// The intents of one correction, in the order they are replayed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Correction {
    /// `open-account` if requested, `set-account` and `confirm` of the
    /// corrected transaction, `add-rule` with a rule, then the
    /// `suggest-account` intents of the rule, then those of the learned
    /// refresh.
    pub intents: Vec<Intent>,
    /// How many other transactions the `suggest-account` intents move.
    pub moved: usize,
}

/// Why a correction cannot be computed.
#[derive(Clone, Copy, Debug, Error, PartialEq, Eq)]
pub enum CorrectionError {
    /// No active transaction has the `lentil_id`; it is unknown or dismissed.
    #[error("no active transaction has this lentil_id")]
    NotFound,
    /// The transaction does not have exactly one posting that is not on a
    /// source account.
    #[error("the transaction has no single counter posting")]
    NotEditable,
}

/// The open transactions both parts consider: those of spec section 16,
/// without reviewed ones.
const OPEN: ReapplyOptions = ReapplyOptions {
    reviewed: false,
    fields: ReapplyFields::AccountOnly,
};

/// The intents of `request` on `snapshot`, the state with the pending
/// intents applied (spec section 16, PRD FR-D14, FR-D15).
///
/// The user's own intents come first: `open-account` if
/// [`CorrectionRequest::open_date`] is set, `set-account` and `confirm`, and
/// with [`CorrectionRequest::rule`] the `add-rule` of
/// [`EditorRule::from_words`]; words that yield no rule add none. On the
/// state after them:
///
/// 1. The rule part: the new rule alone, re-applied to the account of the
///    other open transactions of spec section 16, becomes `suggest-account`
///    intents. The rule part is computed from the rule itself, so it does
///    not depend on whether `add-rule` applies.
/// 2. The learned part: every other one of these transactions whose single
///    counter posting is on `fallback_account` gets a `suggest-account` for
///    the learned suggestion of its source account, if there is one
///    ([`Learner`]).
///
/// A suggestion is kept only if its account is open on the transaction's
/// date and is not a source account. The ledger loads twice: before the
/// user's own intents, which replay on it, and after them.
///
/// # Errors
///
/// [`CorrectionError::NotFound`] if no active transaction has
/// [`CorrectionRequest::id`], [`CorrectionError::NotEditable`] if it does
/// not have exactly one counter posting.
pub fn correct(
    snapshot: &Snapshot,
    config: &Config,
    request: &CorrectionRequest<'_>,
) -> Result<Correction, CorrectionError> {
    let before = ledger::load(snapshot, &config.main_file);
    let (posting, old) = counter_posting(&before, config, request)?;
    let rule = request.editor_rule();
    let mut intents = own_intents(request, posting, old, rule.as_ref());
    let pending: Vec<PendingIntent> = intents
        .iter()
        .enumerate()
        .map(|(key, intent)| PendingIntent {
            key: key.to_string(),
            intent: intent.clone(),
        })
        .collect();
    let replayed = intent::replay_loaded(snapshot, config, &before, &pending);
    let after = ledger::load(&replayed.snapshot, &config.main_file);
    let ruled = rule
        .map(|rule| rule_part(&after, config, &rule, request.id, None))
        .unwrap_or_default();
    let moved: HashSet<&str> = ruled.iter().filter_map(suggested_id).collect();
    let learned = learned_part(&after, config, &moved);
    intents.extend(ruled.iter().chain(&learned).cloned());
    Ok(Correction {
        intents,
        moved: ruled.len() + learned.len(),
    })
}

/// How many other transactions the rule of `request` moves, as [`correct`]
/// computes it, on `ledger`, the state with the pending intents applied; 0
/// without a rule. The learned part does not count.
///
/// The preview replays nothing, so it is cheap enough to follow every
/// choice of words: it leaves out the corrected transaction, and a new
/// account counts as open from [`CorrectionRequest::open_date`].
///
/// # Errors
///
/// As [`correct`].
pub fn preview(
    ledger: &Ledger,
    config: &Config,
    request: &CorrectionRequest<'_>,
) -> Result<usize, CorrectionError> {
    counter_posting(ledger, config, request)?;
    Ok(request.editor_rule().map_or(0, |rule| {
        rule_part(ledger, config, &rule, request.id, request.open_date).len()
    }))
}

/// Index and account of the corrected transaction's counter posting in
/// `ledger`.
fn counter_posting<'l>(
    ledger: &'l Ledger,
    config: &Config,
    request: &CorrectionRequest<'_>,
) -> Result<(usize, &'l str), CorrectionError> {
    let txn = ledger.find(request.id).ok_or(CorrectionError::NotFound)?;
    reapply::counter_posting(txn, config).ok_or(CorrectionError::NotEditable)
}

/// The user's own intents: `open-account` if requested, `set-account` of
/// `posting` from `old`, `confirm`, and `add-rule` of `rule`.
fn own_intents(
    request: &CorrectionRequest<'_>,
    posting: usize,
    old: &str,
    rule: Option<&EditorRule>,
) -> Vec<Intent> {
    let open = request.open_date.map(|date| Intent::OpenAccount {
        account: request.account.to_owned(),
        date,
    });
    let own = [
        Intent::SetAccount {
            id: request.id.to_owned(),
            posting,
            old: old.to_owned(),
            new: request.account.to_owned(),
        },
        Intent::Confirm {
            id: request.id.to_owned(),
        },
    ];
    let add_rule = rule.map(|rule| Intent::AddRule {
        counterparty: rule.counterparty.clone(),
        purpose: rule.purpose.clone(),
        account: rule.account.clone(),
    });
    open.into_iter().chain(own).chain(add_rule).collect()
}

/// The `suggest-account` intents of `rule` alone, re-applied to the account
/// of the open transactions of `ledger` other than `corrected`, in ledger
/// order. The rule's account counts as open from `opened` too, if set.
fn rule_part(
    ledger: &Ledger,
    config: &Config,
    rule: &EditorRule,
    corrected: &str,
    opened: Option<Date>,
) -> Vec<Intent> {
    // A rule in the editor shape always parses.
    let Ok(rules) = Rules::parse(&rule.block()) else {
        return Vec::new();
    };
    reapply::plan(ledger, config, &rules, None, &OPEN)
        .into_iter()
        .filter_map(|change| match change.intent {
            Intent::SetAccount {
                id,
                posting,
                old,
                new,
            } if id != corrected && usable(ledger, config, &new, change.date, opened) => {
                Some(Intent::SuggestAccount {
                    id,
                    posting,
                    old,
                    new,
                })
            }
            _ => None,
        })
        .collect()
}

/// The `suggest-account` intents of the learned refresh, in ledger order:
/// for every open transaction of `after` outside `moved` whose single
/// counter posting is on `fallback_account`, the learned suggestion of its
/// source account. Each source account learns from `after` once, when the
/// first of its transactions needs it.
///
/// Only the account counts, so a transaction that a rule of `rules.toml`
/// books to `fallback_account` on purpose moves too.
fn learned_part(after: &Ledger, config: &Config, moved: &HashSet<&str>) -> Vec<Intent> {
    let mut learners: HashMap<&str, Learner> = HashMap::new();
    let mut intents = Vec::new();
    for (txn, id, source) in reapply::considered(after, config, &OPEN) {
        if moved.contains(id) {
            continue;
        }
        let Some((posting, current)) = reapply::counter_posting(txn, config) else {
            continue;
        };
        if current != config.fallback_account {
            continue;
        }
        let learner = learners.entry(source.account.as_str()).or_insert_with(|| {
            Learner::train(after, &source.account, &config.fallback_account, &[])
        });
        if let Some(account) = learner
            .suggest(&Original::from(txn))
            .filter(|account| usable(after, config, account, txn.date, None))
        {
            intents.push(Intent::SuggestAccount {
                id: id.to_owned(),
                posting,
                old: current.to_owned(),
                new: account.to_owned(),
            });
        }
    }
    intents
}

/// Whether a suggestion may set `account` on a transaction of `date`: it is
/// open on that date, or `opened` is set and not after it, and it is not a
/// source account (spec section 10).
fn usable(
    ledger: &Ledger,
    config: &Config,
    account: &str,
    date: Date,
    opened: Option<Date>,
) -> bool {
    (ledger.is_open_on(account, date) || opened.is_some_and(|opened| opened <= date))
        && !config.is_source_account(account)
}

/// The `lentil_id` a `suggest-account` intent moves.
fn suggested_id(intent: &Intent) -> Option<&str> {
    match intent {
        Intent::SuggestAccount { id, .. } => Some(id),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::intent::{Conflict, ConflictCode, Outcome};

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
account = "Assets:Bank:Card"
currency = "EUR"
importer = "csv"
mapping = "sources/card.toml"
"#;

    const MAIN: &str = "include \"accounts.beancount\"\ninclude \"imports/*/*/*.beancount\"\n";

    const ACCOUNTS: &str = "2026-01-01 open Assets:Bank:Giro EUR\n\
2026-01-01 open Assets:Bank:Card EUR\n\
2026-01-01 open Expenses:Uncategorized\n\
2026-01-01 open Expenses:Food:Groceries\n\
2026-01-01 open Expenses:Subscriptions\n\
2026-01-01 open Expenses:Fees\n";

    const RULES: &str = "# Suggestion rules.\n";

    const RUN_FILE: &str = "imports/giro/2026/20261108T000000Z.beancount";
    const GIRO: &str = "Assets:Bank:Giro";
    const CARD: &str = "Assets:Bank:Card";
    const FALLBACK: &str = "Expenses:Uncategorized";
    const GROCERIES: &str = "Expenses:Food:Groceries";
    const SUBSCRIPTIONS: &str = "Expenses:Subscriptions";
    const FEES: &str = "Expenses:Fees";
    const STREAMING: &str = "Expenses:Streaming";
    const ECKLADEN: &str = "Eckladen Markt GmbH";
    const ZAHLFIX: &str = "Zahlfix Europe S.A. et Cie";
    const STREAMFLIX: &str = "Streamflix Abo Ihr Einkauf bei Streamflix";

    /// ID, date, counterparty and purpose of the six transactions of the
    /// learning tests (`learn.rs`).
    const SIX: [(&str, &str, &str, &str); 6] = [
        (
            "giro-000000000001",
            "2026-11-01",
            ECKLADEN,
            "Kartenzahlung girocard",
        ),
        (
            "giro-000000000002",
            "2026-11-03",
            ECKLADEN,
            "Kartenzahlung girocard 2026-11-03",
        ),
        ("giro-000000000003", "2026-11-04", ZAHLFIX, STREAMFLIX),
        (
            "giro-000000000004",
            "2026-11-05",
            ZAHLFIX,
            "Ihr Einkauf bei Corner Grocery Online",
        ),
        ("giro-000000000005", "2026-11-06", ZAHLFIX, STREAMFLIX),
        (
            "giro-000000000006",
            "2026-11-07",
            "Bakery Miller",
            "Kartenzahlung girocard",
        ),
    ];

    /// A transaction of -10.00 EUR on the giro account against `counter`,
    /// with the original fields also as payee and narration, as the
    /// importer writes.
    fn txn(
        id: &str,
        date: &str,
        flag: char,
        counterparty: &str,
        purpose: &str,
        counter: &str,
    ) -> String {
        format!(
            "{date} {flag} \"{counterparty}\" \"{purpose}\"\n  lentil_id: \"{id}\"\n  lentil_counterparty: \"{counterparty}\"\n  lentil_purpose: \"{purpose}\"\n  {GIRO}  -10.00 EUR\n  {counter}\n\n"
        )
    }

    /// The six transactions, each open on the fallback account unless
    /// `changed` gives its flag and counter account.
    fn six(changed: &[(&str, char, &str)]) -> String {
        SIX.iter()
            .map(|&(id, date, counterparty, purpose)| {
                let (flag, counter) = changed
                    .iter()
                    .find(|(changed, _, _)| *changed == id)
                    .map_or(('!', FALLBACK), |&(_, flag, counter)| (flag, counter));
                txn(id, date, flag, counterparty, purpose, counter)
            })
            .collect()
    }

    /// A repository whose giro run file is `run`, with `rules.toml` if
    /// `rules` is set.
    fn repository(run: &str, rules: bool) -> Snapshot {
        let mut files = vec![
            ("lentil.toml", CONFIG),
            ("main.beancount", MAIN),
            ("accounts.beancount", ACCOUNTS),
            (RUN_FILE, run),
        ];
        if rules {
            files.push(("rules.toml", RULES));
        }
        let snapshot = Snapshot::from_files(files);
        let ledger = ledger::load(&snapshot, "main.beancount");
        assert!(ledger.errors.is_empty(), "{:?}", ledger.errors);
        snapshot
    }

    fn fixture(run: &str) -> Snapshot {
        repository(run, true)
    }

    fn config() -> Config {
        Config::parse(CONFIG).unwrap()
    }

    fn ask<'a>(
        id: &'a str,
        account: &'a str,
        open_date: Option<Date>,
        rule: Option<&'a RuleWords>,
    ) -> CorrectionRequest<'a> {
        CorrectionRequest {
            id,
            account,
            open_date,
            rule,
        }
    }

    fn purpose(words: &[&str]) -> RuleWords {
        RuleWords {
            counterparty: vec![],
            purpose: words.iter().map(|&w| w.to_owned()).collect(),
        }
    }

    fn set_account(id: &str, old: &str, new: &str) -> Intent {
        Intent::SetAccount {
            id: id.into(),
            posting: 1,
            old: old.into(),
            new: new.into(),
        }
    }

    fn confirm(id: &str) -> Intent {
        Intent::Confirm { id: id.into() }
    }

    fn suggest(id: &str, old: &str, new: &str) -> Intent {
        Intent::SuggestAccount {
            id: id.into(),
            posting: 1,
            old: old.into(),
            new: new.into(),
        }
    }

    fn add_rule(purpose: &[&str], account: &str) -> Intent {
        let words: Vec<String> = purpose.iter().map(|&w| w.to_owned()).collect();
        let rule = EditorRule::from_words(&[], &words, account).unwrap();
        Intent::AddRule {
            counterparty: rule.counterparty,
            purpose: rule.purpose,
            account: rule.account,
        }
    }

    /// The outcomes of replaying `intents` on `snapshot`.
    fn outcomes(snapshot: &Snapshot, intents: &[Intent]) -> Vec<Outcome> {
        let pending: Vec<PendingIntent> = intents
            .iter()
            .enumerate()
            .map(|(key, intent)| PendingIntent {
                key: key.to_string(),
                intent: intent.clone(),
            })
            .collect();
        intent::replay(snapshot, &config(), &pending)
            .outcomes
            .into_iter()
            .map(|(_, outcome)| outcome)
            .collect()
    }

    fn date(text: &str) -> Date {
        text.parse().unwrap()
    }

    /// The preview of `request` on the ledger of `snapshot`.
    fn preview_of(
        snapshot: &Snapshot,
        request: &CorrectionRequest<'_>,
    ) -> Result<usize, CorrectionError> {
        preview(
            &ledger::load(snapshot, "main.beancount"),
            &config(),
            request,
        )
    }

    #[test]
    fn rule_words_of_each_field_once_in_order() {
        let snapshot = fixture(&txn(
            "giro-000000000001",
            "2026-11-04",
            '!',
            ZAHLFIX,
            "Streamflix Abo Streamflix",
            FALLBACK,
        ));
        let ledger = ledger::load(&snapshot, "main.beancount");
        let words = rule_words(ledger.find("giro-000000000001").unwrap(), &config());
        assert_eq!(
            words,
            RuleWords {
                counterparty: vec!["zahlfix".into(), "europe".into(), "et".into(), "cie".into()],
                purpose: vec!["streamflix".into(), "abo".into()],
            }
        );
    }

    #[test]
    fn rule_words_of_digits_only_is_empty() {
        let snapshot = fixture(&txn(
            "giro-000000000001",
            "2026-11-04",
            '!',
            "4711",
            "12 34 2026-11-03",
            FALLBACK,
        ));
        let ledger = ledger::load(&snapshot, "main.beancount");
        let words = rule_words(ledger.find("giro-000000000001").unwrap(), &config());
        assert_eq!(words, RuleWords::default());
    }

    #[test]
    fn rule_words_only_of_imported_transactions_of_configured_sources() {
        // Without a lentil_id, and in the run file of a source that
        // lentil.toml does not configure, a transaction offers no words.
        let imported = txn(
            "giro-000000000001",
            "2026-11-04",
            '!',
            ECKLADEN,
            "Kartenzahlung",
            FALLBACK,
        );
        let unnamed = "2026-11-05 ! \"Eckladen Markt GmbH\" \"Kartenzahlung\"\n  lentil_counterparty: \"Eckladen Markt GmbH\"\n  Assets:Bank:Giro  -10.00 EUR\n  Expenses:Uncategorized\n\n";
        let mut snapshot = fixture(&[imported.as_str(), unnamed].concat());
        snapshot.insert(
            "imports/other/2026/20261108T000000Z.beancount",
            txn(
                "other-000000000001",
                "2026-11-06",
                '!',
                ECKLADEN,
                "Kartenzahlung",
                FALLBACK,
            ),
        );
        let ledger = ledger::load(&snapshot, "main.beancount");
        assert!(ledger.errors.is_empty(), "{:?}", ledger.errors);
        let words: Vec<RuleWords> = ledger
            .transactions
            .iter()
            .map(|txn| rule_words(txn, &config()))
            .collect();
        assert_eq!(
            words,
            vec![
                RuleWords {
                    counterparty: vec!["eckladen".into(), "markt".into(), "gmbh".into()],
                    purpose: vec!["kartenzahlung".into()],
                },
                RuleWords::default(),
                RuleWords::default(),
            ]
        );
    }

    #[test]
    fn a_plain_correction_refreshes_open_fallback_transactions() {
        let snapshot = fixture(&six(&[]));
        let request = ask("giro-000000000001", GROCERIES, None, None);
        let c = correct(&snapshot, &config(), &request).unwrap();
        assert_eq!(
            c.intents,
            vec![
                set_account("giro-000000000001", FALLBACK, GROCERIES),
                confirm("giro-000000000001"),
                suggest("giro-000000000002", FALLBACK, GROCERIES),
            ]
        );
        assert_eq!(c.moved, 1);
        // The preview counts only what a rule moves.
        assert_eq!(preview_of(&snapshot, &request), Ok(0));
    }

    #[test]
    fn a_gesture_adds_its_rule_and_moves_what_it_matches() {
        let snapshot = fixture(&six(&[]));
        let words = purpose(&["streamflix"]);
        let request = ask("giro-000000000003", SUBSCRIPTIONS, None, Some(&words));
        let c = correct(&snapshot, &config(), &request).unwrap();
        // 000004 (Zahlfix, Corner Grocery) stays: the learned guard rejects it.
        assert_eq!(
            c.intents,
            vec![
                set_account("giro-000000000003", FALLBACK, SUBSCRIPTIONS),
                confirm("giro-000000000003"),
                add_rule(&["streamflix"], SUBSCRIPTIONS),
                suggest("giro-000000000005", FALLBACK, SUBSCRIPTIONS),
            ]
        );
        assert_eq!(c.moved, 1);
        assert_eq!(preview_of(&snapshot, &request), Ok(1));
        assert_eq!(
            outcomes(&snapshot, &c.intents),
            vec![Outcome::Applied; c.intents.len()]
        );
    }

    #[test]
    fn a_new_account_takes_the_rule_moves() {
        let snapshot = fixture(&six(&[]));
        let words = purpose(&["streamflix"]);
        let opened = date("2026-11-01");
        let request = ask("giro-000000000003", STREAMING, Some(opened), Some(&words));
        let c = correct(&snapshot, &config(), &request).unwrap();
        assert_eq!(
            c.intents,
            vec![
                Intent::OpenAccount {
                    account: STREAMING.into(),
                    date: opened,
                },
                set_account("giro-000000000003", FALLBACK, STREAMING),
                confirm("giro-000000000003"),
                add_rule(&["streamflix"], STREAMING),
                suggest("giro-000000000005", FALLBACK, STREAMING),
            ]
        );
        assert_eq!(c.moved, 1);
        assert_eq!(preview_of(&snapshot, &request), Ok(1));
        assert_eq!(
            outcomes(&snapshot, &c.intents),
            vec![Outcome::Applied; c.intents.len()]
        );
    }

    #[test]
    fn the_rule_moves_open_transactions_off_any_account() {
        // Re-applying the rule alone rebooks the open 000005, though it is
        // not on the fallback account.
        let snapshot = fixture(&six(&[("giro-000000000005", '!', GROCERIES)]));
        let words = purpose(&["streamflix"]);
        let request = ask("giro-000000000003", SUBSCRIPTIONS, None, Some(&words));
        let c = correct(&snapshot, &config(), &request).unwrap();
        assert_eq!(
            c.intents,
            vec![
                set_account("giro-000000000003", FALLBACK, SUBSCRIPTIONS),
                confirm("giro-000000000003"),
                add_rule(&["streamflix"], SUBSCRIPTIONS),
                suggest("giro-000000000005", GROCERIES, SUBSCRIPTIONS),
            ]
        );
        assert_eq!(c.moved, 1);
        assert_eq!(preview_of(&snapshot, &request), Ok(1));
        assert_eq!(
            outcomes(&snapshot, &c.intents),
            vec![Outcome::Applied; c.intents.len()]
        );
    }

    #[test]
    fn the_rule_skips_reviewed_ones_and_learning_skips_other_accounts() {
        let snapshot = fixture(&six(&[
            ("giro-000000000005", '*', SUBSCRIPTIONS),
            ("giro-000000000002", '!', GROCERIES),
        ]));
        // The rule matches the reviewed 000005.
        let words = purpose(&["streamflix"]);
        let request = ask("giro-000000000003", FEES, None, Some(&words));
        let c = correct(&snapshot, &config(), &request).unwrap();
        assert_eq!(
            c.intents,
            vec![
                set_account("giro-000000000003", FALLBACK, FEES),
                confirm("giro-000000000003"),
                add_rule(&["streamflix"], FEES),
            ]
        );
        assert_eq!(c.moved, 0);
        assert_eq!(preview_of(&snapshot, &request), Ok(0));
        // What 000001 teaches fits the open 000002, which is not on the
        // fallback account; 000003 learns from the reviewed 000005.
        let request = ask("giro-000000000001", FEES, None, None);
        let c = correct(&snapshot, &config(), &request).unwrap();
        assert_eq!(
            c.intents,
            vec![
                set_account("giro-000000000001", FALLBACK, FEES),
                confirm("giro-000000000001"),
                suggest("giro-000000000003", FALLBACK, SUBSCRIPTIONS),
            ]
        );
        assert_eq!(c.moved, 1);
    }

    #[test]
    fn a_transaction_before_the_accounts_open_date_does_not_move() {
        // 000003 (2026-11-04) has the words of 000005, and the account opens
        // on 2026-11-05.
        let snapshot = fixture(&six(&[]));
        let opened = date("2026-11-05");
        let open = Intent::OpenAccount {
            account: STREAMING.into(),
            date: opened,
        };
        let words = purpose(&["streamflix"]);
        let with_rule = ask("giro-000000000005", STREAMING, Some(opened), Some(&words));
        let c = correct(&snapshot, &config(), &with_rule).unwrap();
        assert_eq!(
            c.intents,
            vec![
                open.clone(),
                set_account("giro-000000000005", FALLBACK, STREAMING),
                confirm("giro-000000000005"),
                add_rule(&["streamflix"], STREAMING),
            ]
        );
        assert_eq!(c.moved, 0);
        assert_eq!(preview_of(&snapshot, &with_rule), Ok(0));
        // From its open date on, it does.
        let on_the_date = ask(
            "giro-000000000005",
            STREAMING,
            Some(date("2026-11-04")),
            Some(&words),
        );
        let c = correct(&snapshot, &config(), &on_the_date).unwrap();
        assert_eq!(
            c.intents.last(),
            Some(&suggest("giro-000000000003", FALLBACK, STREAMING))
        );
        assert_eq!(c.moved, 1);
        assert_eq!(preview_of(&snapshot, &on_the_date), Ok(1));
        // Nor does the learned refresh move it.
        let plain = ask("giro-000000000005", STREAMING, Some(opened), None);
        let c = correct(&snapshot, &config(), &plain).unwrap();
        assert_eq!(
            c.intents,
            vec![
                open,
                set_account("giro-000000000005", FALLBACK, STREAMING),
                confirm("giro-000000000005"),
            ]
        );
        assert_eq!(c.moved, 0);
    }

    #[test]
    fn a_source_account_is_never_suggested() {
        // A rule on a source account moves nothing; its own intents conflict
        // when they are replayed.
        let snapshot = fixture(&six(&[]));
        let words = purpose(&["streamflix"]);
        let request = ask("giro-000000000003", CARD, None, Some(&words));
        let c = correct(&snapshot, &config(), &request).unwrap();
        assert_eq!(
            c.intents,
            vec![
                set_account("giro-000000000003", FALLBACK, CARD),
                confirm("giro-000000000003"),
                add_rule(&["streamflix"], CARD),
            ]
        );
        assert_eq!(c.moved, 0);
        assert_eq!(preview_of(&snapshot, &request), Ok(0));
        // Neither does a learned source account: 000001 teaches the card
        // account, and 000003, without words, teaches nothing.
        let settlement = ("Card Settlement", "Monthly statement");
        let snapshot = fixture(
            &[
                txn(
                    "giro-000000000001",
                    "2026-11-01",
                    '*',
                    settlement.0,
                    settlement.1,
                    CARD,
                ),
                txn(
                    "giro-000000000002",
                    "2026-11-02",
                    '!',
                    settlement.0,
                    settlement.1,
                    FALLBACK,
                ),
                txn("giro-000000000003", "2026-11-03", '!', "4711", "", FALLBACK),
            ]
            .concat(),
        );
        let request = ask("giro-000000000003", FEES, None, None);
        let c = correct(&snapshot, &config(), &request).unwrap();
        assert_eq!(
            c.intents,
            vec![
                set_account("giro-000000000003", FALLBACK, FEES),
                confirm("giro-000000000003"),
            ]
        );
        assert_eq!(c.moved, 0);
    }

    #[test]
    fn the_rule_part_does_not_depend_on_rules_toml() {
        // Without rules.toml, add-rule conflicts when it is replayed, and the
        // rule still suggests its account.
        let snapshot = repository(&six(&[]), false);
        let words = purpose(&["streamflix"]);
        let request = ask("giro-000000000003", SUBSCRIPTIONS, None, Some(&words));
        let c = correct(&snapshot, &config(), &request).unwrap();
        assert_eq!(
            c.intents,
            vec![
                set_account("giro-000000000003", FALLBACK, SUBSCRIPTIONS),
                confirm("giro-000000000003"),
                add_rule(&["streamflix"], SUBSCRIPTIONS),
                suggest("giro-000000000005", FALLBACK, SUBSCRIPTIONS),
            ]
        );
        assert_eq!(c.moved, 1);
        assert_eq!(preview_of(&snapshot, &request), Ok(1));
        assert_eq!(
            outcomes(&snapshot, &c.intents),
            vec![
                Outcome::Applied,
                Outcome::Applied,
                Outcome::Conflict(Conflict {
                    code: ConflictCode::RulesInvalid,
                    current: None,
                }),
                Outcome::Applied,
            ]
        );
    }

    #[test]
    fn a_rule_without_words_adds_nothing() {
        let snapshot = fixture(&six(&[]));
        let empty = RuleWords::default();
        let request = ask("giro-000000000001", GROCERIES, None, Some(&empty));
        let c = correct(&snapshot, &config(), &request).unwrap();
        assert_eq!(
            c.intents,
            vec![
                set_account("giro-000000000001", FALLBACK, GROCERIES),
                confirm("giro-000000000001"),
                suggest("giro-000000000002", FALLBACK, GROCERIES),
            ]
        );
        assert_eq!(preview_of(&snapshot, &request), Ok(0));
    }

    #[test]
    fn unknown_and_split_transactions_are_errors() {
        let split = "2026-11-02 ! \"Corner Shop\" \"Card payment\"\n  lentil_id: \"giro-000000000007\"\n  Assets:Bank:Giro  -10.00 EUR\n  Expenses:Food:Groceries  5.00 EUR\n  Expenses:Fees\n\n";
        let dismissed: String = txn(
            "giro-000000000008",
            "2026-11-02",
            '!',
            "Old Shop",
            "Card payment",
            FALLBACK,
        )
        .lines()
        .map(|line| format!("; {line}\n"))
        .collect();
        let snapshot = fixture(&[six(&[]).as_str(), split, &dismissed].concat());
        let config = config();
        let attempt = |id| {
            let request = ask(id, GROCERIES, None, None);
            (
                correct(&snapshot, &config, &request).map(|c| c.moved),
                preview_of(&snapshot, &request),
            )
        };
        let not_found = (
            Err(CorrectionError::NotFound),
            Err(CorrectionError::NotFound),
        );
        assert_eq!(attempt("giro-ffffffffffff"), not_found);
        assert_eq!(attempt("giro-000000000008"), not_found);
        assert_eq!(
            attempt("giro-000000000007"),
            (
                Err(CorrectionError::NotEditable),
                Err(CorrectionError::NotEditable)
            )
        );
    }
}
