//! The FFI surface as the app drives it, against a local bare remote: clone,
//! read offline with pending intents, sync, and see conflicts.

// clippy.toml allows unwrap only inside `#[test]` functions; the helpers here
// are test code too.
#![allow(clippy::unwrap_used)]

use std::path::PathBuf;
use std::sync::Arc;

use lentil_core::snapshot::Snapshot;
use lentil_core::sync::git::Git2Backend;
use lentil_core::sync::{Author, Credentials, GitBackend, PushOutcome};
use lentil_ffi::{
    FfiAuthor, FfiCredentials, FfiIdentity, FfiIntent, FfiOutcome, FfiPendingIntent, LentilError,
    LentilRepo, new_device_id,
};
use tempfile::TempDir;

const LENTIL_TOML: &str = r#"spec_version = 0
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

const MAIN: &str = r#"option "title" "Ledger"
option "operating_currency" "EUR"

include "fava.beancount"
include "accounts.beancount"
include "imports/*/*/*.beancount"
include "manual/*/*.beancount"
include "prices/*/*.beancount"
"#;

const ACCOUNTS: &str = "2026-01-01 open Assets:Bank:Giro EUR
2026-01-01 open Equity:OpeningBalances
2026-01-01 open Expenses:Uncategorized
2026-01-01 open Expenses:Unattributed
2026-01-01 open Expenses:Food:Groceries
";

const MAPPING: &str = r#"[file]
encoding = "utf-8"
delimiter = ";"
header = ["Booking date", "Amount"]

[numbers]
decimal = ","

[columns]
booking_date = { column = "Booking date", format = "%d.%m.%Y" }
amount = { column = "Amount" }
counterparty = ["Counterparty"]
purpose = ["Purpose"]

[balance]
from = "none"
"#;

const RUN: &str = r#"; lentil import run
; source: giro, started: 2026-10-20T07:15:02Z, client: lentil-cli/0.1.0

2026-10-12 ! "Corner Grocery" "Card payment"
  lentil_id: "giro-000000000001"
  lentil_desc: "CORNER GROCERY CARD PAYMENT"
  Assets:Bank:Giro  -42.17 EUR
  Expenses:Uncategorized

2026-10-14 ! "Book Shop" "Card payment"
  lentil_id: "giro-000000000002"
  lentil_desc: "BOOK SHOP CARD PAYMENT"
  Assets:Bank:Giro  -12.90 EUR
  Expenses:Uncategorized

2026-10-20 balance Assets:Bank:Giro  -55.07 ~ 0.00 EUR
"#;

const PLACEHOLDER: &str = "; Keeps the include pattern matching.\n";

fn fixture() -> Snapshot {
    Snapshot::from_files([
        ("lentil.toml", LENTIL_TOML),
        (".gitignore", ".*.cache\n.*.picklecache\n"),
        (
            "README.md",
            "# Ledger\n\nThis repository follows the Lentil Convention Spec.\n",
        ),
        ("main.beancount", MAIN),
        (
            "fava.beancount",
            "2020-01-01 custom \"fava-option\" \"language\" \"en\"\n",
        ),
        ("accounts.beancount", ACCOUNTS),
        ("rules.toml", "# Suggestion rules (spec section 16).\n"),
        ("sources/giro.toml", MAPPING),
        ("imports/keep/keep/keep.beancount", PLACEHOLDER),
        ("manual/keep/keep.beancount", PLACEHOLDER),
        ("prices/keep/keep.beancount", PLACEHOLDER),
        ("imports/giro/2026/20261020T071502Z.beancount", RUN),
    ])
}

/// A bare remote whose `main` holds the fixture in one admin commit.
struct Remote {
    dir: TempDir,
}

impl Remote {
    fn new() -> Self {
        let dir = TempDir::new().unwrap();
        let mut options = git2::RepositoryInitOptions::new();
        options.bare(true).initial_head("main");
        git2::Repository::init_opts(dir.path().join("remote.git"), &options).unwrap();
        let remote = Self { dir };
        let seed = TempDir::new().unwrap();
        let mut git = Git2Backend::clone_bare(
            &remote.url(),
            &seed.path().join("seed"),
            Credentials::Default,
        )
        .unwrap();
        let author = Author {
            name: "Admin".to_owned(),
            email: "admin@example.org".to_owned(),
        };
        let commit = git
            .commit(None, &Snapshot::new(), &fixture(), "admin: init\n", &author)
            .unwrap();
        assert_eq!(git.push(&commit).unwrap(), PushOutcome::Pushed);
        remote
    }

    fn url(&self) -> String {
        self.dir.path().join("remote.git").display().to_string()
    }

    /// A new app clone in its own directory.
    fn clone_app(&self, dir: &TempDir) -> Arc<LentilRepo> {
        LentilRepo::clone_repo(&self.url(), app_path(dir), FfiCredentials::Default).unwrap()
    }
}

fn app_path(dir: &TempDir) -> String {
    let path: PathBuf = dir.path().join("ledger.git");
    path.display().to_string()
}

fn pending(key: &str, intent: FfiIntent) -> FfiPendingIntent {
    FfiPendingIntent {
        key: key.to_owned(),
        intent,
    }
}

fn identity(device: &str) -> FfiIdentity {
    FfiIdentity {
        client: "lentil-android/0.1.0".to_owned(),
        device: device.to_owned(),
    }
}

fn author() -> FfiAuthor {
    FfiAuthor {
        name: "Alex Example".to_owned(),
        email: "alex@example.org".to_owned(),
    }
}

fn queue_ids(repo: &LentilRepo, intents: Vec<FfiPendingIntent>) -> Vec<String> {
    repo.queue(intents)
        .unwrap()
        .into_iter()
        .filter_map(|item| item.id)
        .collect()
}

#[test]
fn a_fresh_clone_reads_offline() {
    let remote = Remote::new();
    let dir = TempDir::new().unwrap();
    let repo = remote.clone_app(&dir);

    assert_eq!(repo.read_only_reasons().unwrap(), vec![]);
    let queue = repo.queue(vec![]).unwrap();
    assert_eq!(queue.len(), 2);
    assert_eq!(queue[0].id.as_deref(), Some("giro-000000000002"));
    assert_eq!(queue[0].amount.as_deref(), Some("-12.90"));
    assert_eq!(queue[0].source_account.as_deref(), Some("Assets:Bank:Giro"));
    assert_eq!(
        queue[0].counter_account.as_deref(),
        Some("Expenses:Uncategorized")
    );
    assert_eq!(queue[0].counter_posting, Some(1));
    assert_eq!(
        repo.default_open_date(vec![]).unwrap().as_deref(),
        Some("2026-10-12")
    );
    assert!(
        repo.accounts(vec![])
            .unwrap()
            .contains(&"Expenses:Food:Groceries".to_owned())
    );

    // Reopening finds the same clone.
    let reopened = LentilRepo::open(app_path(&dir)).unwrap();
    assert_eq!(reopened.queue(vec![]).unwrap(), queue);
}

#[test]
fn pending_intents_show_before_the_sync() {
    let remote = Remote::new();
    let dir = TempDir::new().unwrap();
    let repo = remote.clone_app(&dir);
    let intents = vec![
        pending(
            "k1",
            FfiIntent::Confirm {
                id: "giro-000000000002".to_owned(),
            },
        ),
        pending(
            "k2",
            FfiIntent::OpenAccount {
                account: "Expenses:Books".to_owned(),
                date: "2026-10-12".to_owned(),
            },
        ),
        pending(
            "k3",
            FfiIntent::SetAccount {
                id: "giro-000000000001".to_owned(),
                posting: 1,
                old: "Expenses:Uncategorized".to_owned(),
                new: "Expenses:Books".to_owned(),
            },
        ),
    ];

    assert_eq!(queue_ids(&repo, intents.clone()), vec!["giro-000000000001"]);
    let queue = repo.queue(intents.clone()).unwrap();
    assert_eq!(queue[0].counter_account.as_deref(), Some("Expenses:Books"));
    assert!(
        repo.accounts(intents)
            .unwrap()
            .contains(&"Expenses:Books".to_owned())
    );
    // Without the intents, the local state is unchanged.
    assert_eq!(queue_ids(&repo, vec![]).len(), 2);
}

#[test]
fn sync_pushes_intents_and_reports_conflicts() {
    let remote = Remote::new();
    let phone = TempDir::new().unwrap();
    let tablet = TempDir::new().unwrap();
    let first = remote.clone_app(&phone);
    let second = remote.clone_app(&tablet);

    let report = first
        .sync(
            vec![
                pending(
                    "k1",
                    FfiIntent::SetAccount {
                        id: "giro-000000000001".to_owned(),
                        posting: 1,
                        old: "Expenses:Uncategorized".to_owned(),
                        new: "Expenses:Food:Groceries".to_owned(),
                    },
                ),
                pending(
                    "k2",
                    FfiIntent::Confirm {
                        id: "giro-000000000001".to_owned(),
                    },
                ),
            ],
            identity("aaaaaaaa"),
            author(),
            FfiCredentials::Default,
        )
        .unwrap();
    assert!(report.committed);
    assert_eq!(
        report
            .outcomes
            .iter()
            .map(|o| (o.key.as_str(), &o.outcome))
            .collect::<Vec<_>>(),
        vec![("k1", &FfiOutcome::Applied), ("k2", &FfiOutcome::Applied)]
    );
    assert_eq!(queue_ids(&first, vec![]), vec!["giro-000000000002"]);

    // The other device saw the old account and loses the race.
    let report = second
        .sync(
            vec![pending(
                "t1",
                FfiIntent::SetAccount {
                    id: "giro-000000000001".to_owned(),
                    posting: 1,
                    old: "Expenses:Uncategorized".to_owned(),
                    new: "Expenses:Unattributed".to_owned(),
                },
            )],
            identity("bbbbbbbb"),
            author(),
            FfiCredentials::Default,
        )
        .unwrap();
    assert!(!report.committed);
    assert_eq!(
        report.outcomes[0].outcome,
        FfiOutcome::Conflict {
            code: "AccountChanged".to_owned(),
            current: Some("Expenses:Food:Groceries".to_owned()),
        }
    );
    // The sync still brought the other device's change.
    assert_eq!(queue_ids(&second, vec![]), vec!["giro-000000000002"]);
}

#[test]
fn opening_a_missing_clone_fails() {
    let dir = TempDir::new().unwrap();

    assert!(LentilRepo::open(app_path(&dir)).is_err());
    assert!(matches!(
        LentilRepo::clone_repo(
            &dir.path().join("missing.git").display().to_string(),
            app_path(&dir),
            FfiCredentials::Default,
        ),
        Err(LentilError::NotFound | LentilError::Other { .. } | LentilError::Network)
    ));
}

#[test]
fn device_ids_are_eight_base32_characters() {
    let id = new_device_id();

    assert_eq!(id.len(), 8);
    assert!(
        id.chars()
            .all(|c| c.is_ascii_lowercase() || ('2'..='7').contains(&c)),
        "{id}"
    );
}
