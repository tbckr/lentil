//! Suggestion rules from `rules.toml` and learned suggestions (spec section 16).

use std::collections::BTreeMap;

use jiff::civil::Date;
use regex::Regex;
use serde::Deserialize;
use thiserror::Error;

use crate::config::Config;
use crate::ledger::Ledger;

/// Path of the rules file, relative to the repository root.
pub const RULES_FILE: &str = "rules.toml";

/// The parsed `rules.toml`; rules are evaluated in file order.
#[derive(Clone, Debug, Default)]
pub struct Rules {
    rules: Vec<Rule>,
}

/// One `[[rule]]`: every given condition must match.
#[derive(Clone, Debug)]
struct Rule {
    source: Option<String>,
    desc: Option<Regex>,
    payee: Option<Regex>,
    narration: Option<Regex>,
    account: String,
}

/// Why `rules.toml` is rejected.
#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum RulesError {
    /// Not TOML, an unknown key or a missing `account`.
    #[error("invalid rules.toml: {0}")]
    Syntax(String),
    /// A regular expression does not compile.
    #[error("invalid regular expression {0:?}")]
    Regex(String),
    /// A rule matches on nothing (no `desc`, `payee` or `narration`).
    /// Carries the rule's 1-based position in the file.
    #[error("rule {0} has no condition")]
    NoCondition(usize),
    /// A rule's `account` is not a valid Beancount account name.
    #[error("invalid account {0:?}")]
    InvalidAccount(String),
    /// A rule books to a source account, which only its own source posts to
    /// (spec section 16).
    #[error("rule account {0:?} is a source account")]
    SourceAccount(String),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawRules {
    #[serde(default)]
    rule: Vec<RawRule>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawRule {
    source: Option<String>,
    desc: Option<String>,
    payee: Option<String>,
    narration: Option<String>,
    account: String,
}

fn compile(pattern: Option<String>) -> Result<Option<Regex>, RulesError> {
    pattern
        .map(|p| Regex::new(&p).map_err(|_| RulesError::Regex(p)))
        .transpose()
}

/// Whether `name` is a Beancount account under one of the five root accounts.
#[must_use]
pub fn is_account_name(name: &str) -> bool {
    let mut parts = name.split(':');
    let root = parts.next().unwrap_or_default();
    let mut rest = parts.peekable();
    ["Assets", "Liabilities", "Equity", "Income", "Expenses"].contains(&root)
        && rest.peek().is_some()
        && rest.all(|part| {
            let mut chars = part.chars();
            chars
                .next()
                .is_some_and(|c| c.is_uppercase() || c.is_ascii_digit())
                && chars.all(|c| c.is_alphanumeric() || c == '-')
        })
}

impl Rules {
    /// Parses `rules.toml`. An empty file has no rules.
    ///
    /// # Errors
    ///
    /// Returns a [`RulesError`] for invalid content.
    pub fn parse(text: &str) -> Result<Self, RulesError> {
        let raw: RawRules =
            toml::from_str(text).map_err(|e| RulesError::Syntax(e.message().to_owned()))?;
        let rules = raw
            .rule
            .into_iter()
            .enumerate()
            .map(|(i, r)| {
                if r.desc.is_none() && r.payee.is_none() && r.narration.is_none() {
                    return Err(RulesError::NoCondition(i + 1));
                }
                if !is_account_name(&r.account) {
                    return Err(RulesError::InvalidAccount(r.account));
                }
                Ok(Rule {
                    source: r.source,
                    desc: compile(r.desc)?,
                    payee: compile(r.payee)?,
                    narration: compile(r.narration)?,
                    account: r.account,
                })
            })
            .collect::<Result<_, _>>()?;
        Ok(Self { rules })
    }

    /// Checks that no rule books to a source account (spec section 16).
    ///
    /// # Errors
    ///
    /// [`RulesError::SourceAccount`] for the first rule that does.
    pub fn check_accounts(&self, config: &Config) -> Result<(), RulesError> {
        match self
            .rules
            .iter()
            .find(|r| config.is_source_account(&r.account))
        {
            Some(rule) => Err(RulesError::SourceAccount(rule.account.clone())),
            None => Ok(()),
        }
    }

    /// Number of rules.
    #[must_use]
    pub fn len(&self) -> usize {
        self.rules.len()
    }

    /// Whether there are no rules.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }

    /// The account of the first matching rule.
    ///
    /// A rule matches if its `source` (if given) equals `source` and every
    /// given regular expression matches; a missing payee is matched as the
    /// empty string.
    #[must_use]
    pub fn suggest(
        &self,
        source: &str,
        desc: &str,
        payee: Option<&str>,
        narration: &str,
    ) -> Option<&str> {
        let payee = payee.unwrap_or_default();
        let hit = |re: &Option<Regex>, text: &str| re.as_ref().is_none_or(|re| re.is_match(text));
        self.rules
            .iter()
            .find(|r| {
                r.source.as_deref().is_none_or(|s| s == source)
                    && hit(&r.desc, desc)
                    && hit(&r.payee, payee)
                    && hit(&r.narration, narration)
            })
            .map(|r| r.account.as_str())
    }
}

/// Suggestions learned from reviewed transactions of one source account.
#[derive(Clone, Debug, Default)]
pub struct Learned {
    /// Normalized description to the chosen counter account.
    by_key: BTreeMap<String, String>,
}

/// How often a counter account was chosen for one key, and when last.
#[derive(Clone, Copy, Debug)]
struct Tally {
    count: usize,
    last: (Date, usize),
}

impl Learned {
    /// Learns from reviewed (`*`) transactions posting to `source_account`,
    /// using only `lentil_desc` and the counter account (PRD FR-C8).
    ///
    /// Only active transactions with exactly two postings, one on
    /// `source_account`, count. A counter account equal to `fallback_account`
    /// is never learned, so an unsorted transaction teaches nothing. For each
    /// normalized description the most frequent counter account wins; ties go
    /// to the most recent transaction (by date, then ledger order).
    #[must_use]
    pub fn from_ledger(ledger: &Ledger, source_account: &str, fallback_account: &str) -> Self {
        let mut tallies: BTreeMap<String, BTreeMap<&str, Tally>> = BTreeMap::new();
        for (order, txn) in ledger.transactions.iter().enumerate() {
            let [a, b] = txn.postings.as_slice() else {
                continue;
            };
            let counter = match (a.account == source_account, b.account == source_account) {
                (true, false) => &b.account,
                (false, true) => &a.account,
                _ => continue,
            };
            if txn.flag != '*' || counter == fallback_account {
                continue;
            }
            let Some(key) = txn.desc.as_deref().map(learn_key).filter(|k| !k.is_empty()) else {
                continue;
            };
            let tally = tallies
                .entry(key)
                .or_default()
                .entry(counter.as_str())
                .or_insert(Tally {
                    count: 0,
                    last: (txn.date, order),
                });
            tally.count += 1;
            tally.last = tally.last.max((txn.date, order));
        }
        let by_key = tallies
            .into_iter()
            .filter_map(|(key, accounts)| {
                accounts
                    .into_iter()
                    .max_by_key(|(_, t)| (t.count, t.last))
                    .map(|(account, _)| (key, account.to_owned()))
            })
            .collect();
        Self { by_key }
    }

    /// The learned counter account for a description, if any.
    #[must_use]
    pub fn suggest(&self, desc: &str) -> Option<&str> {
        self.by_key.get(&learn_key(desc)).map(String::as_str)
    }
}

/// The key under which a description is learned: lowercase, digits removed,
/// every other character that is not a letter turned into a space, and
/// whitespace collapsed. Card numbers, dates and amounts in a bank's wording
/// therefore do not split one payee into many keys.
#[must_use]
pub fn learn_key(desc: &str) -> String {
    let spaced: String = desc
        .chars()
        .filter(|c| !c.is_numeric())
        .flat_map(|c| {
            let keep = c.is_alphabetic();
            c.to_lowercase()
                .map(move |l| if keep { l } else { ' ' })
                .collect::<Vec<_>>()
        })
        .collect();
    crate::id::normalize_text(&spaced)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ledger;
    use crate::snapshot::Snapshot;

    const RULES: &str = r#"
[[rule]]
source = "giro"
desc = "(?i)corner grocery"
account = "Expenses:Food:Groceries"

[[rule]]
payee = "^City Utilities$"
narration = "(?i)electricity"
account = "Expenses:Home:Electricity"

[[rule]]
desc = "(?i)grocery"
account = "Expenses:Food:Other"
"#;

    #[test]
    fn empty_file_has_no_rules() {
        let rules = Rules::parse("").unwrap();
        assert!(rules.is_empty());
        assert_eq!(rules.suggest("giro", "x", None, "x"), None);
    }

    #[test]
    fn first_matching_rule_wins_in_file_order() {
        let rules = Rules::parse(RULES).unwrap();
        assert_eq!(rules.len(), 3);
        assert!(!rules.is_empty());
        assert_eq!(
            rules.suggest("giro", "CORNER GROCERY 4711", None, ""),
            Some("Expenses:Food:Groceries")
        );
        // The first rule is limited to giro; the third matches for other sources.
        assert_eq!(
            rules.suggest("card", "CORNER GROCERY 4711", None, ""),
            Some("Expenses:Food:Other")
        );
        assert_eq!(rules.suggest("giro", "BAKERY", None, ""), None);
    }

    #[test]
    fn every_given_condition_must_match() {
        let rules = Rules::parse(RULES).unwrap();
        assert_eq!(
            rules.suggest("giro", "X", Some("City Utilities"), "Electricity 10/2026"),
            Some("Expenses:Home:Electricity")
        );
        assert_eq!(
            rules.suggest("giro", "X", Some("City Utilities"), "Water"),
            None
        );
        assert_eq!(rules.suggest("giro", "X", None, "Electricity"), None);
    }

    #[test]
    fn missing_payee_matches_as_empty() {
        let rules =
            Rules::parse("[[rule]]\npayee = \"^$\"\naccount = \"Expenses:Misc\"\n").unwrap();
        assert_eq!(rules.suggest("giro", "x", None, "y"), Some("Expenses:Misc"));
        assert_eq!(rules.suggest("giro", "x", Some("A"), "y"), None);
    }

    #[test]
    fn rejects_unknown_keys_and_missing_account() {
        let unknown = "[[rule]]\ndesc = \"x\"\naccount = \"Expenses:A\"\nprogram = \"y\"\n";
        assert!(matches!(Rules::parse(unknown), Err(RulesError::Syntax(_))));
        assert!(matches!(
            Rules::parse("[settings]\nx = 1\n"),
            Err(RulesError::Syntax(_))
        ));
        assert!(matches!(
            Rules::parse("[[rule]]\ndesc = \"x\"\n"),
            Err(RulesError::Syntax(_))
        ));
    }

    #[test]
    fn rejects_rules_without_condition() {
        let text = "[[rule]]\ndesc = \"x\"\naccount = \"Expenses:A\"\n\n[[rule]]\nsource = \"giro\"\naccount = \"Expenses:B\"\n";
        assert_eq!(Rules::parse(text).unwrap_err(), RulesError::NoCondition(2));
    }

    #[test]
    fn rejects_rules_that_book_to_a_source_account() {
        let config = Config::parse(
            r#"spec_version = 0
main_file = "main.beancount"
fallback_account = "Expenses:Uncategorized"
unattributed_account = "Expenses:Unattributed"

[[source]]
id = "card"
account = "Liabilities:Card"
currency = "EUR"
importer = "csv"
mapping = "sources/card.toml"
"#,
        )
        .unwrap();
        let rules = Rules::parse(RULES).unwrap();
        assert_eq!(rules.check_accounts(&config), Ok(()));
        let to_card =
            Rules::parse("[[rule]]\ndesc = \"CARD STATEMENT\"\naccount = \"Liabilities:Card\"\n")
                .unwrap();
        assert_eq!(
            to_card.check_accounts(&config),
            Err(RulesError::SourceAccount("Liabilities:Card".into()))
        );
    }

    #[test]
    fn rejects_bad_regex_and_account() {
        assert_eq!(
            Rules::parse("[[rule]]\ndesc = \"(?<=x)y\"\naccount = \"Expenses:A\"\n").unwrap_err(),
            RulesError::Regex("(?<=x)y".into())
        );
        for bad in [
            "Food",
            "Expenses",
            "Expenses:food",
            "Spending:Food",
            "Expenses:Fo od",
        ] {
            let text = format!("[[rule]]\ndesc = \"x\"\naccount = {bad:?}\n");
            assert_eq!(
                Rules::parse(&text).unwrap_err(),
                RulesError::InvalidAccount(bad.into()),
                "{bad}"
            );
        }
        assert!(is_account_name("Expenses:Lebensmittel:Bäckerei-2"));
        assert!(is_account_name("Assets:2026"));
    }

    #[test]
    fn learn_key_drops_digits_and_punctuation() {
        assert_eq!(
            learn_key("CORNER GROCERY 4711 CARD-PAYMENT 18.10."),
            "corner grocery card payment"
        );
        assert_eq!(
            learn_key("Bäckerei/Café  Müller*12"),
            "bäckerei café müller"
        );
        assert_eq!(learn_key("4711 12.10.2026"), "");
    }

    const MAIN: &str = "option \"operating_currency\" \"EUR\"\ninclude \"accounts.beancount\"\ninclude \"txns.beancount\"\n";
    const ACCOUNTS: &str = "2020-01-01 open Assets:Bank:Giro EUR\n\
2020-01-01 open Assets:Bank:Card EUR\n\
2020-01-01 open Expenses:Uncategorized\n\
2020-01-01 open Expenses:Food:Groceries\n\
2020-01-01 open Expenses:Food:Other\n\
2020-01-01 open Expenses:Home\n";

    fn txn(
        date: &str,
        flag: char,
        desc: &str,
        payee: &str,
        account: &str,
        counter: &str,
    ) -> String {
        format!(
            "{date} {flag} \"{payee}\" \"\"\n  lentil_desc: \"{desc}\"\n  {account}  -1.00 EUR\n  {counter}\n\n"
        )
    }

    fn learned(txns: &str) -> Learned {
        let snapshot = Snapshot::from_files([
            ("main.beancount", MAIN),
            ("accounts.beancount", ACCOUNTS),
            ("txns.beancount", txns),
        ]);
        let ledger = ledger::load(&snapshot, "main.beancount");
        assert!(ledger.errors.is_empty(), "{:?}", ledger.errors);
        Learned::from_ledger(&ledger, "Assets:Bank:Giro", "Expenses:Uncategorized")
    }

    const GIRO: &str = "Assets:Bank:Giro";

    #[test]
    fn learns_most_frequent_account_for_identical_key() {
        let txns = [
            txn(
                "2026-01-01",
                '*',
                "CORNER GROCERY 1",
                "A",
                GIRO,
                "Expenses:Food:Groceries",
            ),
            txn(
                "2026-01-02",
                '*',
                "Corner Grocery 2",
                "B",
                GIRO,
                "Expenses:Food:Groceries",
            ),
            txn(
                "2026-01-03",
                '*',
                "CORNER GROCERY 3",
                "C",
                GIRO,
                "Expenses:Food:Other",
            ),
        ]
        .concat();
        let learned = learned(&txns);
        assert_eq!(
            learned.suggest("CORNER GROCERY 4711"),
            Some("Expenses:Food:Groceries")
        );
        assert_eq!(learned.suggest("CORNER GROCERY STORE"), None);
    }

    #[test]
    fn ties_go_to_the_most_recent() {
        let txns = [
            txn("2026-01-05", '*', "SHOP", "", GIRO, "Expenses:Food:Other"),
            txn(
                "2026-01-01",
                '*',
                "SHOP",
                "",
                GIRO,
                "Expenses:Food:Groceries",
            ),
        ]
        .concat();
        assert_eq!(learned(&txns).suggest("shop"), Some("Expenses:Food:Other"));
        let same_day = [
            txn("2026-01-05", '*', "SHOP", "", GIRO, "Expenses:Food:Other"),
            txn(
                "2026-01-05",
                '*',
                "SHOP",
                "",
                GIRO,
                "Expenses:Food:Groceries",
            ),
        ]
        .concat();
        assert_eq!(
            learned(&same_day).suggest("shop"),
            Some("Expenses:Food:Groceries")
        );
    }

    #[test]
    fn learns_only_from_reviewed_two_posting_source_transactions() {
        let three_postings = "2026-01-04 * \"\"\n  lentil_desc: \"SPLIT\"\n  Assets:Bank:Giro  -2.00 EUR\n  Expenses:Food:Other  1.00 EUR\n  Expenses:Home\n\n";
        let txns = [
            txn("2026-01-01", '!', "OPEN", "", GIRO, "Expenses:Food:Other"),
            txn(
                "2026-01-02",
                '*',
                "OTHER SOURCE",
                "",
                "Assets:Bank:Card",
                "Expenses:Food:Other",
            ),
            txn(
                "2026-01-03",
                '*',
                "UNSORTED",
                "",
                GIRO,
                "Expenses:Uncategorized",
            ),
            three_postings.to_owned(),
            txn("2026-01-05", '*', "SWAPPED", "", "Expenses:Home", GIRO),
        ]
        .concat();
        let learned = learned(&txns);
        assert_eq!(learned.suggest("OPEN"), None);
        assert_eq!(learned.suggest("OTHER SOURCE"), None);
        assert_eq!(learned.suggest("UNSORTED"), None);
        assert_eq!(learned.suggest("SPLIT"), None);
        assert_eq!(learned.suggest("SWAPPED"), Some("Expenses:Home"));
    }

    #[test]
    fn learns_from_desc_never_from_payee_or_narration() {
        let txns = [
            txn("2026-01-01", '*', "BANK WORDING", "Edited Payee", GIRO, "Expenses:Home"),
            "2026-01-02 * \"No Desc\" \"Narration\"\n  Assets:Bank:Giro  -1.00 EUR\n  Expenses:Food:Other\n\n".to_owned(),
        ]
        .concat();
        let learned = learned(&txns);
        assert_eq!(learned.suggest("BANK WORDING"), Some("Expenses:Home"));
        assert_eq!(learned.suggest("Edited Payee"), None);
        assert_eq!(learned.suggest("No Desc"), None);
        assert_eq!(learned.suggest("Narration"), None);
    }
}
