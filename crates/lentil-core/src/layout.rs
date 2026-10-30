//! Repository layout (spec section 3) and the files `init` creates (PRD FR-A1).

use crate::config::CONFIG_FILE;
use crate::snapshot::Snapshot;

/// The include patterns `main.beancount` holds, in order (spec section 3).
pub const MAIN_INCLUDES: [&str; 5] = [
    "fava.beancount",
    "accounts.beancount",
    "imports/*/*/*.beancount",
    "manual/*/*.beancount",
    "prices/*/*.beancount",
];

/// Placeholder files that keep each glob include matching in a repository
/// without imports, manual entries or prices. Python Beancount and rustledger
/// both report a glob without matches as an error.
pub const PLACEHOLDERS: [&str; 3] = [
    "imports/keep/keep/keep.beancount",
    "manual/keep/keep.beancount",
    "prices/keep/keep.beancount",
];

/// The default main file (spec section 3).
pub const MAIN_FILE: &str = "main.beancount";
/// Fava options and sidebar links (spec section 3).
pub const FAVA_FILE: &str = "fava.beancount";
/// `open`, `close` and `commodity` directives (spec section 3).
pub const ACCOUNTS_FILE: &str = "accounts.beancount";
/// Suggestion rules (spec section 16).
pub const RULES_FILE: &str = "rules.toml";
/// Keeps load caches out of Git (spec section 3).
pub const GITIGNORE_FILE: &str = ".gitignore";
/// Explains the convention to users of the ledger repository.
pub const README_FILE: &str = "README.md";

/// Fallback account written by `init` (spec section 4).
pub const DEFAULT_FALLBACK_ACCOUNT: &str = "Expenses:Uncategorized";
/// Unattributed account written by `init` (spec section 4).
pub const DEFAULT_UNATTRIBUTED_ACCOUNT: &str = "Expenses:Unattributed";
/// Counter account of the first balance snapshot of an account (spec section 9).
pub const OPENING_BALANCES_ACCOUNT: &str = "Equity:Opening-Balances";

/// Date of the directives `init` writes: early enough for any real entry.
const INIT_DATE: &str = "2000-01-01";

/// Options for a new repository.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InitOptions {
    /// `option "title"`.
    pub title: String,
    /// `option "operating_currency"`.
    pub currency: String,
}

/// The files of a new repository: `lentil.toml`, `main.beancount`,
/// `fava.beancount`, `accounts.beancount`, `rules.toml`, `.gitignore`,
/// `README.md` and the placeholders.
#[must_use]
pub fn init_files(options: &InitOptions) -> Snapshot {
    let mut files = Snapshot::new();
    files.insert(CONFIG_FILE, config_file(&options.currency));
    files.insert(MAIN_FILE, main_file(&options.title, &options.currency));
    files.insert(FAVA_FILE, fava_file(&options.title));
    files.insert(ACCOUNTS_FILE, accounts_file());
    files.insert(RULES_FILE, RULES);
    files.insert(GITIGNORE_FILE, GITIGNORE);
    files.insert(README_FILE, readme(&options.title, &options.currency));
    for (placeholder, pattern) in PLACEHOLDERS.iter().zip(&MAIN_INCLUDES[2..]) {
        files.insert(*placeholder, placeholder_file(pattern));
    }
    files
}

/// The exact `main.beancount` of spec section 3 for a title and currency.
#[must_use]
pub fn main_file(title: &str, currency: &str) -> String {
    let mut text = format!(
        "option \"title\" {}\noption \"operating_currency\" {}\n\n",
        quote(title),
        quote(currency)
    );
    for include in MAIN_INCLUDES {
        text.push_str(&format!("include \"{include}\"\n"));
    }
    text
}

/// The URL slug Fava derives from a ledger title, as Fava's `slugify`:
/// drop everything but word characters, whitespace and hyphens, trim,
/// lowercase, and collapse runs of hyphens and whitespace into one hyphen.
#[must_use]
pub fn fava_slug(title: &str) -> String {
    let kept: String = title
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == '_' || *c == '-' || c.is_whitespace())
        .collect();
    let lower = kept.trim().to_lowercase();
    let mut slug = String::with_capacity(lower.len());
    let mut in_run = false;
    for c in lower.chars() {
        if c == '-' || c.is_whitespace() {
            if !in_run {
                slug.push('-');
            }
            in_run = true;
        } else {
            slug.push(c);
            in_run = false;
        }
    }
    slug
}

/// A Beancount string literal.
fn quote(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for c in value.chars() {
        if c == '"' || c == '\\' {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('"');
    out
}

fn config_file(currency: &str) -> String {
    format!(
        r#"# Lentil configuration (Lentil Convention Spec, section 4). Never put
# credentials, PINs or tokens into this file or anywhere in the repository.

spec_version = 0
main_file = "main.beancount"
fallback_account = "{DEFAULT_FALLBACK_ACCOUNT}"
unattributed_account = "{DEFAULT_UNATTRIBUTED_ACCOUNT}"
price_currencies = []

# One [[source]] per imported bank account or card, for example:
#
# [[source]]
# id = "giro"                       # [a-z0-9-], part of every imported ID; never change it
# account = "Assets:Bank:Giro"      # must be opened in accounts.beancount
# currency = "{currency}"
# importer = "csv"
# mapping = "sources/giro.toml"     # CSV mapping (spec section 15)
"#
    )
}

fn fava_file(title: &str) -> String {
    let slug = fava_slug(title);
    format!(
        "; Fava options and sidebar links, written by lentil init. This file holds\n\
         ; only custom \"fava-option\" and custom \"fava-sidebar-link\" directives.\n\
         \n\
         {INIT_DATE} custom \"fava-sidebar-link\" \"Open transactions\" \"/{slug}/journal/?filter=flag%3A%22%21%22\"\n\
         {INIT_DATE} custom \"fava-sidebar-link\" \"Uncategorized\" \"/{slug}/account/{DEFAULT_FALLBACK_ACCOUNT}/\"\n"
    )
}

fn accounts_file() -> String {
    format!(
        "; Accounts: open, close and commodity directives. Editors append new open\n\
         ; directives at the end of this file.\n\
         \n\
         {INIT_DATE} open {OPENING_BALANCES_ACCOUNT}\n\
         {INIT_DATE} open {DEFAULT_FALLBACK_ACCOUNT}\n\
         {INIT_DATE} open {DEFAULT_UNATTRIBUTED_ACCOUNT}\n"
    )
}

fn placeholder_file(pattern: &str) -> String {
    format!(
        "; Placeholder written by lentil init. It keeps the include pattern\n\
         ; \"{pattern}\" matching while no other file does.\n\
         ; Leave it in place and empty.\n"
    )
}

const RULES: &str = r#"# Suggestion rules (Lentil Convention Spec, section 16). Rules are evaluated
# in file order and the first match wins. Without a match, the importer
# learns from reviewed transactions, and only then uses fallback_account.
# Regular expressions use RE2 syntax without lookaround.
#
# [[rule]]
# source = "giro"                        # optional: limit the rule to one source
# desc = "(?i)corner grocery"            # regular expression on lentil_desc
# account = "Expenses:Food:Groceries"
"#;

const GITIGNORE: &str = "# Load caches of rustledger and Python Beancount never belong in Git.\n\
                         .*.cache\n\
                         .*.picklecache\n";

fn readme(title: &str, currency: &str) -> String {
    format!(
        r#"# {title}

This repository is a [Beancount](https://github.com/beancount/beancount) ledger kept with Lentil. Every file is plain Beancount or TOML, so any Beancount tool can read it, and several Lentil clients can write to it at the same time because each of them follows the Lentil Convention Spec.

## Layout

Every file has exactly one kind of writer, and fixed include patterns in `main.beancount` pick up new files.

| Path | Content | Written by |
| --- | --- | --- |
| `lentil.toml` | Configuration: sources and default accounts | Admin |
| `main.beancount` | Entry point: two options and five includes; never change the includes | Admin |
| `fava.beancount` | Fava options and sidebar links | Admin |
| `accounts.beancount` | `open`, `close` and `commodity` directives | Admin; editors append `open` directives |
| `rules.toml` | Suggestion rules for imports | Admin |
| `sources/<source-id>.toml` | CSV mapping of one source | Admin |
| `imports/<source-id>/<YYYY>/<YYYYMMDDTHHMMSSZ>.beancount` | One import run | Importer |
| `manual/<device-id>/<YYYY-MM>.beancount` | Manual entries of one device | Editor |
| `prices/<YYYY>/<YYYYMMDD>.beancount` | Exchange rates | Importer, admin |

- Directory and file names use lowercase letters, digits and hyphens, apart from the run timestamps and the fixed names above.
- `.beancount` files anywhere else are not part of the ledger and are rejected.
- The `keep` files keep the include patterns matching while a directory has no other file. Leave them in place and empty.
- Never commit the load caches `.*.cache` and `.*.picklecache`; `.gitignore` keeps them out.

## Review state

Review state lives only in the transaction flag.

- `!` means open: importers write every transaction with `!`.
- `*` means reviewed: confirming or correcting a transaction sets `*`, and manual entries start as `*`.
- A duplicate is dismissed by commenting it out in place. Its `lentil_id` still counts, so it is never imported again.

## Roles

Every change belongs to one role, and validation rejects changes outside that role's area.

- **Importer** (`lentil import`): only creates new run files under `imports/<source-id>/` and never changes existing files.
- **Editor** (the Android app): changes the flag, payee, narration, tags, links, metadata and the counter account of existing transactions, comments out duplicates, appends `open` directives to `accounts.beancount` and writes only its own `manual/<device-id>/` files.
- **Admin** (`lentil init`, a text editor followed by `lentil check`): may change anything, as long as the result still follows the convention.

## Protected metadata

Lentil addresses transactions by their metadata, so never change or remove these keys:

- `lentil_id`: the stable ID of a transaction.
- `lentil_ref`: the bank's own reference.
- `lentil_desc`: the bank's original description, from which suggestions learn.

Rename payees and narrations freely instead; `lentil_desc` keeps the bank's wording.

## Adding a source

A source is one bank account or card whose CSV exports you import.

1. Open its account in `accounts.beancount`, for example `2026-01-01 open Assets:Bank:Giro {currency}`.
2. Add it to `lentil.toml`:

   ```toml
   [[source]]
   id = "giro"
   account = "Assets:Bank:Giro"
   currency = "{currency}"
   importer = "csv"
   mapping = "sources/giro.toml"
   ```

3. Describe the bank's CSV export in `sources/giro.toml`. Columns are found by their header names:

   ```toml
   [file]
   encoding = "utf-8"
   delimiter = ";"
   header = ["Booking date", "Amount"]

   [numbers]
   decimal = ","
   thousands = "."

   [columns]
   booking_date = {{ column = "Booking date", format = "%d.%m.%Y" }}
   amount = {{ column = "Amount" }}
   counterparty = ["Payee"]
   purpose = ["Purpose"]

   [[skip]]
   column = "Status"
   equals = "Pending"

   [balance]
   from = "column"
   column = "Balance"
   ```

4. Run `lentil import` with the source ID and the export file. Every new transaction arrives with flag `!` and a suggested counter account; transactions imported before are skipped. If the export holds no balance, `lentil import` asks for the balance shown in online banking, and `lentil status` reports the run as unchecked if you skip it.

## Viewing and checking

- `fava main.beancount` shows the ledger. The sidebar links to the open transactions and to `{DEFAULT_FALLBACK_ACCOUNT}`.
- `lentil status` lists open transactions, failing balance assertions and import runs without a balance check.
- `lentil check` validates the repository after a change in a text editor; `bean-check main.beancount` runs the reference implementation.
"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::ledger;

    fn options() -> InitOptions {
        InitOptions {
            title: "Ledger".into(),
            currency: "EUR".into(),
        }
    }

    #[test]
    fn main_file_is_exactly_the_spec_example() {
        assert_eq!(
            main_file("Ledger", "EUR"),
            r#"option "title" "Ledger"
option "operating_currency" "EUR"

include "fava.beancount"
include "accounts.beancount"
include "imports/*/*/*.beancount"
include "manual/*/*.beancount"
include "prices/*/*.beancount"
"#
        );
    }

    #[test]
    fn main_file_escapes_the_title() {
        let text = main_file(r#"My "big" \ ledger"#, "EUR");
        assert!(text.starts_with(r#"option "title" "My \"big\" \\ ledger""#));
        let mut files = init_files(&options());
        files.insert(MAIN_FILE, text);
        let ledger = ledger::load(&files, MAIN_FILE);
        assert!(ledger.errors.is_empty(), "{:?}", ledger.errors);
    }

    #[test]
    fn init_files_lists_every_file() {
        let files = init_files(&options());
        let mut expected = vec![
            ".gitignore",
            "README.md",
            "accounts.beancount",
            "fava.beancount",
            "imports/keep/keep/keep.beancount",
            "lentil.toml",
            "main.beancount",
            "manual/keep/keep.beancount",
            "prices/keep/keep.beancount",
            "rules.toml",
        ];
        expected.sort_unstable();
        assert_eq!(files.paths().collect::<Vec<_>>(), expected);
    }

    #[test]
    fn init_config_parses_with_spec_defaults() {
        let files = init_files(&options());
        let config = Config::parse(files.text(CONFIG_FILE).unwrap()).unwrap();
        assert_eq!(config.spec_version, 0);
        assert_eq!(config.main_file, "main.beancount");
        assert_eq!(config.fallback_account, "Expenses:Uncategorized");
        assert_eq!(config.unattributed_account, "Expenses:Unattributed");
        assert!(config.price_currencies.is_empty());
        assert!(config.sources.is_empty());
        assert!(config.snapshots.is_empty());
    }

    #[test]
    fn init_config_example_source_parses_when_uncommented() {
        let files = init_files(&options());
        let text = files.text(CONFIG_FILE).unwrap();
        let start = text.find("# [[source]]").unwrap();
        let uncommented: String = text[start..]
            .lines()
            .map(|l| l.strip_prefix("# ").unwrap_or(l))
            .collect::<Vec<_>>()
            .join("\n");
        let config = Config::parse(&format!("{}{uncommented}\n", &text[..start])).unwrap();
        assert_eq!(config.sources[0].id, "giro");
    }

    #[test]
    fn init_ledger_loads_without_errors() {
        let files = init_files(&options());
        let ledger = ledger::load(&files, "main.beancount");
        assert!(ledger.errors.is_empty(), "{:?}", ledger.errors);
        assert!(ledger.transactions.is_empty());
        let mut expected = vec!["main.beancount", "fava.beancount", "accounts.beancount"];
        expected.extend(PLACEHOLDERS);
        assert_eq!(ledger.files, expected);
        for account in [
            "Expenses:Uncategorized",
            "Expenses:Unattributed",
            "Equity:Opening-Balances",
        ] {
            assert!(ledger.accounts.contains_key(account), "{account}");
        }
    }

    #[test]
    fn placeholders_hold_only_comments() {
        let files = init_files(&options());
        for placeholder in PLACEHOLDERS {
            let text = files.text(placeholder).unwrap();
            assert!(text.lines().all(|l| l.starts_with(';')), "{placeholder}");
        }
    }

    #[test]
    fn fava_file_holds_only_fava_custom_directives() {
        let files = init_files(&options());
        let text = files.text(FAVA_FILE).unwrap();
        let directives: Vec<_> = text
            .lines()
            .filter(|l| !l.is_empty() && !l.starts_with(';'))
            .collect();
        assert_eq!(directives.len(), 2);
        for line in &directives {
            assert!(
                line.starts_with("2000-01-01 custom \"fava-sidebar-link\" ")
                    || line.starts_with("2000-01-01 custom \"fava-option\" "),
                "{line}"
            );
        }
        assert!(text.contains("\"/ledger/journal/?filter=flag%3A%22%21%22\""));
        assert!(text.contains("\"/ledger/account/Expenses:Uncategorized/\""));
    }

    #[test]
    fn fava_slug_follows_favas_slugify() {
        assert_eq!(fava_slug("Ledger"), "ledger");
        assert_eq!(fava_slug("My Household Ledger"), "my-household-ledger");
        assert_eq!(fava_slug("  Kasse & Bank -- 2026 "), "kasse-bank-2026");
        assert_eq!(fava_slug("Müller_Finanzen"), "müller_finanzen");
    }

    #[test]
    fn gitignore_excludes_load_caches() {
        let files = init_files(&options());
        let lines: Vec<_> = files.text(GITIGNORE_FILE).unwrap().lines().collect();
        assert!(lines.contains(&".*.cache"));
        assert!(lines.contains(&".*.picklecache"));
    }

    #[test]
    fn rules_file_is_only_a_commented_example() {
        let files = init_files(&options());
        let text = files.text(RULES_FILE).unwrap();
        assert!(text.lines().all(|l| l.starts_with('#')));
        assert!(text.contains("# [[rule]]"));
        let table: toml::Table = toml::from_str(text).unwrap();
        assert!(table.is_empty());
    }

    #[test]
    fn readme_explains_the_convention() {
        let files = init_files(&options());
        let text = files.text(README_FILE).unwrap();
        for needle in [
            "# Ledger",
            "`!`",
            "`*`",
            "lentil_id",
            "lentil_ref",
            "lentil_desc",
            "Importer",
            "Editor",
            "Admin",
            "[[source]]",
            "lentil import",
            "fava main.beancount",
        ] {
            assert!(text.contains(needle), "{needle}");
        }
    }
}
