//! Learned suggestions (spec section 16): the counter account that the
//! reviewed transactions of one source account teach for the words of an
//! original text.
//!
//! [`rustledger_ops::ml`] picks the candidate account. Lentil's guard
//! suggests it only if a reviewed transaction booked to it shares words that
//! carry at least half of the text's weight, so wording common to most of a
//! source's transactions teaches little.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt;
use std::sync::LazyLock;

use jiff::civil::Date;
use regex::Regex;
use rustledger_core::{Directive, Posting, Transaction};
use rustledger_ops::ml::CategorizationModel;

use crate::id;
use crate::import::Original;
use crate::ledger::{Ledger, Txn};

/// A word: a run of at least two characters of Unicode general category L.
static WORD: LazyLock<Regex> = LazyLock::new(|| {
    #[allow(clippy::unwrap_used)] // A constant pattern.
    Regex::new(r"\pL{2,}").unwrap()
});

/// The words of `text` in text order, duplicates kept: its maximal runs of
/// characters of Unicode general category L (Letter) that have at least two
/// letters, lowercased by Unicode's full mapping with its final-sigma rule,
/// except that "İ" (U+0130) stays as it is (spec section 16). Digits and
/// every other character separate words.
///
/// "İ" is the only letter whose lowercase is longer: "i" and a combining
/// dot, which is no letter. Kept, it leaves a word letters only, and a rule
/// pattern of the word matches the text, because `(?i)` folds letter by
/// letter. A word holds no combining dot of its own, so every "i" with one
/// after lowercasing was an "İ".
#[must_use]
pub fn words(text: &str) -> Vec<String> {
    WORD.find_iter(text)
        .map(|word| word.as_str().to_lowercase().replace("i\u{307}", "İ"))
        .collect()
}

/// The learned suggestions of one source account (spec section 16).
pub struct Learner {
    /// `N`: how many texts count for the word weights.
    texts: usize,
    /// `df`: for each word, how many of these texts contain it.
    frequency: HashMap<String, usize>,
    /// Each learned account with the reviewed transactions that teach it.
    taught: BTreeMap<String, Samples>,
    /// The classifier over all of them, or `None` if `ml` found too little
    /// data, as with fewer than two learned accounts.
    model: Option<CategorizationModel>,
}

/// The reviewed transactions that teach one account, indexed by word.
#[derive(Debug, Default)]
struct Samples {
    /// How many there are.
    count: usize,
    /// For each word, the samples whose words contain it, by index.
    by_word: HashMap<String, Vec<usize>>,
}

impl Samples {
    /// Adds one sample with the given words.
    fn add(&mut self, words: &Words) {
        for word in words.distinct() {
            self.by_word
                .entry(word.to_owned())
                .or_default()
                .push(self.count);
        }
        self.count += 1;
    }
}

/// The words of the two original fields that learning reads.
struct Words {
    /// The words of `lentil_counterparty`, in text order.
    counterparty: Vec<String>,
    /// The words of `lentil_purpose`, in text order.
    purpose: Vec<String>,
}

impl Words {
    /// The words of `lentil_counterparty` and `lentil_purpose`, never of
    /// `lentil_type`.
    fn of(original: &Original<'_>) -> Self {
        Self {
            counterparty: words(original.counterparty),
            purpose: words(original.purpose),
        }
    }

    /// The distinct words of both fields.
    fn distinct(&self) -> BTreeSet<&str> {
        self.counterparty
            .iter()
            .chain(&self.purpose)
            .map(String::as_str)
            .collect()
    }

    /// The transaction `ml` learns from: the counterparty's words as payee,
    /// the purpose's as narration, the source posting first and the counter
    /// posting second, which `ml` reads as the target. An empty payee adds
    /// no word.
    fn directive(&self, date: Date, source_account: &str, counter: &str) -> Directive {
        Directive::Transaction(
            Transaction::new(date, self.purpose.join(" "))
                .with_payee(self.counterparty.join(" "))
                .with_synthesized_posting(Posting::auto(source_account))
                .with_synthesized_posting(Posting::auto(counter)),
        )
    }
}

impl Learner {
    /// Learns from the active transactions of `ledger` that post to
    /// `source_account`, reading only the words of their original fields
    /// `lentil_counterparty` and `lentil_purpose` (PRD FR-C8, FR-C22).
    ///
    /// A reviewed (`*`) transaction with exactly two postings, one on
    /// `source_account`, teaches its counter account, unless that is
    /// `fallback_account` or the transaction has no words. Every one with an
    /// original text, open or reviewed, counts for the word weights, and so
    /// does every text in `extra`, which teaches nothing: at import, the
    /// run's new transactions.
    #[must_use]
    pub fn train(
        ledger: &Ledger,
        source_account: &str,
        fallback_account: &str,
        extra: &[Original<'_>],
    ) -> Self {
        let mut learner = Self {
            texts: 0,
            frequency: HashMap::new(),
            taught: BTreeMap::new(),
            model: None,
        };
        let mut directives = Vec::new();
        for txn in &ledger.transactions {
            if !txn.postings.iter().any(|p| p.account == source_account) {
                continue;
            }
            let original = Original::from(txn);
            let words = Words::of(&original);
            learner.count(&original, &words);
            if txn.flag != '*' || words.distinct().is_empty() {
                continue;
            }
            let Some(counter) =
                counter_account(txn, source_account).filter(|a| *a != fallback_account)
            else {
                continue;
            };
            learner
                .taught
                .entry(counter.to_owned())
                .or_default()
                .add(&words);
            directives.push(words.directive(txn.date, source_account, counter));
        }
        for original in extra {
            learner.count(original, &Words::of(original));
        }
        learner.model = CategorizationModel::train(&directives).ok();
        learner
    }

    /// Counts a text for the word weights if its original text is not empty.
    fn count(&mut self, original: &Original<'_>, words: &Words) {
        if id::original_text(Some(original.counterparty), Some(original.purpose)).is_empty() {
            return;
        }
        self.texts += 1;
        for word in words.distinct() {
            *self.frequency.entry(word.to_owned()).or_default() += 1;
        }
    }

    /// A word's weight, `ln(N / df)`, where a word outside the counted texts
    /// counts as found once.
    fn weight(&self, word: &str) -> f64 {
        let df = self.frequency.get(word).copied().unwrap_or_default().max(1);
        (self.texts as f64 / df as f64).ln()
    }

    /// The learned counter account for the original fields of a transaction,
    /// if the guard of spec section 16 admits one.
    ///
    /// The candidate is `ml`'s top account, or the only learned account if
    /// `ml` found too little data. It is suggested only if a reviewed
    /// transaction booked to it shares words with the text whose weights add
    /// up to at least half of the weight of all the text's distinct words. A
    /// text whose words weigh 0 in total gets no suggestion, and no other
    /// account is tried.
    #[must_use]
    pub fn suggest(&self, original: &Original<'_>) -> Option<&str> {
        let words = Words::of(original);
        let weights: Vec<(&str, f64)> = words
            .distinct()
            .into_iter()
            .map(|word| (word, self.weight(word)))
            .collect();
        let total: f64 = weights.iter().map(|(_, weight)| weight).sum();
        if total <= 0.0 {
            return None;
        }
        let (account, samples) = self.candidate(&words)?;
        let mut shared = vec![0.0; samples.count];
        for (word, weight) in &weights {
            for &sample in samples.by_word.get(*word).into_iter().flatten() {
                shared[sample] += weight;
            }
        }
        shared
            .into_iter()
            .any(|weight| weight * 2.0 >= total)
            .then_some(account.as_str())
    }

    /// The candidate account for the text's words and the samples that
    /// teach it.
    fn candidate(&self, words: &Words) -> Option<(&String, &Samples)> {
        let Some(model) = &self.model else {
            return self.taught.iter().next().filter(|_| self.taught.len() == 1);
        };
        let predictions = model.predict(
            &words.purpose.join(" "),
            Some(&words.counterparty.join(" ")),
        );
        let (top, _) = predictions.into_iter().next()?;
        self.taught.get_key_value(&top)
    }
}

impl fmt::Debug for Learner {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // The classifier has no `Debug`; the learned accounts name what it
        // can predict.
        f.debug_struct("Learner")
            .field("texts", &self.texts)
            .field("taught", &self.taught.keys().collect::<Vec<_>>())
            .finish_non_exhaustive()
    }
}

/// The counter account of a transaction with exactly two postings, one of
/// them on `source_account`.
fn counter_account<'t>(txn: &'t Txn, source_account: &str) -> Option<&'t str> {
    let [a, b] = txn.postings.as_slice() else {
        return None;
    };
    match (a.account == source_account, b.account == source_account) {
        (true, false) => Some(&b.account),
        (false, true) => Some(&a.account),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ledger;
    use crate::snapshot::Snapshot;

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
"#;

    const MAIN: &str = "include \"accounts.beancount\"\ninclude \"imports/*/*/*.beancount\"\n";

    const ACCOUNTS: &str = "2026-01-01 open Assets:Bank:Giro EUR\n\
2026-01-01 open Assets:Bank:Card EUR\n\
2026-01-01 open Expenses:Uncategorized\n\
2026-01-01 open Expenses:Food:Groceries\n\
2026-01-01 open Expenses:Subscriptions\n\
2026-01-01 open Expenses:Fees\n";

    const RUN_FILE: &str = "imports/giro/2026/20261108T000000Z.beancount";
    const GIRO: &str = "Assets:Bank:Giro";
    const FALLBACK: &str = "Expenses:Uncategorized";
    const ZAHLFIX: &str = "Zahlfix Europe S.A. et Cie";
    const STREAMFLIX: &str = "Streamflix Abo Ihr Einkauf bei Streamflix";

    /// A transaction of -10.00 EUR on `source` against `counter`, with the
    /// original fields also as payee and narration, as the importer writes.
    fn txn(
        id: &str,
        date: &str,
        flag: char,
        counterparty: &str,
        purpose: &str,
        source: &str,
        counter: &str,
    ) -> String {
        format!(
            "{date} {flag} \"{counterparty}\" \"{purpose}\"\n  lentil_id: \"{id}\"\n  lentil_counterparty: \"{counterparty}\"\n  lentil_purpose: \"{purpose}\"\n  {source}  -10.00 EUR\n  {counter}\n\n"
        )
    }

    fn load(run: &str) -> Ledger {
        let snapshot = Snapshot::from_files([
            ("lentil.toml", CONFIG),
            ("main.beancount", MAIN),
            ("accounts.beancount", ACCOUNTS),
            (RUN_FILE, run),
        ]);
        let ledger = ledger::load(&snapshot, "main.beancount");
        assert!(ledger.errors.is_empty(), "{:?}", ledger.errors);
        ledger
    }

    fn fixture(r2_reviewed: bool) -> Ledger {
        let (flag, counter) = if r2_reviewed {
            ('*', "Expenses:Subscriptions")
        } else {
            ('!', FALLBACK)
        };
        let eckladen = "Eckladen Markt GmbH";
        load(
            &[
                txn(
                    "giro-000000000001",
                    "2026-11-01",
                    '*',
                    eckladen,
                    "Kartenzahlung girocard",
                    GIRO,
                    "Expenses:Food:Groceries",
                ),
                txn(
                    "giro-000000000002",
                    "2026-11-03",
                    '!',
                    eckladen,
                    "Kartenzahlung girocard 2026-11-03",
                    GIRO,
                    FALLBACK,
                ),
                txn(
                    "giro-000000000003",
                    "2026-11-04",
                    flag,
                    ZAHLFIX,
                    STREAMFLIX,
                    GIRO,
                    counter,
                ),
                txn(
                    "giro-000000000004",
                    "2026-11-05",
                    '!',
                    ZAHLFIX,
                    "Ihr Einkauf bei Corner Grocery Online",
                    GIRO,
                    FALLBACK,
                ),
                txn(
                    "giro-000000000005",
                    "2026-11-06",
                    '!',
                    ZAHLFIX,
                    STREAMFLIX,
                    GIRO,
                    FALLBACK,
                ),
                txn(
                    "giro-000000000006",
                    "2026-11-07",
                    '!',
                    "Bakery Miller",
                    "Kartenzahlung girocard",
                    GIRO,
                    FALLBACK,
                ),
            ]
            .concat(),
        )
    }

    fn train(ledger: &Ledger, extra: &[Original<'_>]) -> Learner {
        Learner::train(ledger, GIRO, FALLBACK, extra)
    }

    #[test]
    fn words_are_letter_runs_of_two_or_more() {
        assert_eq!(
            words("Zahlfix Europe S.A. et Cie"),
            ["zahlfix", "europe", "et", "cie"]
        );
        assert_eq!(
            words("ECKLADEN4711 Markt-Straße 12"),
            ["eckladen", "markt", "straße"]
        );
        assert_eq!(words("Café Müller café"), ["café", "müller", "café"]);
        // "İ" lowercases to two characters, "i" and a combining dot.
        assert_eq!(words("İSTANBUL GRILL ΟΔΟΣ"), ["İstanbul", "grill", "οδος"]);
        assert!(words("12 34 a").is_empty());
    }

    #[test]
    fn shared_distinctive_words_suggest_and_common_ones_do_not() {
        let ledger = fixture(true);
        let learner = Learner::train(&ledger, "Assets:Bank:Giro", "Expenses:Uncategorized", &[]);
        let s = |id| learner.suggest(&Original::from(ledger.find(id).unwrap()));
        assert_eq!(s("giro-000000000002"), Some("Expenses:Food:Groceries"));
        assert_eq!(s("giro-000000000004"), None); // only "zahlfix", "ihr", "bei" … shared
        assert_eq!(s("giro-000000000005"), Some("Expenses:Subscriptions"));
        assert_eq!(s("giro-000000000006"), None); // only "kartenzahlung girocard" shared
    }

    #[test]
    fn one_learned_account_still_suggests() {
        let ledger = fixture(false);
        let learner = Learner::train(&ledger, "Assets:Bank:Giro", "Expenses:Uncategorized", &[]);
        let s = |id| learner.suggest(&Original::from(ledger.find(id).unwrap()));
        assert_eq!(s("giro-000000000002"), Some("Expenses:Food:Groceries"));
        assert_eq!(s("giro-000000000005"), None);
        assert_eq!(s("giro-000000000006"), None);
    }

    /// A reviewed "Bank" / "Fee" on `Expenses:Fees` and an open one, plus
    /// `more` transactions.
    fn fees(more: &str) -> Ledger {
        load(
            &[
                txn(
                    "giro-000000000001",
                    "2026-11-01",
                    '*',
                    "Bank",
                    "Fee",
                    GIRO,
                    "Expenses:Fees",
                ),
                txn(
                    "giro-000000000002",
                    "2026-11-02",
                    '!',
                    "Bank",
                    "Fee",
                    GIRO,
                    FALLBACK,
                ),
                more.to_owned(),
            ]
            .concat(),
        )
    }

    #[test]
    fn words_in_every_transaction_weigh_nothing() {
        // N = df = 2, so the total weight is 0; another source's transactions
        // do not count.
        let ledger = fees(&txn(
            "card-000000000001",
            "2026-11-03",
            '!',
            "City Utilities",
            "Electricity",
            "Assets:Bank:Card",
            FALLBACK,
        ));
        let open = Original::from(ledger.find("giro-000000000002").unwrap());
        assert_eq!(train(&ledger, &[]).suggest(&open), None);
    }

    #[test]
    fn texts_without_words_count_for_the_weights() {
        // A third original text, without words: N = 3, df = 2.
        let ledger = fees(&txn(
            "giro-000000000003",
            "2026-11-03",
            '!',
            "4711",
            "",
            GIRO,
            FALLBACK,
        ));
        let open = Original::from(ledger.find("giro-000000000002").unwrap());
        assert_eq!(train(&ledger, &[]).suggest(&open), Some("Expenses:Fees"));
    }

    #[test]
    fn extra_texts_count_for_the_weights() {
        let ledger = fees("");
        let open = Original::from(ledger.find("giro-000000000002").unwrap());
        let extra = [Original::new(Some("City Utilities"), None, None)];
        assert_eq!(train(&ledger, &extra).suggest(&open), Some("Expenses:Fees"));
        // An empty original text does not count; the booking type is no part
        // of it.
        let empty = [Original::new(Some(" "), Some(""), Some("Debit"))];
        assert_eq!(train(&ledger, &empty).suggest(&open), None);
    }

    #[test]
    fn words_outside_the_corpus_count_as_once() {
        // "musterstadt" weighs ln(6 / 1), less than the shared words' ln(6 / 2) * 3
        // + ln(6 / 3) * 2.
        let ledger = fixture(true);
        let text = Original::new(
            Some("Eckladen Markt GmbH"),
            Some("Kartenzahlung girocard Musterstadt"),
            None,
        );
        assert_eq!(
            train(&ledger, &[]).suggest(&text),
            Some("Expenses:Food:Groceries")
        );
    }

    #[test]
    fn fallback_and_other_sources_teach_nothing() {
        // A reviewed transaction on the fallback account, and one on another
        // source account, with the same words as an open one; a third text
        // keeps the words' weight above 0.
        let words = ("Corner Shop", "Card payment");
        let ledger = load(
            &[
                txn(
                    "giro-000000000001",
                    "2026-11-01",
                    '*',
                    words.0,
                    words.1,
                    GIRO,
                    FALLBACK,
                ),
                txn(
                    "card-000000000001",
                    "2026-11-01",
                    '*',
                    words.0,
                    words.1,
                    "Assets:Bank:Card",
                    "Expenses:Food:Groceries",
                ),
                txn(
                    "giro-000000000002",
                    "2026-11-02",
                    '!',
                    words.0,
                    words.1,
                    GIRO,
                    FALLBACK,
                ),
                txn(
                    "giro-000000000003",
                    "2026-11-03",
                    '!',
                    "City Utilities",
                    "Electricity",
                    GIRO,
                    FALLBACK,
                ),
            ]
            .concat(),
        );
        let open = Original::from(ledger.find("giro-000000000002").unwrap());
        assert_eq!(train(&ledger, &[]).suggest(&open), None);
    }

    #[test]
    fn only_reviewed_transactions_with_two_postings_teach() {
        let three_postings = "2026-11-02 * \"Corner Shop\" \"Card payment\"\n  lentil_counterparty: \"Corner Shop\"\n  lentil_purpose: \"Card payment\"\n  Assets:Bank:Giro  -10.00 EUR\n  Expenses:Food:Groceries  5.00 EUR\n  Expenses:Fees\n\n";
        let open = txn(
            "giro-000000000001",
            "2026-11-01",
            '!',
            "Corner Shop",
            "Card payment",
            GIRO,
            "Expenses:Food:Groceries",
        );
        // The source posting second teaches too.
        let swapped = "2026-11-03 * \"City Utilities\" \"Electricity\"\n  lentil_counterparty: \"City Utilities\"\n  lentil_purpose: \"Electricity\"\n  Expenses:Subscriptions  10.00 EUR\n  Assets:Bank:Giro\n\n";
        let ledger = load(&[open.as_str(), three_postings, swapped].concat());
        let learner = train(&ledger, &[]);
        let corner = Original::new(Some("Corner Shop"), Some("Card payment"), None);
        let city = Original::new(Some("City Utilities"), Some("Electricity"), None);
        assert_eq!(learner.suggest(&corner), None);
        assert_eq!(learner.suggest(&city), Some("Expenses:Subscriptions"));
    }

    #[test]
    fn only_counterparty_and_purpose_teach() {
        let edited = "2026-11-01 * \"Corner Shop\" \"Weekly groceries\"\n  lentil_counterparty: \"CS 4711\"\n  lentil_type: \"Card payment\"\n  Assets:Bank:Giro  -10.00 EUR\n  Expenses:Food:Groceries\n\n";
        let other = txn(
            "giro-000000000002",
            "2026-11-02",
            '!',
            "City Utilities",
            "Electricity",
            GIRO,
            FALLBACK,
        );
        let ledger = load(&[edited, other.as_str()].concat());
        let learner = train(&ledger, &[]);
        let s = |counterparty, purpose, booking_type| {
            learner.suggest(&Original::new(
                Some(counterparty),
                Some(purpose),
                Some(booking_type),
            ))
        };
        assert_eq!(s("CS 4711", "", ""), Some("Expenses:Food:Groceries"));
        assert_eq!(s("Corner Shop", "Weekly groceries", ""), None);
        assert_eq!(s("", "", "Card payment"), None);
        // The text's booking type does not count either; with it, "cs" would
        // carry less than half of the weight.
        assert_eq!(
            s("CS", "", "City Utilities"),
            Some("Expenses:Food:Groceries")
        );
    }

    #[test]
    fn half_of_the_weight_is_enough() {
        // "cs" and the unknown "rent" both weigh ln(2 / 1).
        let edited = "2026-11-01 * \"\" \"\"\n  lentil_counterparty: \"CS 4711\"\n  Assets:Bank:Giro  -10.00 EUR\n  Expenses:Food:Groceries\n\n";
        let other = txn(
            "giro-000000000002",
            "2026-11-02",
            '!',
            "City Utilities",
            "Electricity",
            GIRO,
            FALLBACK,
        );
        let learner = train(&load(&[edited, other.as_str()].concat()), &[]);
        let text = Original::new(Some("CS"), Some("Rent"), None);
        assert_eq!(learner.suggest(&text), Some("Expenses:Food:Groceries"));
        let text = Original::new(Some("CS"), Some("Rent Due"), None);
        assert_eq!(learner.suggest(&text), None);
    }

    #[test]
    fn a_failing_candidate_never_falls_back_to_another_account() {
        // Two reviewed "Zahlfix Europe" payments teach groceries, one
        // Streamflix payment with a long text teaches subscriptions, and two
        // open "Zahlfix Europe" payments make those words common.
        let groceries = "Expenses:Food:Groceries";
        let zahlfix = "Zahlfix Europe";
        let subscription = (
            "Streamflix International Media Services",
            "Monthly membership",
        );
        let ledger = load(
            &[
                txn(
                    "giro-000000000001",
                    "2026-11-01",
                    '*',
                    zahlfix,
                    "",
                    GIRO,
                    groceries,
                ),
                txn(
                    "giro-000000000002",
                    "2026-11-02",
                    '*',
                    zahlfix,
                    "",
                    GIRO,
                    groceries,
                ),
                txn(
                    "giro-000000000003",
                    "2026-11-03",
                    '*',
                    subscription.0,
                    subscription.1,
                    GIRO,
                    "Expenses:Subscriptions",
                ),
                txn(
                    "giro-000000000004",
                    "2026-11-04",
                    '!',
                    zahlfix,
                    "",
                    GIRO,
                    FALLBACK,
                ),
                txn(
                    "giro-000000000005",
                    "2026-11-05",
                    '!',
                    zahlfix,
                    "",
                    GIRO,
                    FALLBACK,
                ),
            ]
            .concat(),
        );
        let learner = train(&ledger, &[]);
        let text = Original::new(Some(zahlfix), Some("Streamflix"), None);
        // ml's top account is groceries, whose samples share only the common
        // words.
        let words = Words::of(&text);
        let (candidate, _) = learner.candidate(&words).unwrap();
        assert_eq!(candidate, groceries);
        // The Streamflix sample shares "streamflix", which carries more than
        // half of the text's weight, so subscriptions would pass the guard.
        let total: f64 = words.distinct().iter().map(|w| learner.weight(w)).sum();
        assert!(learner.weight("streamflix") * 2.0 >= total);
        // Still no suggestion: no other account is tried.
        assert_eq!(learner.suggest(&text), None);
    }

    #[test]
    fn debug_names_the_learned_accounts() {
        let learner = train(&fixture(true), &[]);
        let debug = format!("{learner:?}");
        assert!(debug.contains("Expenses:Subscriptions"), "{debug}");
    }
}
