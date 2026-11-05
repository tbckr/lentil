//! Tags: valid names, the optional taxonomy in `tags.toml`, and the
//! warnings `lentil check` gives about tags outside it (spec section 16).

use std::collections::BTreeSet;

use serde::Deserialize;
use thiserror::Error;

use crate::config::Config;
use crate::import::Rules;
use crate::intent::Intent;
use crate::ledger::Ledger;
use crate::snapshot::Snapshot;

/// The tag taxonomy at the repository root (spec section 16).
pub const TAGS_FILE: &str = "tags.toml";

/// Whether `name` is a valid Beancount tag, written without `#`: one or more
/// of `A-Z a-z 0-9 - _ / .`, the tag pattern of rustledger's lexer and of
/// Python Beancount.
#[must_use]
pub fn is_tag_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'/' | b'.'))
}

/// Whether `name` matches `[a-z0-9-]+`: a trip name (spec section 9), and
/// what the app lets the user type after a tag family's prefix.
#[must_use]
pub fn is_plain_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// The tags a ledger uses, read from `tags.toml` (spec section 16).
#[derive(Clone, Debug, Default)]
pub struct Taxonomy {
    tags: BTreeSet<String>,
    families: Vec<String>,
    exclusive: Vec<Vec<String>>,
}

/// Why `tags.toml` is rejected.
#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum TaxonomyError {
    /// Not TOML, an unknown key or a value of the wrong type.
    #[error("invalid tags.toml: {0}")]
    Syntax(String),
    /// The file is not UTF-8 text.
    #[error("the file is not UTF-8 text")]
    NotUtf8,
    /// A tag, a family prefix or a group member that is not a valid tag.
    #[error("invalid tag {0:?}")]
    InvalidTag(String),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawTaxonomy {
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    families: Vec<String>,
    #[serde(default)]
    exclusive: Vec<Vec<String>>,
}

impl Taxonomy {
    /// Parses `tags.toml`. All keys are optional, so an empty file knows no
    /// tag.
    ///
    /// # Errors
    ///
    /// [`TaxonomyError::Syntax`] for text that is not TOML, an unknown key or
    /// a wrong type, and [`TaxonomyError::InvalidTag`] for the first entry
    /// that is not a valid tag: in `tags`, then in `families`, then in
    /// `exclusive`.
    pub fn parse(text: &str) -> Result<Self, TaxonomyError> {
        let raw: RawTaxonomy =
            toml::from_str(text).map_err(|e| TaxonomyError::Syntax(e.message().to_owned()))?;
        if let Some(bad) = raw
            .tags
            .iter()
            .chain(&raw.families)
            .chain(raw.exclusive.iter().flatten())
            .find(|t| !is_tag_name(t))
        {
            return Err(TaxonomyError::InvalidTag(bad.clone()));
        }
        Ok(Self {
            tags: raw.tags.into_iter().collect(),
            families: raw.families,
            // A member listed twice in a group counts once.
            exclusive: raw
                .exclusive
                .into_iter()
                .map(|group| {
                    group.into_iter().fold(Vec::new(), |mut members, tag| {
                        if !members.contains(&tag) {
                            members.push(tag);
                        }
                        members
                    })
                })
                .collect(),
        })
    }

    /// Reads and parses the `tags.toml` of a repository state; `None` if the
    /// file does not exist.
    ///
    /// # Errors
    ///
    /// The inner result is [`TaxonomyError::NotUtf8`] if the file is not
    /// UTF-8, otherwise as [`Taxonomy::parse`].
    #[must_use]
    pub fn load(snapshot: &Snapshot) -> Option<Result<Self, TaxonomyError>> {
        let bytes = snapshot.get(TAGS_FILE)?;
        Some(
            std::str::from_utf8(bytes)
                .map_err(|_| TaxonomyError::NotUtf8)
                .and_then(Self::parse),
        )
    }

    /// Whether `tag` is known: listed, or starting with a family prefix.
    #[must_use]
    pub fn is_known(&self, tag: &str) -> bool {
        self.tags.contains(tag) || self.families.iter().any(|f| tag.starts_with(f.as_str()))
    }

    /// Whether `carried` holds a tag other than `tag` from an exclusive group
    /// that contains `tag`, so that no rule may add `tag` (spec section 16).
    #[must_use]
    pub fn holds_back(&self, tag: &str, carried: &[String]) -> bool {
        self.exclusive
            .iter()
            .filter(|group| group.iter().any(|m| m == tag))
            .any(|group| carried.iter().any(|c| c != tag && group.contains(c)))
    }

    /// The tags of `carried`, other than `tag`, that share an exclusive group
    /// with `tag`: the tags picking `tag` replaces. In `carried` order,
    /// without duplicates.
    #[must_use]
    pub fn rivals<'c>(&self, tag: &str, carried: &'c [String]) -> Vec<&'c str> {
        let mut rivals: Vec<&str> = Vec::new();
        for c in carried {
            let shares = self
                .exclusive
                .iter()
                .any(|group| group.iter().any(|m| m == tag) && group.contains(c));
            if c != tag && shares && !rivals.contains(&c.as_str()) {
                rivals.push(c);
            }
        }
        rivals
    }

    /// For each exclusive group in file order, the members `carried` holds, in
    /// group order; only groups of which `carried` holds two or more members.
    #[must_use]
    pub fn clashes(&self, carried: &[String]) -> Vec<Vec<&str>> {
        self.exclusive
            .iter()
            .map(|group| {
                group
                    .iter()
                    .filter(|m| carried.contains(m))
                    .map(String::as_str)
                    .collect::<Vec<_>>()
            })
            .filter(|members| members.len() > 1)
            .collect()
    }
}

/// Where the tag picker's choices come from (PRD: FR-D12); the app shows a
/// hint unless they come from the taxonomy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChoiceSource {
    /// `tags.toml` parsed.
    Taxonomy,
    /// There is no `tags.toml`; the choices are the tags the ledger uses.
    Missing,
    /// `tags.toml` does not parse; the choices are the tags the ledger uses.
    Invalid,
}

/// A tag the picker offers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TagChoice {
    /// The tag, without `#`.
    pub tag: String,
    /// The tags of the transaction that picking it removes: the other members
    /// of its exclusive groups.
    pub replaces: Vec<String>,
}

/// What the tag picker offers for one transaction.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TagChoices {
    /// Tags the transaction does not carry yet, sorted.
    pub tags: Vec<TagChoice>,
    /// Family prefixes, in file order: the user types the rest of the tag.
    pub families: Vec<String>,
    /// Where the choices come from.
    pub source: ChoiceSource,
}

/// The tag picker's choices for a transaction that carries `carried` (PRD:
/// FR-D12). With a parsed taxonomy, its tags and families, each tag with the
/// rivals it replaces. Without one, the tags of the ledger's active
/// transactions, with no replacements and no families.
#[must_use]
pub fn tag_choices(
    taxonomy: Option<&Result<Taxonomy, TaxonomyError>>,
    ledger: &Ledger,
    carried: &[String],
) -> TagChoices {
    let fresh = |tag: &&str| !carried.iter().any(|c| c == tag);
    match taxonomy {
        Some(Ok(taxonomy)) => TagChoices {
            tags: taxonomy
                .tags
                .iter()
                .map(String::as_str)
                .filter(fresh)
                .map(|tag| TagChoice {
                    tag: tag.to_owned(),
                    replaces: taxonomy
                        .rivals(tag, carried)
                        .into_iter()
                        .map(str::to_owned)
                        .collect(),
                })
                .collect(),
            // A family listed twice is offered once.
            families: taxonomy
                .families
                .iter()
                .fold(Vec::new(), |mut families, f| {
                    if !families.contains(f) {
                        families.push(f.clone());
                    }
                    families
                }),
            source: ChoiceSource::Taxonomy,
        },
        _ => {
            let used: BTreeSet<&str> = ledger
                .transactions
                .iter()
                .flat_map(|txn| txn.tags.iter().map(String::as_str))
                .collect();
            TagChoices {
                tags: used
                    .into_iter()
                    .filter(fresh)
                    .map(|tag| TagChoice {
                        tag: tag.to_owned(),
                        replaces: Vec::new(),
                    })
                    .collect(),
                families: Vec::new(),
                source: if taxonomy.is_some() {
                    ChoiceSource::Invalid
                } else {
                    ChoiceSource::Missing
                },
            }
        }
    }
}

/// The intents of picking `tag` for transaction `id`, which carries
/// `carried`: a `remove-tag` for each rival it replaces, then the `add-tag`.
/// Without a taxonomy nothing is replaced.
#[must_use]
pub fn pick_tag(
    taxonomy: Option<&Taxonomy>,
    id: &str,
    tag: &str,
    carried: &[String],
) -> Vec<Intent> {
    let rivals = taxonomy.map(|t| t.rivals(tag, carried)).unwrap_or_default();
    rivals
        .into_iter()
        .map(|rival| Intent::RemoveTag {
            id: id.to_owned(),
            tag: rival.to_owned(),
        })
        .chain(std::iter::once(Intent::AddTag {
            id: id.to_owned(),
            tag: tag.to_owned(),
        }))
        .collect()
}

/// A tag that breaks the taxonomy of `tags.toml` (spec section 16); never a
/// finding of section 13.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TagWarning {
    /// A transaction carries a tag the taxonomy does not know.
    UnknownTag {
        /// The file of the transaction.
        file: String,
        /// 1-based line of its header.
        line: u32,
        /// The tag, without `#`.
        tag: String,
    },
    /// A transaction carries several tags of one exclusive group, in group
    /// order.
    ExclusiveTags {
        /// The file of the transaction.
        file: String,
        /// 1-based line of its header.
        line: u32,
        /// The members it carries, without `#`.
        tags: Vec<String>,
    },
    /// A rule names a tag the taxonomy does not know, in `match.tags` or
    /// `add_tags`.
    UnknownRuleTag {
        /// The rule's 1-based position in `rules.toml`.
        rule: usize,
        /// The tag, without `#`.
        tag: String,
    },
}

/// Violations of `tags.toml` in the active transactions of `ledger` and in
/// `rules.toml`, in ledger order and then rule order; empty without a
/// readable `tags.toml`, and without the rule part if `rules.toml` is
/// rejected.
///
/// Within each transaction, unknown tags come in tag order, without
/// duplicates, and clashes follow in group order.
#[must_use]
pub fn tag_warnings(snapshot: &Snapshot, config: &Config, ledger: &Ledger) -> Vec<TagWarning> {
    let Some(Ok(taxonomy)) = Taxonomy::load(snapshot) else {
        return Vec::new();
    };
    let mut warnings = Vec::new();
    for txn in &ledger.transactions {
        let mut seen: Vec<&str> = Vec::new();
        for tag in &txn.tags {
            if !taxonomy.is_known(tag) && !seen.contains(&tag.as_str()) {
                seen.push(tag);
                warnings.push(TagWarning::UnknownTag {
                    file: txn.file.clone(),
                    line: txn.line,
                    tag: tag.clone(),
                });
            }
        }
        for clash in taxonomy.clashes(&txn.tags) {
            warnings.push(TagWarning::ExclusiveTags {
                file: txn.file.clone(),
                line: txn.line,
                tags: clash.into_iter().map(str::to_owned).collect(),
            });
        }
    }
    if let Ok(rules) = Rules::load(snapshot, config) {
        for (rule, tag) in rules.tag_names() {
            if !taxonomy.is_known(tag) {
                warnings.push(TagWarning::UnknownRuleTag {
                    rule,
                    tag: tag.to_owned(),
                });
            }
        }
    }
    warnings
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::ledger;

    const EXAMPLE: &str = "tags = [\"recurring\", \"irregular\", \"amortize-year\", \"friends\", \"tax-relevant\"]\nfamilies = [\"trip-\", \"share-\"]\nexclusive = [[\"recurring\", \"irregular\"]]\n";

    fn carried(tags: &[&str]) -> Vec<String> {
        tags.iter().map(|t| (*t).to_owned()).collect()
    }

    #[test]
    fn plain_names_are_lowercase_letters_digits_and_hyphens() {
        for name in ["portugal-2026", "a", "2026", "-"] {
            assert!(is_plain_name(name), "{name:?}");
        }
        for name in ["", "Portugal", "a b", "a_b", "a.b", "a/b", "ä", "a\"b"] {
            assert!(!is_plain_name(name), "{name:?}");
        }
    }

    #[test]
    fn tag_names_follow_the_beancount_lexer() {
        for ok in ["recurring", "trip-portugal-2026", "A_b/c.d", "2026", "-"] {
            assert!(is_tag_name(ok), "{ok}");
        }
        for bad in ["", "#recurring", "a b", "ä", "a,b", "a;b", "a\"b", "a^b"] {
            assert!(!is_tag_name(bad), "{bad}");
        }
    }

    #[test]
    fn parses_the_spec_example() {
        let t = Taxonomy::parse(EXAMPLE).unwrap();
        for known in ["recurring", "tax-relevant", "trip-portugal", "share-max"] {
            assert!(t.is_known(known), "{known}");
        }
        for unknown in ["trip", "Recurring", "groceries", "xtrip-a"] {
            assert!(!t.is_known(unknown), "{unknown}");
        }
    }

    #[test]
    fn all_keys_are_optional() {
        assert!(!Taxonomy::parse("").unwrap().is_known("x"));
        assert!(
            Taxonomy::parse("families = [\"share-\"]\n")
                .unwrap()
                .is_known("share-max")
        );
    }

    #[test]
    fn a_member_listed_twice_is_one_member() {
        let t = Taxonomy::parse("exclusive = [[\"recurring\", \"recurring\", \"irregular\"]]\n")
            .unwrap();
        let c = |tags: &[&str]| tags.iter().map(|t| (*t).to_owned()).collect::<Vec<_>>();
        assert_eq!(t.clashes(&c(&["recurring"])), Vec::<Vec<&str>>::new());
        assert_eq!(
            t.clashes(&c(&["irregular", "recurring"])),
            vec![vec!["recurring", "irregular"]]
        );
        assert!(!t.holds_back("recurring", &c(&["recurring"])));
    }

    #[test]
    fn rejects_unknown_keys_wrong_types_and_invalid_tags() {
        for syntax in [
            "tag = [\"x\"]\n",
            "tags = \"recurring\"\n",
            "exclusive = [\"a\", \"b\"]\n",
            "tags = [\n",
        ] {
            assert!(
                matches!(Taxonomy::parse(syntax), Err(TaxonomyError::Syntax(_))),
                "{syntax}"
            );
        }
        for (text, tag) in [
            ("tags = [\"#recurring\"]\n", "#recurring"),
            ("families = [\"trip \"]\n", "trip "),
            ("exclusive = [[\"a\", \"b c\"]]\n", "b c"),
        ] {
            assert_eq!(
                Taxonomy::parse(text).unwrap_err(),
                TaxonomyError::InvalidTag(tag.into()),
                "{text}"
            );
        }
    }

    #[test]
    fn load_reads_tags_toml() {
        assert!(Taxonomy::load(&Snapshot::from_files([("lentil.toml", "")])).is_none());
        let ok = Snapshot::from_files([(TAGS_FILE, EXAMPLE)]);
        assert!(Taxonomy::load(&ok).unwrap().unwrap().is_known("friends"));
        let mut bad = Snapshot::default();
        bad.insert(TAGS_FILE, vec![b't', 0xff]);
        assert_eq!(
            Taxonomy::load(&bad).unwrap().unwrap_err(),
            TaxonomyError::NotUtf8
        );
    }

    #[test]
    fn holds_back_only_for_another_member_of_a_group() {
        let t = Taxonomy::parse(
            "exclusive = [[\"recurring\", \"irregular\"], [\"mine\", \"shared\"]]\n",
        )
        .unwrap();
        assert!(t.holds_back("recurring", &carried(&["irregular"])));
        assert!(t.holds_back("shared", &carried(&["friends", "mine"])));
        assert!(!t.holds_back("recurring", &carried(&["recurring"])));
        assert!(!t.holds_back("recurring", &carried(&["mine"])));
        assert!(!t.holds_back("friends", &carried(&["irregular"])));
        assert!(!t.holds_back("recurring", &[]));
    }

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
"#;
    const MAIN: &str = "option \"operating_currency\" \"EUR\"\ninclude \"accounts.beancount\"\ninclude \"imports/*/*/*.beancount\"\ninclude \"manual/*/*.beancount\"\n";
    const ACCOUNTS: &str =
        "2020-01-01 open Assets:Bank:Giro EUR\n2020-01-01 open Expenses:Uncategorized\n";
    const GIRO_RUN: &str = "imports/giro/2026/20261020T071502Z.beancount";
    const MANUAL: &str = "manual/k7p2q9xd/2026-10.beancount";
    const RULES: &str = "[[rule]]\nmatch = { tags = [\"amortize-year\"] }\nadd_tags = [\"recurring\", \"recuring\"]\n";

    fn txn(day: u8, n: u8, tags: &str) -> String {
        format!(
            "2026-10-{day:02} ! \"Shop\" \"\" {tags}\n  lentil_id: \"giro-00000000000{n}\"\n  Assets:Bank:Giro  -1.00 EUR\n  Expenses:Uncategorized\n\n"
        )
    }

    /// T1 on line 1, T2 on line 6 and T3 on line 11 of the run, and one
    /// manual entry; `tags.toml` is the spec example.
    fn tagged(rules: &str) -> Snapshot {
        let run = [
            txn(2, 1, "#recurring #trip-lisbon"),
            txn(3, 2, "#Recurring #groceries"),
            txn(4, 3, "#recurring #irregular"),
        ]
        .concat();
        let manual = "2026-10-05 * \"Cash\" \"\" #typo\n  lentil_id: \"k7p2q9xd-01JBQ0000000000000000000000\"\n  Assets:Bank:Giro  -1.00 EUR\n  Expenses:Uncategorized\n";
        Snapshot::from_files([
            ("lentil.toml", CONFIG.to_owned()),
            ("main.beancount", MAIN.to_owned()),
            ("accounts.beancount", ACCOUNTS.to_owned()),
            (GIRO_RUN, run),
            (MANUAL, manual.to_owned()),
            (TAGS_FILE, EXAMPLE.to_owned()),
            ("rules.toml", rules.to_owned()),
        ])
    }

    fn warnings(snapshot: &Snapshot) -> Vec<TagWarning> {
        let config = Config::parse(CONFIG).unwrap();
        let ledger = ledger::load(snapshot, "main.beancount");
        assert!(ledger.errors.is_empty(), "{:?}", ledger.errors);
        tag_warnings(snapshot, &config, &ledger)
    }

    fn unknown(file: &str, line: u32, tag: &str) -> TagWarning {
        TagWarning::UnknownTag {
            file: file.into(),
            line,
            tag: tag.into(),
        }
    }

    /// The warnings about the transactions of [`tagged`].
    fn transaction_warnings() -> Vec<TagWarning> {
        vec![
            unknown(GIRO_RUN, 6, "Recurring"),
            unknown(GIRO_RUN, 6, "groceries"),
            TagWarning::ExclusiveTags {
                file: GIRO_RUN.into(),
                line: 11,
                tags: vec!["recurring".into(), "irregular".into()],
            },
            unknown(MANUAL, 1, "typo"),
        ]
    }

    #[test]
    fn warns_about_unknown_and_clashing_tags() {
        let mut expected = transaction_warnings();
        expected.push(TagWarning::UnknownRuleTag {
            rule: 1,
            tag: "recuring".into(),
        });
        assert_eq!(warnings(&tagged(RULES)), expected);
    }

    #[test]
    fn unknown_tags_are_listed_once_per_transaction() {
        let mut snapshot = tagged("");
        let run = snapshot.text(GIRO_RUN).unwrap().replacen(
            "#groceries",
            "#groceries #groceries\n  #groceries",
            1,
        );
        snapshot.insert(GIRO_RUN, run);
        let t2 = |w: &&TagWarning| matches!(w, TagWarning::UnknownTag { line: 6, .. });
        assert_eq!(
            warnings(&snapshot).iter().filter(t2).collect::<Vec<_>>(),
            transaction_warnings().iter().filter(t2).collect::<Vec<_>>()
        );
    }

    #[test]
    fn no_warnings_without_a_readable_tags_toml() {
        let mut snapshot = tagged(RULES);
        snapshot.remove(TAGS_FILE);
        assert_eq!(warnings(&snapshot), Vec::new());
        snapshot.insert(TAGS_FILE, "tags = \"x\"\n");
        assert_eq!(warnings(&snapshot), Vec::new());
    }

    #[test]
    fn rejected_rules_still_give_transaction_warnings() {
        assert_eq!(
            warnings(&tagged(
                "[[rule]]\nmatch = { tags = [] }\nadd_tags = [\"recuring\"]\n"
            )),
            transaction_warnings()
        );
    }

    #[test]
    fn clashes_list_each_group_with_more_than_one_member() {
        let t = Taxonomy::parse(
            "exclusive = [[\"recurring\", \"irregular\"], [\"mine\", \"shared\"]]\n",
        )
        .unwrap();
        assert_eq!(
            t.clashes(&carried(&["irregular", "mine", "recurring", "shared"])),
            vec![vec!["recurring", "irregular"], vec!["mine", "shared"]]
        );
        assert_eq!(
            t.clashes(&carried(&["recurring", "mine"])),
            Vec::<Vec<&str>>::new()
        );
    }

    fn add(tag: &str) -> Intent {
        Intent::AddTag {
            id: "giro-000000000001".to_owned(),
            tag: tag.to_owned(),
        }
    }

    fn remove(tag: &str) -> Intent {
        Intent::RemoveTag {
            id: "giro-000000000001".to_owned(),
            tag: tag.to_owned(),
        }
    }

    #[test]
    fn rivals_are_the_carried_members_of_the_picks_groups() {
        let taxonomy = Taxonomy::parse(
            "exclusive = [[\"recurring\", \"irregular\"], [\"a\", \"b\", \"c\"], [\"c\", \"d\"]]\n",
        )
        .unwrap();
        let rivals = |tag: &str, tags: &[&str]| -> Vec<String> {
            let carried = carried(tags);
            taxonomy
                .rivals(tag, &carried)
                .into_iter()
                .map(str::to_owned)
                .collect()
        };
        assert_eq!(
            rivals("irregular", &["recurring", "friends"]),
            ["recurring"]
        );
        assert!(rivals("recurring", &["recurring"]).is_empty());
        assert!(rivals("friends", &["recurring"]).is_empty());
        assert_eq!(rivals("b", &["c", "a"]), ["c", "a"]);
        assert_eq!(rivals("c", &["d", "a", "d"]), ["d", "a"]);
    }

    #[test]
    fn choices_follow_the_taxonomy() {
        let taxonomy = Ok(Taxonomy::parse(EXAMPLE).unwrap());
        let choices = tag_choices(
            Some(&taxonomy),
            &Ledger::default(),
            &carried(&["recurring"]),
        );
        let choice = |tag: &str, replaces: &[&str]| TagChoice {
            tag: tag.to_owned(),
            replaces: carried(replaces),
        };
        assert_eq!(
            choices,
            TagChoices {
                tags: vec![
                    choice("amortize-year", &[]),
                    choice("friends", &[]),
                    choice("irregular", &["recurring"]),
                    choice("tax-relevant", &[]),
                ],
                families: carried(&["trip-", "share-"]),
                source: ChoiceSource::Taxonomy,
            }
        );
    }

    #[test]
    fn a_family_listed_twice_is_offered_once() {
        let taxonomy =
            Ok(Taxonomy::parse("families = [\"trip-\", \"share-\", \"trip-\"]\n").unwrap());
        let choices = tag_choices(Some(&taxonomy), &Ledger::default(), &[]);
        assert_eq!(choices.families, carried(&["trip-", "share-"]));
    }

    #[test]
    fn without_a_readable_taxonomy_choices_are_the_ledgers_tags() {
        let snapshot = Snapshot::from_files([
            ("main.beancount", MAIN.to_owned()),
            ("accounts.beancount", ACCOUNTS.to_owned()),
            (GIRO_RUN, [txn(2, 1, "#b #a"), txn(3, 2, "#a #c")].concat()),
        ]);
        let ledger = ledger::load(&snapshot, "main.beancount");
        let expected = |source| TagChoices {
            tags: ["a", "b"]
                .into_iter()
                .map(|tag| TagChoice {
                    tag: tag.to_owned(),
                    replaces: Vec::new(),
                })
                .collect(),
            families: Vec::new(),
            source,
        };
        let carried = carried(&["c"]);
        assert_eq!(
            tag_choices(None, &ledger, &carried),
            expected(ChoiceSource::Missing)
        );
        assert_eq!(
            tag_choices(Some(&Err(TaxonomyError::NotUtf8)), &ledger, &carried),
            expected(ChoiceSource::Invalid)
        );
    }

    #[test]
    fn picking_a_tag_removes_its_rivals_first() {
        let taxonomy = Taxonomy::parse(EXAMPLE).unwrap();
        let carried = carried(&["recurring", "friends"]);
        assert_eq!(
            pick_tag(Some(&taxonomy), "giro-000000000001", "irregular", &carried),
            vec![remove("recurring"), add("irregular")]
        );
        assert_eq!(
            pick_tag(None, "giro-000000000001", "irregular", &carried),
            vec![add("irregular")]
        );
    }
}
