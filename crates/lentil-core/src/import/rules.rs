//! Suggestion rules from `rules.toml`, which set the payee and the account and
//! add tags (spec section 16). Learned suggestions live in [`crate::learn`].

use regex::Regex;
use serde::Deserialize;
use thiserror::Error;

use crate::config::Config;
use crate::id;
use crate::ledger::Txn;
use crate::snapshot::Snapshot;
use crate::tags::is_tag_name;

/// Path of the rules file, relative to the repository root.
pub const RULES_FILE: &str = "rules.toml";

/// The parsed `rules.toml`; rules are evaluated in file order.
#[derive(Clone, Debug, Default)]
pub struct Rules {
    rules: Vec<Rule>,
}

/// One `[[rule]]`: every condition of its `match` must match, and it sets
/// the payee, the account, adds tags or several of them.
#[derive(Clone, Debug)]
struct Rule {
    source: Option<String>,
    counterparty: Option<Regex>,
    purpose: Option<Regex>,
    booking_type: Option<Regex>,
    /// `match.tags`: tags the transaction must all carry; empty if absent.
    tags: Vec<String>,
    payee: Option<String>,
    account: Option<String>,
    /// `add_tags`; empty if absent.
    add_tags: Vec<String>,
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

    /// Whether the rule is `rule`: the same regular expressions, as written,
    /// and the same account, and no other key.
    fn is(&self, rule: &EditorRule) -> bool {
        self.counterparty.as_ref().map(Regex::as_str) == rule.counterparty.as_deref()
            && self.purpose.as_ref().map(Regex::as_str) == rule.purpose.as_deref()
            && self.account.as_deref() == Some(rule.account.as_str())
            && self.source.is_none()
            && self.booking_type.is_none()
            && self.tags.is_empty()
            && self.payee.is_none()
            && self.add_tags.is_empty()
    }
}

/// A rule in the editor shape (spec section 16): a `match` with only
/// `counterparty`, `purpose` or both, and only `account`. Editors insert it
/// before the first rule of `rules.toml` through `add-rule` (spec
/// section 10).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EditorRule {
    /// The regular expression on `lentil_counterparty`, if any.
    pub counterparty: Option<String>,
    /// The regular expression on `lentil_purpose`, if any.
    pub purpose: Option<String>,
    /// The counter account the rule sets.
    pub account: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawEditorRules {
    rule: Vec<RawEditorRule>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawEditorRule {
    #[serde(rename = "match")]
    conditions: RawEditorMatch,
    account: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawEditorMatch {
    counterparty: Option<String>,
    purpose: Option<String>,
}

/// The regular expression that matches a text containing `words` in this
/// order, each bounded by a non-letter or the edge of the text,
/// case-insensitively; `None` without words.
///
/// Letters are characters of Unicode general category L (`\pL`), as for
/// [`crate::learn::words`]. The expression avoids `\b`, which RE2 defines
/// for ASCII only: each word is followed by a non-letter, and anything
/// between two words ends in a non-letter.
fn words_pattern(words: &[String]) -> Option<String> {
    if words.is_empty() {
        return None;
    }
    let words: Vec<String> = words.iter().map(|w| regex::escape(w)).collect();
    Some(format!(
        r"(?i)(?:^|\PL){}(?:\PL|$)",
        words.join(r"\PL(?:.*\PL)?")
    ))
}

impl EditorRule {
    /// The rule that sets `account` for every transaction whose counterparty
    /// contains the words `counterparty` and whose purpose contains the
    /// words `purpose`, each in text order and bounded by non-letters or the
    /// edge of the text, case-insensitively. A field without words is not
    /// matched. `None` if both are empty.
    #[must_use]
    pub fn from_words(counterparty: &[String], purpose: &[String], account: &str) -> Option<Self> {
        let counterparty = words_pattern(counterparty);
        let purpose = words_pattern(purpose);
        if counterparty.is_none() && purpose.is_none() {
            return None;
        }
        Some(Self {
            counterparty,
            purpose,
            account: account.to_owned(),
        })
    }

    /// The rule as a `[[rule]]` block of `rules.toml`, ending in a line
    /// break. The regular expressions are TOML literal strings, so they need
    /// no escaping, and the account is written as it is.
    ///
    /// A literal string cannot hold `'` or a line break, and the account is
    /// not escaped, so the block holds exactly this rule only if
    /// [`Self::parse_blocks`] reads it back as this rule; `add-rule` checks
    /// that. The words of [`Self::from_words`] and a valid account name
    /// always do.
    #[must_use]
    pub fn block(&self) -> String {
        let conditions: Vec<String> = [
            ("counterparty", &self.counterparty),
            ("purpose", &self.purpose),
        ]
        .into_iter()
        .filter_map(|(key, pattern)| Some(format!("{key} = '{}'", pattern.as_deref()?)))
        .collect();
        format!(
            "[[rule]]\nmatch = {{ {} }}\naccount = \"{}\"\n",
            conditions.join(", "),
            self.account
        )
    }

    /// The rules of `text` if it is TOML that holds only rules, at least
    /// one, each with a `match` of only `counterparty`, `purpose` or both
    /// and an `account`, and no other key; otherwise `None`.
    #[must_use]
    pub fn parse_blocks(text: &str) -> Option<Vec<Self>> {
        let raw: RawEditorRules = toml::from_str(text).ok()?;
        if raw.rule.is_empty() {
            return None;
        }
        raw.rule
            .into_iter()
            .map(|r| {
                let RawEditorMatch {
                    counterparty,
                    purpose,
                } = r.conditions;
                (counterparty.is_some() || purpose.is_some()).then_some(Self {
                    counterparty,
                    purpose,
                    account: r.account,
                })
            })
            .collect()
    }
}

/// `text`, a `rules.toml` that parses, with `block` inserted as its first
/// rule (spec section 10).
///
/// The block goes before the first `[[rule]]` header and before the comment
/// lines directly above it, unless they reach the start of the file, and one
/// blank line separates it from what follows. A file without rules gets the
/// block at its end, after one blank line. A leading byte order mark stays
/// first, and the file starts after it. Every byte of `text` stays as it is;
/// the block keeps its own line breaks.
#[must_use]
pub fn insert_rule(text: &str, block: &str) -> String {
    // TOML skips a leading byte order mark, and `trim` does not remove it.
    let body = text.strip_prefix('\u{feff}').unwrap_or(text);
    let bom = text.len() - body.len();
    // `rule` is the only key of a rules.toml that parses, so above its first
    // rule there are only blank and comment lines: the first other line
    // starts the first rule, and no multi-line string starts above it.
    let mut start = bom;
    // Where the comment lines directly above the current line start.
    let mut comments: Option<usize> = None;
    for line in body.split_inclusive('\n') {
        let content = line.trim();
        if content.starts_with('#') {
            comments.get_or_insert(start);
        } else if content.is_empty() {
            comments = None;
        } else {
            if content.starts_with("[[") {
                let at = comments.filter(|&c| c > bom).unwrap_or(start);
                return format!("{}{block}\n{}", &text[..at], &text[at..]);
            }
            break;
        }
        start += line.len();
    }
    let separator = if body.is_empty() {
        ""
    } else if body.ends_with('\n') {
        "\n"
    } else {
        "\n\n"
    };
    format!("{text}{separator}{block}")
}

/// The original fields of one transaction (spec section 5), which rules
/// match and learned suggestions ([`crate::learn`]) read. Each is normalized
/// as the run file writes it; a missing field is the empty string.
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

impl<'a> From<&'a Txn> for Original<'a> {
    /// The original fields of an indexed transaction, through the same
    /// constructor the importer uses, so rules match it as they did at import.
    fn from(txn: &'a Txn) -> Self {
        Self::new(
            txn.counterparty.as_deref(),
            txn.purpose.as_deref(),
            txn.booking_type.as_deref(),
        )
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
    /// The file is not UTF-8 text.
    #[error("the file is not UTF-8 text")]
    NotUtf8,
    /// A regular expression does not compile.
    #[error("invalid regular expression {0:?}")]
    Regex(String),
    /// A rule's `match` has neither a regular expression (`counterparty`,
    /// `purpose` or `type`) nor `tags`. Carries the rule's 1-based position in
    /// the file.
    #[error("rule {0} has neither a regular expression nor tags in match")]
    NoCondition(usize),
    /// A rule sets no `payee`, `account` or `add_tags`. Carries the rule's
    /// 1-based position in the file.
    #[error("rule {0} sets no payee, account or tags")]
    NoOutput(usize),
    /// A rule's `match.tags` or `add_tags` is an empty list. Carries the
    /// rule's 1-based position in the file.
    #[error("rule {0} has an empty list of tags")]
    EmptyTags(usize),
    /// A tag in `match.tags` or `add_tags` is not a valid Beancount tag.
    #[error("rule {rule} has the invalid tag {tag:?}")]
    InvalidTag {
        /// The rule's 1-based position in the file.
        rule: usize,
        /// The tag as written.
        tag: String,
    },
    /// A rule with a `tags` condition sets the payee or the account (spec
    /// section 16). Carries the rule's 1-based position in the file.
    #[error("rule {0} matches tags, so it may only add tags")]
    TagMatchSetsField(usize),
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
    /// A rule's `match.source` names no source of `lentil.toml` (spec
    /// section 16).
    #[error("rule {rule} names the source {id:?}, which lentil.toml does not define")]
    UnknownSource {
        /// The rule's 1-based position in the file.
        rule: usize,
        /// The source ID the rule names.
        id: String,
    },
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
    add_tags: Option<Vec<String>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawMatch {
    source: Option<String>,
    counterparty: Option<String>,
    purpose: Option<String>,
    #[serde(rename = "type")]
    booking_type: Option<String>,
    tags: Option<Vec<String>>,
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
                    tags,
                } = r.conditions;
                if counterparty.is_none()
                    && purpose.is_none()
                    && booking_type.is_none()
                    && tags.is_none()
                {
                    return Err(RulesError::NoCondition(position));
                }
                if r.payee.is_none() && r.account.is_none() && r.add_tags.is_none() {
                    return Err(RulesError::NoOutput(position));
                }
                if [&tags, &r.add_tags]
                    .into_iter()
                    .any(|list| list.as_ref().is_some_and(Vec::is_empty))
                {
                    return Err(RulesError::EmptyTags(position));
                }
                if let Some(bad) = tags
                    .iter()
                    .chain(&r.add_tags)
                    .flatten()
                    .find(|t| !is_tag_name(t))
                {
                    return Err(RulesError::InvalidTag {
                        rule: position,
                        tag: bad.clone(),
                    });
                }
                if tags.is_some() && (r.payee.is_some() || r.account.is_some()) {
                    return Err(RulesError::TagMatchSetsField(position));
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
                    tags: tags.unwrap_or_default(),
                    payee,
                    account: r.account,
                    add_tags: r.add_tags.unwrap_or_default(),
                })
            })
            .collect::<Result<_, _>>()?;
        Ok(Self { rules })
    }

    /// Reads, parses and checks the `rules.toml` of a repository state. A
    /// missing file has no rules.
    ///
    /// # Errors
    ///
    /// [`RulesError::NotUtf8`] if the file is not UTF-8, otherwise as
    /// [`Rules::parse`] and [`Rules::check`].
    pub fn load(snapshot: &Snapshot, config: &Config) -> Result<Self, RulesError> {
        let text = match snapshot.get(RULES_FILE) {
            Some(bytes) => std::str::from_utf8(bytes).map_err(|_| RulesError::NotUtf8)?,
            None => "",
        };
        let rules = Self::parse(text)?;
        rules.check(config)?;
        Ok(rules)
    }

    /// Checks the rules against `config` (spec section 16): no rule books to
    /// a source account, and every `match.source` names a configured
    /// source. Rules that set only the payee book nowhere.
    ///
    /// # Errors
    ///
    /// [`RulesError::SourceAccount`] or [`RulesError::UnknownSource`] for
    /// the first rule in file order that breaks either.
    pub fn check(&self, config: &Config) -> Result<(), RulesError> {
        for (i, rule) in self.rules.iter().enumerate() {
            if let Some(account) = rule
                .account
                .as_deref()
                .filter(|a| config.is_source_account(a))
            {
                return Err(RulesError::SourceAccount(account.to_owned()));
            }
            if let Some(id) = rule
                .source
                .as_deref()
                .filter(|id| config.source(id).is_none())
            {
                return Err(RulesError::UnknownSource {
                    rule: i + 1,
                    id: id.to_owned(),
                });
            }
        }
        Ok(())
    }

    /// The 1-based position and the account of each rule that sets one, in
    /// file order.
    pub fn accounts(&self) -> impl Iterator<Item = (usize, &str)> {
        self.rules
            .iter()
            .enumerate()
            .filter_map(|(i, r)| Some((i + 1, r.account.as_deref()?)))
    }

    /// Whether the first rule is identical to `rule`: the same regular
    /// expressions, as written, and the same account, and no other key
    /// (spec section 10, `add-rule`).
    #[must_use]
    pub fn is_first(&self, rule: &EditorRule) -> bool {
        self.rules.first().is_some_and(|r| r.is(rule))
    }

    /// Whether some rule lists `tag` in its `add_tags`.
    #[must_use]
    pub fn adds(&self, tag: &str) -> bool {
        self.rules
            .iter()
            .any(|r| r.add_tags.iter().any(|t| t == tag))
    }

    /// The 1-based position of each rule and each distinct tag it names, in
    /// file order: those of `match.tags` first, then those of `add_tags`.
    pub fn tag_names(&self) -> impl Iterator<Item = (usize, &str)> {
        self.rules.iter().enumerate().flat_map(|(i, r)| {
            let mut seen: Vec<&str> = Vec::new();
            r.tags
                .iter()
                .chain(&r.add_tags)
                .map(String::as_str)
                .filter(move |t| {
                    let new = !seen.contains(t);
                    if new {
                        seen.push(t);
                    }
                    new
                })
                .map(move |t| (i + 1, t))
        })
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

    /// The tags the rules add to a transaction of `source` with these
    /// original fields that carries the tags `start`, in the order the
    /// evaluation adds them (spec section 16).
    ///
    /// The tags of all matching rules are united, evaluated to a fixed point
    /// in rounds: each round matches every rule against the tags at its
    /// start and adds their tags that are not carried yet, rules in file
    /// order and each rule's tags in `add_tags` order. A tag of `start` is
    /// never returned, and neither is a tag for which `held_back` is true.
    #[must_use]
    pub fn tags<'r>(
        &'r self,
        source: &str,
        original: &Original<'_>,
        start: &[&str],
        held_back: impl Fn(&str) -> bool,
    ) -> Vec<&'r str> {
        let matching: Vec<&Rule> = self
            .rules
            .iter()
            .filter(|r| !r.add_tags.is_empty() && r.matches(source, original))
            .collect();
        let mut carried: Vec<&str> = start.to_vec();
        let mut added: Vec<&'r str> = Vec::new();
        loop {
            let mut round: Vec<&'r str> = Vec::new();
            for rule in matching
                .iter()
                .filter(|r| r.tags.iter().all(|t| carried.contains(&t.as_str())))
            {
                for tag in rule.add_tags.iter().map(String::as_str) {
                    if !carried.contains(&tag) && !round.contains(&tag) && !held_back(tag) {
                        round.push(tag);
                    }
                }
            }
            if round.is_empty() {
                return added;
            }
            carried.extend(&round);
            added.extend(round);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    const TAG_RULES: &str = r#"
[[rule]]
match = { counterparty = "(?i)^streamflix" }
account = "Expenses:Subscriptions"
add_tags = ["recurring"]

[[rule]]
match = { counterparty = "(?i)streamflix" }
add_tags = ["entertainment", "recurring"]
"#;

    fn none(_: &str) -> bool {
        false
    }

    #[test]
    fn tags_of_all_matching_rules_are_united() {
        let rules = Rules::parse(TAG_RULES).unwrap();
        let streamflix = original("STREAMFLIX 0815", "", "");
        assert_eq!(
            rules.tags("giro", &streamflix, &[], none),
            vec!["recurring", "entertainment"]
        );
        assert_eq!(
            rules.suggest("giro", &streamflix).account,
            Some("Expenses:Subscriptions")
        );
        assert_eq!(
            rules.tags("giro", &streamflix, &["recurring"], none),
            vec!["entertainment"]
        );
        assert_eq!(
            Rules::parse(RULES).unwrap().tags(
                "giro",
                &original("CORNER GROCERY", "", ""),
                &[],
                none
            ),
            Vec::<&str>::new()
        );
    }

    #[test]
    fn tag_rules_chain_to_a_fixed_point_whatever_their_order() {
        let chain = "[[rule]]\nmatch = { tags = [\"amortize-year\"] }\nadd_tags = [\"recurring\"]\n\n[[rule]]\nmatch = { counterparty = \"(?i)^webhost annual\" }\nadd_tags = [\"amortize-year\"]\n";
        let webhost = original("WEBHOST ANNUAL", "", "");
        assert_eq!(
            Rules::parse(chain)
                .unwrap()
                .tags("giro", &webhost, &[], none),
            vec!["amortize-year", "recurring"]
        );
        let three = "[[rule]]\nmatch = { tags = [\"b\"] }\nadd_tags = [\"c\"]\n\n[[rule]]\nmatch = { tags = [\"a\"] }\nadd_tags = [\"b\"]\n\n[[rule]]\nmatch = { purpose = \"x\" }\nadd_tags = [\"a\"]\n";
        assert_eq!(
            Rules::parse(three)
                .unwrap()
                .tags("giro", &original("", "x", ""), &[], none),
            vec!["a", "b", "c"]
        );
        assert_eq!(
            Rules::parse(chain).unwrap().tags(
                "giro",
                &original("OTHER", "", ""),
                &["amortize-year"],
                none
            ),
            vec!["recurring"]
        );
    }

    #[test]
    fn a_round_adds_rules_in_file_order_and_tags_in_add_tags_order() {
        let rules = "[[rule]]\nmatch = { purpose = \"x\" }\nadd_tags = [\"b\", \"a\"]\n\n[[rule]]\nmatch = { purpose = \"x\" }\nadd_tags = [\"c\", \"a\"]\n";
        assert_eq!(
            Rules::parse(rules)
                .unwrap()
                .tags("giro", &original("", "x", ""), &[], none),
            vec!["b", "a", "c"]
        );
    }

    #[test]
    fn a_tag_condition_needs_every_listed_tag_and_the_source() {
        let rules = Rules::parse(
            "[[rule]]\nmatch = { source = \"giro\", tags = [\"a\", \"b\"] }\nadd_tags = [\"c\"]\n",
        )
        .unwrap();
        let any = original("", "", "");
        assert_eq!(rules.tags("giro", &any, &["a"], none), Vec::<&str>::new());
        assert_eq!(rules.tags("giro", &any, &["b", "a"], none), vec!["c"]);
        assert_eq!(
            rules.tags("card", &any, &["a", "b"], none),
            Vec::<&str>::new()
        );
    }

    #[test]
    fn a_held_back_tag_is_skipped_and_the_rule_adds_its_other_tags() {
        let rules = Rules::parse(&format!(
            "{TAG_RULES}\n[[rule]]\nmatch = {{ tags = [\"recurring\"] }}\nadd_tags = [\"subscription\"]\n"
        ))
        .unwrap();
        let held = |t: &str| t == "recurring";
        assert_eq!(
            rules.tags("giro", &original("STREAMFLIX", "", ""), &[], held),
            vec!["entertainment"]
        );
    }

    #[test]
    fn rejects_invalid_tag_rules() {
        let cases = [
            (
                "match = { tags = [] }\nadd_tags = [\"x\"]",
                RulesError::EmptyTags(1),
            ),
            (
                "match = { purpose = \"x\" }\nadd_tags = []",
                RulesError::EmptyTags(1),
            ),
            (
                "match = { purpose = \"x\" }\nadd_tags = [\"#x\"]",
                RulesError::InvalidTag {
                    rule: 1,
                    tag: "#x".into(),
                },
            ),
            (
                "match = { tags = [\"a b\"] }\nadd_tags = [\"x\"]",
                RulesError::InvalidTag {
                    rule: 1,
                    tag: "a b".into(),
                },
            ),
            (
                "match = { tags = [\"a\"] }\naccount = \"Expenses:A\"",
                RulesError::TagMatchSetsField(1),
            ),
            (
                "match = { tags = [\"a\"], purpose = \"x\" }\npayee = \"P\"",
                RulesError::TagMatchSetsField(1),
            ),
            (
                "match = { source = \"giro\" }\nadd_tags = [\"b\"]",
                RulesError::NoCondition(1),
            ),
            ("match = { purpose = \"x\" }", RulesError::NoOutput(1)),
        ];
        for (rule, error) in cases {
            assert_eq!(
                Rules::parse(&format!("[[rule]]\n{rule}\n")).unwrap_err(),
                error,
                "{rule}"
            );
        }
        assert!(matches!(
            Rules::parse("[[rule]]\nmatch = { purpose = \"x\" }\nadd_tags = \"x\"\n"),
            Err(RulesError::Syntax(_))
        ));
        assert_eq!(
            Rules::parse("[[rule]]\nmatch = { purpose = \"x\" }\nadd_tags = [\"a\"]\n\n[[rule]]\nmatch = { purpose = \"y\" }\nadd_tags = [\"b\", \"c;\"]\n")
                .unwrap_err(),
            RulesError::InvalidTag {
                rule: 2,
                tag: "c;".into()
            }
        );
        assert_eq!(
            Rules::parse("[[rule]]\nmatch = { tags = [\"a\"] }\nadd_tags = [\"b\"]\n")
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn adds_and_tag_names_list_the_tags_rules_name() {
        let rules = Rules::parse("[[rule]]\nmatch = { purpose = \"x\" }\nadd_tags = [\"a\", \"b\"]\n\n[[rule]]\nmatch = { tags = [\"a\"] }\nadd_tags = [\"c\", \"a\"]\n").unwrap();
        assert!(rules.adds("a") && rules.adds("c"));
        assert!(!rules.adds("z"));
        assert_eq!(
            rules.tag_names().collect::<Vec<_>>(),
            vec![(1, "a"), (1, "b"), (2, "a"), (2, "c")]
        );
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

    /// The sources `giro` and `card`.
    fn config() -> Config {
        Config::parse(
            r#"spec_version = 0
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
"#,
        )
        .unwrap()
    }

    #[test]
    fn rejects_rules_that_book_to_a_source_account() {
        let config = config();
        let rules = Rules::parse(RULES).unwrap();
        assert_eq!(rules.check(&config), Ok(()));
        let to_card = Rules::parse(
            "[[rule]]\nmatch = { purpose = \"x\" }\npayee = \"Card\"\n\n[[rule]]\nmatch = { purpose = \"CARD STATEMENT\" }\naccount = \"Liabilities:Card\"\n",
        )
        .unwrap();
        assert_eq!(
            to_card.check(&config),
            Err(RulesError::SourceAccount("Liabilities:Card".into()))
        );
    }

    #[test]
    fn rejects_rules_for_unknown_sources() {
        let config = config();
        let rule = |source: &str, out: &str| {
            format!("[[rule]]\nmatch = {{ source = {source:?}, purpose = \"x\" }}\n{out}\n\n")
        };
        let unknown = format!(
            "{}{}",
            rule("card", "payee = \"A\""),
            rule("girro", "payee = \"B\"")
        );
        assert_eq!(
            Rules::parse(&unknown).unwrap().check(&config),
            Err(RulesError::UnknownSource {
                rule: 2,
                id: "girro".into()
            })
        );
        // The first offending rule in file order wins, whatever its kind.
        let to_card = "[[rule]]\nmatch = { purpose = \"y\" }\naccount = \"Liabilities:Card\"\n\n";
        let first_unknown = format!("{}{to_card}", rule("girro", "payee = \"B\""));
        assert_eq!(
            Rules::parse(&first_unknown).unwrap().check(&config),
            Err(RulesError::UnknownSource {
                rule: 1,
                id: "girro".into()
            })
        );
        let first_card = format!("{to_card}{}", rule("girro", "payee = \"B\""));
        assert_eq!(
            Rules::parse(&first_card).unwrap().check(&config),
            Err(RulesError::SourceAccount("Liabilities:Card".into()))
        );
    }

    #[test]
    fn load_reads_rules_toml_and_checks_it() {
        let config = config();
        assert!(Rules::load(&Snapshot::new(), &config).unwrap().is_empty());
        let with = |content: &[u8]| Snapshot::from_files([(RULES_FILE, content)]);
        assert_eq!(
            Rules::load(&with(RULES.as_bytes()), &config).unwrap().len(),
            3
        );
        assert_eq!(
            Rules::load(&with(b"# Regel f\xfcr die B\xe4ckerei\n"), &config).unwrap_err(),
            RulesError::NotUtf8
        );
        let unknown = b"[[rule]]\nmatch = { source = \"girro\", purpose = \"x\" }\npayee = \"B\"\n";
        assert_eq!(
            Rules::load(&with(unknown), &config).unwrap_err(),
            RulesError::UnknownSource {
                rule: 1,
                id: "girro".into()
            }
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

    fn strings(words: &[&str]) -> Vec<String> {
        words.iter().map(|w| (*w).to_owned()).collect()
    }

    /// Whether the editor rule on these words books a transaction with
    /// these original fields.
    fn books(counterparty: &[&str], purpose: &[&str], original: &Original<'_>) -> bool {
        let rule = EditorRule::from_words(&strings(counterparty), &strings(purpose), "Expenses:A")
            .unwrap();
        let rules = Rules::parse(&rule.block()).unwrap();
        rules.suggest("giro", original).account == Some("Expenses:A")
    }

    #[test]
    fn a_block_is_a_rule_of_the_editor_shape() {
        let rule =
            EditorRule::from_words(&[], &["streamflix".into()], "Expenses:Subscriptions").unwrap();
        assert_eq!(
            rule.block(),
            "[[rule]]\nmatch = { purpose = '(?i)(?:^|\\PL)streamflix(?:\\PL|$)' }\naccount = \"Expenses:Subscriptions\"\n"
        );
        assert_eq!(
            EditorRule::parse_blocks(&rule.block()),
            Some(vec![rule.clone()])
        );
        let rules = Rules::parse(&rule.block()).unwrap();
        assert!(rules.is_first(&rule));
        assert!(EditorRule::from_words(&[], &[], "Expenses:Subscriptions").is_none());

        let both =
            EditorRule::from_words(&["zahlfix".into()], &["abo".into()], "Expenses:A").unwrap();
        assert_eq!(
            both.block(),
            "[[rule]]\nmatch = { counterparty = '(?i)(?:^|\\PL)zahlfix(?:\\PL|$)', purpose = '(?i)(?:^|\\PL)abo(?:\\PL|$)' }\naccount = \"Expenses:A\"\n"
        );
        assert_eq!(
            EditorRule::parse_blocks(&format!("{}\n{}", rule.block(), both.block())),
            Some(vec![rule, both])
        );
    }

    #[test]
    fn only_an_identical_first_rule_is_first() {
        let rule =
            EditorRule::from_words(&["corner".into()], &["card".into()], "Expenses:A").unwrap();
        let block = rule.block();
        let account = "account = \"Expenses:A\"";
        assert!(
            Rules::parse(&format!("{block}\n{RULES}"))
                .unwrap()
                .is_first(&rule)
        );
        // An identical rule further down does not count.
        assert!(
            !Rules::parse(&format!("{RULES}\n{block}"))
                .unwrap()
                .is_first(&rule)
        );
        assert!(!Rules::parse("").unwrap().is_first(&rule));
        for other in [
            block.replace(account, "payee = \"P\"\naccount = \"Expenses:A\""),
            block.replace(account, "account = \"Expenses:A\"\nadd_tags = [\"x\"]"),
            block.replace("match = { ", "match = { source = \"giro\", "),
            block.replace("match = { ", "match = { type = \"x\", "),
            block.replace(account, "account = \"Expenses:B\""),
            block.replace("corner", "corners"),
            block.replace("card", "cards"),
            EditorRule::from_words(&["corner".into()], &[], "Expenses:A")
                .unwrap()
                .block(),
            EditorRule::from_words(&[], &["card".into()], "Expenses:A")
                .unwrap()
                .block(),
        ] {
            assert!(!Rules::parse(&other).unwrap().is_first(&rule), "{other}");
        }
    }

    #[test]
    fn several_words_match_in_order_at_letter_boundaries() {
        let words = ["zahlfix", "europe"];
        let counterparty = |text| original(text, "", "");
        for hit in [
            "Zahlfix Europe S.A.",
            "ZAHLFIX (EUROPE)",
            "zahlfix-europe",
            "Pay: zahlfix 0815 Europe",
        ] {
            assert!(books(&words, &[], &counterparty(hit)), "{hit}");
        }
        for miss in [
            "Europe Zahlfix",
            "ZahlfixEurope",
            "zahlfixs europe",
            "zahlfix european",
            "Zahlfix",
            "",
        ] {
            assert!(!books(&words, &[], &counterparty(miss)), "{miss}");
        }
        // Digits separate words, as in the words of a text.
        assert!(books(&["streamflix"], &[], &counterparty("STREAMFLIX0815")));
        // A word given twice must occur twice.
        assert!(books(&["abo", "abo"], &[], &counterparty("Abo / abo")));
        assert!(!books(&["abo", "abo"], &[], &counterparty("Abo")));
        // Every field with words must match.
        assert!(books(
            &["zahlfix"],
            &["streamflix"],
            &original("Zahlfix", "Streamflix Abo", "")
        ));
        assert!(!books(
            &["zahlfix"],
            &["streamflix"],
            &original("Zahlfix", "Other", "")
        ));
        assert!(!books(
            &["zahlfix"],
            &["streamflix"],
            &original("Other", "Streamflix", "")
        ));
    }

    #[test]
    fn patterns_respect_unicode_letters() {
        let counterparty = |text| original(text, "", "");
        assert!(books(&["müller"], &[], &counterparty("Bäckerei MÜLLER")));
        assert!(books(&["café"], &[], &counterparty("CAFÉ 0815")));
        for miss in ["Müllerstraße", "Bäckereimüller", "MÜLLERS"] {
            assert!(!books(&["müller"], &[], &counterparty(miss)), "{miss}");
        }
        // A non-ASCII letter is a letter, not a boundary.
        assert!(!books(&["ller"], &[], &counterparty("Müller")));
        assert!(!books(&["ckerei"], &[], &counterparty("Bäckerei")));
        // The words of a text match it, also where a letter's lowercase is
        // longer than the letter, as with "İ".
        for text in ["İSTANBUL GRILL", "ΟΔΟΣ 7", "Straße", "Ǆemal"] {
            let words = crate::learn::words(text);
            let words: Vec<&str> = words.iter().map(String::as_str).collect();
            assert!(books(&words, &[], &counterparty(text)), "{text}");
        }
    }

    #[test]
    fn parse_blocks_rejects_other_keys() {
        let block = |rule: &str| format!("[[rule]]\n{rule}\n");
        let editor =
            block("match = { counterparty = 'x', purpose = 'y' }\naccount = \"Expenses:A\"");
        assert_eq!(
            EditorRule::parse_blocks(&editor),
            Some(vec![EditorRule {
                counterparty: Some("x".into()),
                purpose: Some("y".into()),
                account: "Expenses:A".into(),
            }])
        );
        for other in [
            block("match = { purpose = 'x' }\npayee = \"P\"\naccount = \"Expenses:A\""),
            block("match = { purpose = 'x' }\naccount = \"Expenses:A\"\nadd_tags = [\"t\"]"),
            block("match = { source = \"giro\", purpose = 'x' }\naccount = \"Expenses:A\""),
            block("match = { type = 'x', purpose = 'x' }\naccount = \"Expenses:A\""),
            block("match = { tags = [\"t\"], purpose = 'x' }\naccount = \"Expenses:A\""),
            block("match = { purpose = 'x' }"),
            block("match = {}\naccount = \"Expenses:A\""),
            block("account = \"Expenses:A\""),
            format!("x = 1\n{editor}"),
            "# only a comment\n".to_owned(),
            "rule = []\n".to_owned(),
            String::new(),
            "[[rule]\n".to_owned(),
        ] {
            assert_eq!(EditorRule::parse_blocks(&other), None, "{other}");
        }
    }

    /// The block of an editor rule for the purpose word `streamflix`.
    const BLOCK: &str = "[[rule]]\nmatch = { purpose = '(?i)(?:^|\\PL)streamflix(?:\\PL|$)' }\naccount = \"Expenses:Subscriptions\"\n";
    const GROCERIES: &str = "[[rule]]\nmatch = { counterparty = \"(?i)grocery\" }\naccount = \"Expenses:Food:Groceries\"\n";

    /// Checks that `after` parses with the block's rule first, followed by
    /// `rules` more.
    fn inserted_first(after: &str, rules: usize) {
        let parsed = Rules::parse(after).unwrap();
        assert_eq!(parsed.len(), rules + 1, "{after}");
        assert_eq!(
            parsed.accounts().next(),
            Some((1, "Expenses:Subscriptions")),
            "{after}"
        );
    }

    #[test]
    fn insert_goes_before_the_first_rule_and_its_comments() {
        let text = format!("# header\n\n# Groceries\n# and more\n{GROCERIES}\n# Other\n{RULES}");
        let after = insert_rule(&text, BLOCK);
        assert_eq!(
            after,
            format!("# header\n\n{BLOCK}\n# Groceries\n# and more\n{GROCERIES}\n# Other\n{RULES}")
        );
        inserted_first(&after, 4);
        // Whitespace inside the brackets, indentation and a comment on the
        // header line; a rule header in a comment is no rule.
        let text = format!(
            "# [[rule]]\n\n  # Groceries\n  [[ rule ]] # first\n{}",
            &GROCERIES[9..]
        );
        let after = insert_rule(&text, BLOCK);
        assert_eq!(
            after,
            format!(
                "# [[rule]]\n\n{BLOCK}\n  # Groceries\n  [[ rule ]] # first\n{}",
                &GROCERIES[9..]
            )
        );
        inserted_first(&after, 1);
    }

    #[test]
    fn insert_after_a_header_that_runs_into_the_rule() {
        let text = format!("# header\n# Groceries\n{GROCERIES}");
        let after = insert_rule(&text, BLOCK);
        assert_eq!(
            after,
            format!("# header\n# Groceries\n{BLOCK}\n{GROCERIES}")
        );
        inserted_first(&after, 1);
        assert_eq!(
            insert_rule(GROCERIES, BLOCK),
            format!("{BLOCK}\n{GROCERIES}")
        );
        assert_eq!(
            insert_rule(&format!("\n# Groceries\n{GROCERIES}"), BLOCK),
            format!("\n{BLOCK}\n# Groceries\n{GROCERIES}")
        );
    }

    #[test]
    fn insert_appends_to_a_file_without_rules() {
        let init = crate::layout::init_files(&crate::layout::InitOptions {
            title: "Ledger".into(),
            currency: "EUR".into(),
        });
        let text = init.text(RULES_FILE).unwrap();
        let after = insert_rule(text, BLOCK);
        assert_eq!(after, format!("{text}\n{BLOCK}"));
        inserted_first(&after, 0);
        assert_eq!(
            insert_rule("# no rules yet", BLOCK),
            format!("# no rules yet\n\n{BLOCK}")
        );
        assert_eq!(insert_rule("", BLOCK), BLOCK);
    }

    #[test]
    fn insert_keeps_crlf_and_missing_newline() {
        let crlf = format!(
            "# header\r\n\r\n# Groceries\r\n{}",
            GROCERIES.replace('\n', "\r\n")
        );
        let after = insert_rule(&crlf, BLOCK);
        assert_eq!(
            after,
            format!(
                "# header\r\n\r\n{BLOCK}\n# Groceries\r\n{}",
                GROCERIES.replace('\n', "\r\n")
            )
        );
        inserted_first(&after, 1);
        let unterminated = GROCERIES.trim_end();
        let after = insert_rule(unterminated, BLOCK);
        assert_eq!(after, format!("{BLOCK}\n{unterminated}"));
        inserted_first(&after, 1);
        let comment = "# header\r\n# no rules yet";
        assert_eq!(insert_rule(comment, BLOCK), format!("{comment}\n\n{BLOCK}"));
    }

    #[test]
    fn insert_goes_after_a_byte_order_mark() {
        const BOM: &str = "\u{feff}";
        let after = insert_rule(&format!("{BOM}{GROCERIES}"), BLOCK);
        assert_eq!(after, format!("{BOM}{BLOCK}\n{GROCERIES}"));
        inserted_first(&after, 1);
        // Comment lines right after the mark reach the start of the file.
        let text = format!("{BOM}# header\n# Groceries\n{GROCERIES}");
        let after = insert_rule(&text, BLOCK);
        assert_eq!(
            after,
            format!("{BOM}# header\n# Groceries\n{BLOCK}\n{GROCERIES}")
        );
        inserted_first(&after, 1);
        let text = format!("{BOM}# header\n\n# Groceries\n{GROCERIES}");
        let after = insert_rule(&text, BLOCK);
        assert_eq!(
            after,
            format!("{BOM}# header\n\n{BLOCK}\n# Groceries\n{GROCERIES}")
        );
        inserted_first(&after, 1);
        assert_eq!(insert_rule(BOM, BLOCK), format!("{BOM}{BLOCK}"));
        assert_eq!(
            insert_rule(&format!("{BOM}# no rules yet\n"), BLOCK),
            format!("{BOM}# no rules yet\n\n{BLOCK}")
        );
    }
}
