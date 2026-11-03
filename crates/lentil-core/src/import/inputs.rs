//! The files an import or `rules apply` reads, checked before they are needed.
//!
//! These checks are not part of [`validate`](crate::validate): a broken
//! `rules.toml`, `tags.toml` or mapping stops only the commands that read it,
//! never the clients that triage, so it never makes the repository read-only.

use std::collections::BTreeSet;

use jiff::civil::Date;

use crate::config::Config;
use crate::import::{Mapping, MappingError, Rules, RulesError};
use crate::ledger::Ledger;
use crate::snapshot::Snapshot;
use crate::tags::{Taxonomy, TaxonomyError};

/// A problem with the files an import or `rules apply` reads (spec sections
/// 15 and 16).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InputProblem {
    /// `rules.toml` is rejected.
    Rules(RulesError),
    /// A rule sets an account that is not open on the checked date.
    RuleAccountNotOpen {
        /// The 1-based position of the first rule that sets the account.
        rule: usize,
        /// The account.
        account: String,
    },
    /// `tags.toml` is rejected (spec section 16).
    Taxonomy(TaxonomyError),
    /// A source's mapping file does not exist. Carries its path.
    MappingMissing(String),
    /// A source's mapping file is rejected.
    Mapping {
        /// The mapping file.
        path: String,
        /// Why it is rejected.
        error: MappingError,
    },
}

/// Checks `rules.toml`, `tags.toml` and the mapping files of `config`'s
/// sources in `snapshot`, and that every rule account is open in `ledger` on
/// `today`.
///
/// Problems with the rules come first: either the reason `rules.toml` is
/// rejected, or one problem per account that is not open, with the first
/// rule that sets it. The reason `tags.toml` is rejected follows, if it is.
/// Problems with mappings come last, in source order, one per file even if
/// several sources share it.
#[must_use]
pub fn check_inputs(
    snapshot: &Snapshot,
    config: &Config,
    ledger: &Ledger,
    today: Date,
) -> Vec<InputProblem> {
    let mut problems = Vec::new();
    match Rules::load(snapshot, config) {
        Err(error) => problems.push(InputProblem::Rules(error)),
        Ok(rules) => {
            let mut seen = BTreeSet::new();
            for (rule, account) in rules.accounts() {
                if seen.insert(account) && !ledger.is_open_on(account, today) {
                    problems.push(InputProblem::RuleAccountNotOpen {
                        rule,
                        account: account.to_owned(),
                    });
                }
            }
        }
    }
    if let Some(Err(error)) = Taxonomy::load(snapshot) {
        problems.push(InputProblem::Taxonomy(error));
    }
    let mut seen = BTreeSet::new();
    for path in config.sources.iter().filter_map(|s| s.mapping.as_deref()) {
        if !seen.insert(path) {
            continue;
        }
        match Mapping::load(snapshot, path) {
            None => problems.push(InputProblem::MappingMissing(path.to_owned())),
            Some(Err(error)) => problems.push(InputProblem::Mapping {
                path: path.to_owned(),
                error,
            }),
            Some(Ok(_)) => {}
        }
    }
    problems
}

#[cfg(test)]
mod tests {
    use jiff::civil::date;

    use super::*;
    use crate::ledger;
    use crate::tags::{TAGS_FILE, TaxonomyError};

    const MAIN: &str = "option \"title\" \"Ledger\"\noption \"operating_currency\" \"EUR\"\n\ninclude \"accounts.beancount\"\n";

    const ACCOUNTS: &str = "2020-01-01 open Assets:Bank:Giro EUR
2020-01-01 open Assets:Bank:Card EUR
2020-01-01 open Assets:Wallet EUR
2020-01-01 open Equity:OpeningBalances
2020-01-01 open Expenses:Uncategorized
2020-01-01 open Expenses:Food
2020-01-01 open Expenses:Old
2026-06-01 close Expenses:Old
";

    const CONFIG: &str = r#"spec_version = 0
main_file = "main.beancount"
fallback_account = "Expenses:Uncategorized"
unattributed_account = "Expenses:Uncategorized"

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

[[source]]
id = "wallet"
account = "Assets:Wallet"
currency = "EUR"
importer = "csv"
mapping = "sources/card.toml"
"#;

    const MAPPING: &str = r#"[file]
header = ["Date", "Amount"]

[columns]
booking_date = { column = "Date", format = "%Y-%m-%d" }
amount = { column = "Amount" }

[balance]
from = "none"
"#;

    /// A valid repository state whose `rules.toml` is `rules`.
    fn snapshot(rules: &str) -> Snapshot {
        Snapshot::from_files([
            ("main.beancount", MAIN),
            ("accounts.beancount", ACCOUNTS),
            ("lentil.toml", CONFIG),
            ("rules.toml", rules),
            ("sources/giro.toml", MAPPING),
            ("sources/card.toml", MAPPING),
        ])
    }

    /// One rule per `(source, account)`; without an account the rule sets
    /// only the payee.
    fn rules(rules: &[(Option<&str>, Option<&str>)]) -> String {
        rules
            .iter()
            .map(|(source, account)| {
                let source = source.map_or_else(String::new, |s| format!("source = {s:?}, "));
                let output = account.map_or_else(
                    || "payee = \"Payee\"".to_owned(),
                    |a| format!("account = {a:?}"),
                );
                format!("[[rule]]\nmatch = {{ {source}purpose = \"x\" }}\n{output}\n\n")
            })
            .collect()
    }

    fn check_on(snapshot: &Snapshot, today: Date) -> Vec<InputProblem> {
        let config = Config::parse(snapshot.text("lentil.toml").unwrap()).unwrap();
        let ledger = ledger::load(snapshot, "main.beancount");
        check_inputs(snapshot, &config, &ledger, today)
    }

    fn check(snapshot: &Snapshot) -> Vec<InputProblem> {
        check_on(snapshot, date(2026, 11, 2))
    }

    fn not_open(rule: usize, account: &str) -> InputProblem {
        InputProblem::RuleAccountNotOpen {
            rule,
            account: account.into(),
        }
    }

    #[test]
    fn valid_inputs_have_no_problems() {
        assert_eq!(
            check(&snapshot(&rules(&[(None, Some("Expenses:Food"))]))),
            []
        );
    }

    #[test]
    fn rule_accounts_must_be_open_today() {
        let files = snapshot(&rules(&[
            (None, Some("Expenses:Food")),
            (None, Some("Expenses:Garden")),
            (None, Some("Expenses:Garden")),
            (None, None),
            (None, Some("Expenses:Old")),
        ]));
        assert_eq!(
            check(&files),
            [not_open(2, "Expenses:Garden"), not_open(5, "Expenses:Old")]
        );
        assert_eq!(
            check_on(&files, date(2026, 5, 31)),
            [not_open(2, "Expenses:Garden")]
        );
    }

    #[test]
    fn rejected_rules_hide_their_accounts() {
        let files = snapshot(&rules(&[
            (Some("girro"), None),
            (None, Some("Expenses:Garden")),
        ]));
        assert_eq!(
            check(&files),
            [InputProblem::Rules(RulesError::UnknownSource {
                rule: 1,
                id: "girro".into()
            })]
        );
    }

    #[test]
    fn each_mapping_file_is_checked_once() {
        let mut files = snapshot("");
        files.remove("sources/giro.toml");
        files.insert(
            "sources/card.toml",
            format!("{MAPPING}\n[numbers]\ndecimal = \"1\"\n"),
        );
        assert_eq!(
            check(&files),
            [
                InputProblem::MappingMissing("sources/giro.toml".into()),
                InputProblem::Mapping {
                    path: "sources/card.toml".into(),
                    error: MappingError::DigitOrSign("decimal"),
                },
            ]
        );

        let mut files = snapshot("");
        let mut latin1 = MAPPING.as_bytes().to_vec();
        latin1.extend_from_slice(b"# Empf\xe4nger\n");
        files.insert("sources/giro.toml", latin1);
        assert_eq!(
            check(&files),
            [InputProblem::Mapping {
                path: "sources/giro.toml".into(),
                error: MappingError::NotUtf8,
            }]
        );
    }

    #[test]
    fn reports_a_broken_tags_toml() {
        let invalid = InputProblem::Taxonomy(TaxonomyError::InvalidTag("#x".into()));
        let mut files = snapshot("");
        assert_eq!(check(&files), []);
        files.insert(TAGS_FILE, "tags = [\"#x\"]\n");
        assert_eq!(check(&files), std::slice::from_ref(&invalid));
        files.insert("rules.toml", "[[rule]]\n");
        files.remove("sources/giro.toml");
        let problems = check(&files);
        assert!(
            matches!(problems[0], InputProblem::Rules(RulesError::Syntax(_))),
            "{problems:?}"
        );
        assert_eq!(
            problems[1..],
            [
                invalid,
                InputProblem::MappingMissing("sources/giro.toml".into())
            ]
        );
    }

    #[test]
    fn rules_problems_come_before_mapping_problems() {
        let mut files = snapshot(&rules(&[(None, Some("Expenses:Garden"))]));
        files.remove("sources/giro.toml");
        assert_eq!(
            check(&files),
            [
                not_open(1, "Expenses:Garden"),
                InputProblem::MappingMissing("sources/giro.toml".into()),
            ]
        );
    }
}
