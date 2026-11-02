//! Suggestion rules from `rules.toml` and learned suggestions (spec section 16).

use std::collections::BTreeMap;

use jiff::civil::Date;
use regex::Regex;
use serde::Deserialize;
use thiserror::Error;

use crate::config::Config;
use crate::id;
use crate::ledger::Ledger;

/// Path of the rules file, relative to the repository root.
pub const RULES_FILE: &str = "rules.toml";

/// The parsed `rules.toml`; rules are evaluated in file order.
#[derive(Clone, Debug, Default)]
pub struct Rules {
    rules: Vec<Rule>,
}

/// One `[[rule]]`: every condition of its `match` must match, and it sets
/// the payee, the account or both.
#[derive(Clone, Debug)]
struct Rule {
    source: Option<String>,
    counterparty: Option<Regex>,
    purpose: Option<Regex>,
    booking_type: Option<Regex>,
    payee: Option<String>,
    account: Option<String>,
}

impl Rule {
    /// Whether the rule's `source` (if given) equals `source` and each of its
    /// regular expressions matches its original field.
    fn matches(&self, source: &str, original: &Original<'_>) -> bool {
        let hit = |re: &Option<Regex>, text: &str| re.as_ref().is_none_or(|re| re.is_match(text));
        self.source.as_deref().is_none_or(|s| s == source)
            && hit(&self.counterparty, original.counterparty)
            && hit(&self.purpose, original.purpose)
            && hit(&self.booking_type, original.booking_type)
    }
}

/// The original fields of one transaction (spec section 5), which rules
/// match. Each is normalized as the run file writes it; a missing field is
/// the empty string.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Original<'a> {
    /// `lentil_counterparty`.
    pub counterparty: &'a str,
    /// `lentil_purpose`.
    pub purpose: &'a str,
    /// `lentil_type`.
    pub booking_type: &'a str,
}

impl<'a> Original<'a> {
    /// The original fields from the values of `lentil_counterparty`,
    /// `lentil_purpose` and `lentil_type`, a missing one counting as empty.
    #[must_use]
    pub fn new(
        counterparty: Option<&'a str>,
        purpose: Option<&'a str>,
        booking_type: Option<&'a str>,
    ) -> Self {
        Self {
            counterparty: counterparty.unwrap_or_default(),
            purpose: purpose.unwrap_or_default(),
            booking_type: booking_type.unwrap_or_default(),
        }
    }
}

/// What the rules set for one transaction: for each field, the value of the
/// first matching rule that sets it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RuleMatch<'r> {
    /// The payee, normalized.
    pub payee: Option<&'r str>,
    /// The counter account.
    pub account: Option<&'r str>,
}

/// Why `rules.toml` is rejected.
#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum RulesError {
    /// Not TOML, an unknown key or a missing `match`.
    #[error("invalid rules.toml: {0}")]
    Syntax(String),
    /// A regular expression does not compile.
    #[error("invalid regular expression {0:?}")]
    Regex(String),
    /// A rule's `match` has no regular expression (`counterparty`, `purpose`
    /// or `type`). Carries the rule's 1-based position in the file.
    #[error("rule {0} has no regular expression in match")]
    NoCondition(usize),
    /// A rule sets neither `payee` nor `account`. Carries the rule's 1-based
    /// position in the file.
    #[error("rule {0} sets neither payee nor account")]
    NoOutput(usize),
    /// A rule's `payee` is empty after normalization. Carries the rule's
    /// 1-based position in the file.
    #[error("rule {0} has an empty payee")]
    EmptyPayee(usize),
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
    #[serde(rename = "match")]
    conditions: RawMatch,
    payee: Option<String>,
    account: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawMatch {
    source: Option<String>,
    counterparty: Option<String>,
    purpose: Option<String>,
    #[serde(rename = "type")]
    booking_type: Option<String>,
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
                let position = i + 1;
                let RawMatch {
                    source,
                    counterparty,
                    purpose,
                    booking_type,
                } = r.conditions;
                if counterparty.is_none() && purpose.is_none() && booking_type.is_none() {
                    return Err(RulesError::NoCondition(position));
                }
                if r.payee.is_none() && r.account.is_none() {
                    return Err(RulesError::NoOutput(position));
                }
                let payee = r.payee.map(|p| id::normalize_text(&p));
                if payee.as_deref() == Some("") {
                    return Err(RulesError::EmptyPayee(position));
                }
                if let Some(account) = r.account.as_ref().filter(|a| !is_account_name(a)) {
                    return Err(RulesError::InvalidAccount(account.clone()));
                }
                Ok(Rule {
                    source,
                    counterparty: compile(counterparty)?,
                    purpose: compile(purpose)?,
                    booking_type: compile(booking_type)?,
                    payee,
                    account: r.account,
                })
            })
            .collect::<Result<_, _>>()?;
        Ok(Self { rules })
    }

    /// Checks that no rule books to a source account (spec section 16).
    /// Rules that set only the payee book nowhere.
    ///
    /// # Errors
    ///
    /// [`RulesError::SourceAccount`] for the first rule that does.
    pub fn check_accounts(&self, config: &Config) -> Result<(), RulesError> {
        match self
            .rules
            .iter()
            .filter_map(|r| r.account.as_deref())
            .find(|a| config.is_source_account(a))
        {
            Some(account) => Err(RulesError::SourceAccount(account.to_owned())),
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

    /// The payee and the account the rules set for a transaction of `source`
    /// with these original fields.
    ///
    /// A rule matches if its `source` (if given) equals `source` and each of
    /// its regular expressions matches. For each field, the first matching
    /// rule that sets it wins, so a payee rule and a later account rule both
    /// apply.
    #[must_use]
    pub fn suggest(&self, source: &str, original: &Original<'_>) -> RuleMatch<'_> {
        let mut matching = self.rules.iter().filter(|r| r.matches(source, original));
        RuleMatch {
            payee: matching.clone().find_map(|r| r.payee.as_deref()),
            account: matching.find_map(|r| r.account.as_deref()),
        }
    }
}

/// Suggestions learned from reviewed transactions of one source account.
#[derive(Clone, Debug, Default)]
pub struct Learned {
    /// Learn key of the original text to the chosen counter account.
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
    /// using only the bank's original text
    /// ([`Txn::original_text`](crate::ledger::Txn::original_text)) and the
    /// counter account (PRD FR-C8).
    ///
    /// Only active transactions with exactly two postings, one on
    /// `source_account`, count. A counter account equal to `fallback_account`
    /// is never learned, so an unsorted transaction teaches nothing. For each
    /// learn key the most frequent counter account wins; ties go
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
            let Some(key) = txn
                .original_text()
                .map(|text| learn_key(&text))
                .filter(|k| !k.is_empty())
            else {
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

    /// The learned counter account for an original text, if any.
    #[must_use]
    pub fn suggest(&self, desc: &str) -> Option<&str> {
        self.by_key.get(&learn_key(desc)).map(String::as_str)
    }
}

/// The key under which an original text is learned: lowercase, digits removed,
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
match = { source = "giro", counterparty = "(?i)corner grocery" }
payee = "Corner Grocery"
account = "Expenses:Food:Groceries"

[[rule]]
match = { counterparty = "^CITY UTILITIES$", purpose = "(?i)electricity", type = "^Direct debit$" }
account = "Expenses:Home:Electricity"

[[rule]]
match = { counterparty = "(?i)grocery" }
account = "Expenses:Food:Other"
"#;

    fn original<'a>(
        counterparty: &'a str,
        purpose: &'a str,
        booking_type: &'a str,
    ) -> Original<'a> {
        Original {
            counterparty,
            purpose,
            booking_type,
        }
    }

    fn found<'r>(payee: Option<&'r str>, account: Option<&'r str>) -> RuleMatch<'r> {
        RuleMatch { payee, account }
    }

    #[test]
    fn empty_file_has_no_rules() {
        let rules = Rules::parse("").unwrap();
        assert!(rules.is_empty());
        assert_eq!(
            rules.suggest("giro", &original("x", "x", "x")),
            RuleMatch::default()
        );
    }

    #[test]
    fn first_matching_rule_wins_in_file_order() {
        let rules = Rules::parse(RULES).unwrap();
        assert_eq!(rules.len(), 3);
        assert!(!rules.is_empty());
        let grocery = original("CORNER GROCERY 4711", "", "");
        assert_eq!(
            rules.suggest("giro", &grocery),
            found(Some("Corner Grocery"), Some("Expenses:Food:Groceries"))
        );
        // The first rule is limited to giro; the third matches for other sources.
        assert_eq!(
            rules.suggest("card", &grocery),
            found(None, Some("Expenses:Food:Other"))
        );
        assert_eq!(
            rules.suggest("giro", &original("BAKERY", "", "")),
            RuleMatch::default()
        );
    }

    #[test]
    fn every_condition_must_match() {
        let rules = Rules::parse(RULES).unwrap();
        let electricity = found(None, Some("Expenses:Home:Electricity"));
        assert_eq!(
            rules.suggest(
                "giro",
                &original("CITY UTILITIES", "Electricity 10/2026", "Direct debit")
            ),
            electricity
        );
        for other in [
            original("CITY UTILITIES LTD", "Electricity", "Direct debit"),
            original("CITY UTILITIES", "Water", "Direct debit"),
            original("CITY UTILITIES", "Electricity", "Transfer"),
            original("CITY UTILITIES", "Electricity", ""),
        ] {
            assert_eq!(
                rules.suggest("giro", &other),
                RuleMatch::default(),
                "{other:?}"
            );
        }
    }

    #[test]
    fn each_field_takes_the_first_rule_that_sets_it() {
        let rules = Rules::parse(
            r#"
[[rule]]
match = { counterparty = "(?i)^corner grocery" }
payee = "Corner Grocery"

[[rule]]
match = { purpose = "(?i)card payment" }
payee = "Card"
account = "Expenses:Food:Groceries"

[[rule]]
match = { purpose = "(?i)card" }
account = "Expenses:Food:Other"
"#,
        )
        .unwrap();
        assert_eq!(
            rules.suggest("giro", &original("CORNER GROCERY 4711", "Card payment", "")),
            found(Some("Corner Grocery"), Some("Expenses:Food:Groceries"))
        );
        assert_eq!(
            rules.suggest("giro", &original("BAKERY", "Card payment", "")),
            found(Some("Card"), Some("Expenses:Food:Groceries"))
        );
        assert_eq!(
            rules.suggest("giro", &original("CORNER GROCERY", "Card", "")),
            found(Some("Corner Grocery"), Some("Expenses:Food:Other"))
        );
        assert_eq!(
            rules.suggest("giro", &original("CORNER GROCERY", "Cash", "")),
            found(Some("Corner Grocery"), None)
        );
    }

    #[test]
    fn a_missing_field_matches_as_empty() {
        let rules = Rules::parse(
            "[[rule]]\nmatch = { counterparty = \"^$\", type = \"^$\" }\naccount = \"Expenses:Misc\"\n",
        )
        .unwrap();
        let missing = Original::new(None, Some("y"), None);
        assert_eq!(missing, original("", "y", ""));
        assert_eq!(
            rules.suggest("giro", &missing),
            found(None, Some("Expenses:Misc"))
        );
        assert_eq!(
            rules.suggest("giro", &Original::new(Some("A"), Some("y"), None)),
            RuleMatch::default()
        );
        assert_eq!(
            Original::new(Some("A"), Some("B"), Some("C")),
            original("A", "B", "C")
        );
    }

    #[test]
    fn the_payee_is_normalized() {
        let rules = Rules::parse(
            "[[rule]]\nmatch = { purpose = \"x\" }\npayee = \"  Corner \\t Grocery \"\n",
        )
        .unwrap();
        assert_eq!(
            rules.suggest("giro", &original("", "x", "")),
            found(Some("Corner Grocery"), None)
        );
    }

    #[test]
    fn rejects_rules_in_the_old_format() {
        for old in [
            "[[rule]]\ndesc = \"(?i)salary\"\naccount = \"Income:Salary\"\n",
            "[[rule]]\npayee = \"^City Utilities$\"\nnarration = \"(?i)electricity\"\naccount = \"Expenses:Home\"\n",
            "[[rule]]\nsource = \"giro\"\nmatch = { purpose = \"x\" }\naccount = \"Expenses:A\"\n",
            "[[rule]]\nmatch = { desc = \"x\" }\naccount = \"Expenses:A\"\n",
            "[[rule]]\nmatch = { payee = \"x\" }\naccount = \"Expenses:A\"\n",
            "[[rule]]\nmatch = { narration = \"x\" }\naccount = \"Expenses:A\"\n",
        ] {
            assert!(
                matches!(Rules::parse(old), Err(RulesError::Syntax(_))),
                "{old}"
            );
        }
    }

    #[test]
    fn rejects_unknown_keys_and_a_missing_match() {
        let unknown =
            "[[rule]]\nmatch = { purpose = \"x\" }\naccount = \"Expenses:A\"\nprogram = \"y\"\n";
        assert!(matches!(Rules::parse(unknown), Err(RulesError::Syntax(_))));
        assert!(matches!(
            Rules::parse("[settings]\nx = 1\n"),
            Err(RulesError::Syntax(_))
        ));
        assert!(matches!(
            Rules::parse("[[rule]]\naccount = \"Expenses:A\"\n"),
            Err(RulesError::Syntax(_))
        ));
    }

    #[test]
    fn rejects_rules_without_condition() {
        let text = "[[rule]]\nmatch = { purpose = \"x\" }\naccount = \"Expenses:A\"\n\n[[rule]]\nmatch = { source = \"giro\" }\naccount = \"Expenses:B\"\n";
        assert_eq!(Rules::parse(text).unwrap_err(), RulesError::NoCondition(2));
        let empty = "[[rule]]\nmatch = {}\naccount = \"Expenses:A\"\n";
        assert_eq!(Rules::parse(empty).unwrap_err(), RulesError::NoCondition(1));
        for one in ["counterparty", "purpose", "type"] {
            let text = format!("[[rule]]\nmatch = {{ {one} = \"x\" }}\naccount = \"Expenses:A\"\n");
            assert_eq!(Rules::parse(&text).unwrap().len(), 1, "{one}");
        }
    }

    #[test]
    fn rejects_rules_without_output_or_with_an_empty_payee() {
        let text = "[[rule]]\nmatch = { purpose = \"x\" }\npayee = \"A\"\n\n[[rule]]\nmatch = { purpose = \"y\" }\n";
        assert_eq!(Rules::parse(text).unwrap_err(), RulesError::NoOutput(2));
        for empty in ["", " \t "] {
            let text = format!(
                "[[rule]]\nmatch = {{ purpose = \"x\" }}\npayee = {empty:?}\naccount = \"Expenses:A\"\n"
            );
            assert_eq!(
                Rules::parse(&text).unwrap_err(),
                RulesError::EmptyPayee(1),
                "{empty:?}"
            );
        }
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
        let to_card = Rules::parse(
            "[[rule]]\nmatch = { purpose = \"x\" }\npayee = \"Card\"\n\n[[rule]]\nmatch = { purpose = \"CARD STATEMENT\" }\naccount = \"Liabilities:Card\"\n",
        )
        .unwrap();
        assert_eq!(
            to_card.check_accounts(&config),
            Err(RulesError::SourceAccount("Liabilities:Card".into()))
        );
    }

    #[test]
    fn rejects_bad_regex_and_account() {
        for field in ["counterparty", "purpose", "type"] {
            let text = format!(
                "[[rule]]\nmatch = {{ {field} = \"(?<=x)y\" }}\naccount = \"Expenses:A\"\n"
            );
            assert_eq!(
                Rules::parse(&text).unwrap_err(),
                RulesError::Regex("(?<=x)y".into()),
                "{field}"
            );
        }
        for bad in [
            "Food",
            "Expenses",
            "Expenses:food",
            "Spending:Food",
            "Expenses:Fo od",
        ] {
            let text = format!("[[rule]]\nmatch = {{ purpose = \"x\" }}\naccount = {bad:?}\n");
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
            "{date} {flag} \"{payee}\" \"\"\n  lentil_counterparty: \"{desc}\"\n  {account}  -1.00 EUR\n  {counter}\n\n"
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
        let three_postings = "2026-01-04 * \"\"\n  lentil_counterparty: \"SPLIT\"\n  Assets:Bank:Giro  -2.00 EUR\n  Expenses:Food:Other  1.00 EUR\n  Expenses:Home\n\n";
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
    fn learns_from_the_original_text_never_from_payee_or_narration() {
        let txns = [
            txn("2026-01-01", '*', "BANK WORDING", "Edited Payee", GIRO, "Expenses:Home"),
            "2026-01-02 * \"No Original\" \"Narration\"\n  Assets:Bank:Giro  -1.00 EUR\n  Expenses:Food:Other\n\n".to_owned(),
            "2026-01-03 * \"Shop\" \"\"\n  lentil_counterparty: \"CORNER SHOP\"\n  lentil_purpose: \"CARD PAYMENT\"\n  lentil_type: \"DEBIT\"\n  Assets:Bank:Giro  -1.00 EUR\n  Expenses:Food:Groceries\n\n".to_owned(),
        ]
        .concat();
        let learned = learned(&txns);
        assert_eq!(learned.suggest("BANK WORDING"), Some("Expenses:Home"));
        assert_eq!(learned.suggest("Edited Payee"), None);
        assert_eq!(learned.suggest("No Original"), None);
        assert_eq!(learned.suggest("Narration"), None);
        assert_eq!(
            learned.suggest("CORNER SHOP CARD PAYMENT"),
            Some("Expenses:Food:Groceries")
        );
        assert_eq!(learned.suggest("CORNER SHOP"), None);
        assert_eq!(learned.suggest("CORNER SHOP CARD PAYMENT DEBIT"), None);
    }
}
