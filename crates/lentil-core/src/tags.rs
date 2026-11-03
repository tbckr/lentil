//! Tags: valid names and the optional taxonomy in `tags.toml` (spec section 16).

use std::collections::BTreeSet;

use serde::Deserialize;
use thiserror::Error;

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

#[cfg(test)]
mod tests {
    use super::*;

    const EXAMPLE: &str = "tags = [\"recurring\", \"irregular\", \"amortize-year\", \"friends\", \"tax-relevant\"]\nfamilies = [\"trip-\", \"share-\"]\nexclusive = [[\"recurring\", \"irregular\"]]\n";

    fn carried(tags: &[&str]) -> Vec<String> {
        tags.iter().map(|t| (*t).to_owned()).collect()
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
}
