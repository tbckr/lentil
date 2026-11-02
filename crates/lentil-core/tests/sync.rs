//! Synchronization against real Git repositories (spec sections 11 and 12).
//!
//! Every remote is a local bare repository. The backend tests drive
//! [`Git2Backend`] directly; the multi-writer tests let several clients with
//! pending intents push to one remote, interleaved so that pushes are
//! rejected and retried, and fail on lost intents, duplicate transactions or
//! non-linear history (CONTRIBUTING: Testing).

// clippy.toml allows unwrap only inside `#[test]` functions; the helpers here
// are test code too.
#![allow(clippy::unwrap_used)]

use std::collections::{BTreeSet, VecDeque};
use std::path::{Path, PathBuf};
use std::time::Duration;

use git2::Repository;
use lentil_core::finding::{FindingCode, Severity};
use lentil_core::intent::{Conflict, ConflictCode, Intent, Outcome, PendingIntent};
use lentil_core::ledger::{self, Ledger};
use lentil_core::snapshot::Snapshot;
use lentil_core::sync::git::Git2Backend;
use lentil_core::sync::{
    Author, CommitId, Credentials, EditorSync, GitBackend, GitError, Identity, MAX_ATTEMPTS,
    PushOutcome, Sleeper, SyncError, Work, run_sync, sync_intents,
};
use lentil_core::validate::Role;
use tempfile::TempDir;

// ---------------------------------------------------------------------------
// Fixture: a small repository that follows the spec (sections 3, 4, 8, 15).
// ---------------------------------------------------------------------------

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

const FAVA: &str = "2020-01-01 custom \"fava-option\" \"language\" \"en\"\n";

const ACCOUNTS: &str = "2020-01-01 open Assets:Bank:Giro EUR
2020-01-01 open Equity:OpeningBalances
2020-01-01 open Expenses:Uncategorized
2020-01-01 open Expenses:Unattributed
2020-01-01 open Expenses:Food:Groceries
2020-01-01 open Expenses:Household
2020-01-01 open Expenses:Utilities
2020-01-01 open Income:Salary
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

const RUN_FILE: &str = "imports/giro/2026/20261020T071502Z.beancount";
const PLACEHOLDER: &str = "; Keeps the include pattern matching.\n";

/// `(id, date, payee, amount)`; every counter posting is on the fallback account.
const TRANSACTIONS: [(&str, &str, &str, &str); 8] = [
    (
        "giro-000000000001",
        "2026-10-12",
        "Corner Grocery",
        "-42.17",
    ),
    (
        "giro-000000000002",
        "2026-10-13",
        "City Utilities",
        "-64.00",
    ),
    ("giro-000000000003", "2026-10-13", "Bakery", "-3.10"),
    ("giro-000000000004", "2026-10-14", "Employer", "2500.00"),
    (
        "giro-000000000005",
        "2026-10-15",
        "Hardware Store",
        "-19.99",
    ),
    ("giro-000000000006", "2026-10-16", "Pharmacy", "-8.45"),
    ("giro-000000000007", "2026-10-17", "Water Works", "-31.20"),
    ("giro-000000000008", "2026-10-18", "Book Shop", "-12.90"),
];
/// The sum of [`TRANSACTIONS`].
const BALANCE: &str = "2318.19";

fn id(n: usize) -> &'static str {
    TRANSACTIONS[n - 1].0
}

fn run_file(balance: &str) -> String {
    let mut text = String::from(
        "; lentil import run\n; source: giro, started: 2026-10-20T07:15:02Z, client: lentil-cli/0.1.0\n",
    );
    for (id, date, payee, amount) in TRANSACTIONS {
        text.push_str(&format!(
            "\n{date} ! \"{payee}\" \"Card payment\"\n  lentil_id: \"{id}\"\n  lentil_desc: \"{} CARD PAYMENT\"\n  Assets:Bank:Giro  {amount} EUR\n  Expenses:Uncategorized\n",
            payee.to_uppercase()
        ));
    }
    text.push_str(&format!(
        "\n2026-10-20 balance Assets:Bank:Giro  {balance} ~ 0.00 EUR\n"
    ));
    text
}

fn fixture_with(balance: &str) -> Snapshot {
    Snapshot::from_files([
        ("lentil.toml", LENTIL_TOML.to_owned()),
        (".gitignore", ".*.cache\n.*.picklecache\n".to_owned()),
        (
            "README.md",
            "# Ledger\n\nThis repository follows the Lentil Convention Spec.\n".to_owned(),
        ),
        ("main.beancount", MAIN.to_owned()),
        ("fava.beancount", FAVA.to_owned()),
        ("accounts.beancount", ACCOUNTS.to_owned()),
        (
            "rules.toml",
            "# Suggestion rules (spec section 16).\n".to_owned(),
        ),
        ("sources/giro.toml", MAPPING.to_owned()),
        ("imports/keep/keep/keep.beancount", PLACEHOLDER.to_owned()),
        ("manual/keep/keep.beancount", PLACEHOLDER.to_owned()),
        ("prices/keep/keep.beancount", PLACEHOLDER.to_owned()),
        (RUN_FILE, run_file(balance)),
    ])
}

fn fixture() -> Snapshot {
    fixture_with(BALANCE)
}

#[test]
fn fixture_loads_without_errors() {
    let ledger = load(&fixture());
    assert!(ledger.errors.is_empty(), "{:?}", ledger.errors);
    assert!(ledger.balance_failures.is_empty());
    assert_eq!(ledger.transactions.len(), TRANSACTIONS.len());
    assert!(ledger.transactions.iter().all(|t| t.flag == '!'));
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// A bare repository that plays the remote.
struct Remote {
    _dir: TempDir,
    path: PathBuf,
}

impl Remote {
    fn empty() -> Self {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("remote.git");
        let mut options = git2::RepositoryInitOptions::new();
        options.bare(true).initial_head("main");
        Repository::init_opts(&path, &options).unwrap();
        Self { _dir: dir, path }
    }

    /// A remote whose `main` holds `files` in one admin commit.
    fn with(files: &Snapshot) -> Self {
        let remote = Self::empty();
        let seed = TempDir::new().unwrap();
        let mut git =
            Git2Backend::clone_bare(&remote.url(), seed.path(), Credentials::Default).unwrap();
        assert_eq!(git.fetch().unwrap(), None);
        let commit = git
            .commit(
                None,
                &Snapshot::new(),
                files,
                "admin: init\n",
                &author("Admin"),
            )
            .unwrap();
        assert_eq!(git.push(&commit).unwrap(), PushOutcome::Pushed);
        remote
    }

    fn url(&self) -> String {
        self.path.to_string_lossy().into_owned()
    }

    fn file_url(&self) -> String {
        format!("file://{}", self.path.display())
    }

    fn head(&self) -> Option<CommitId> {
        let repo = Repository::open(&self.path).unwrap();
        repo.find_reference("refs/heads/main")
            .ok()
            .and_then(|r| r.target())
            .map(|oid| CommitId(oid.to_string()))
    }

    fn snapshot(&self) -> Snapshot {
        // Without an `origin`, the backend reads the repository's own `main`.
        let mut git = Git2Backend::open(&self.path, Credentials::Default).unwrap();
        let head = git.fetch().unwrap();
        git.snapshot(head.as_ref()).unwrap()
    }

    /// The commits of `main`, oldest first. Fails on any merge commit and on
    /// any branch besides `main`.
    fn history(&self) -> Vec<CommitInfo> {
        let repo = Repository::open(&self.path).unwrap();
        let branches: Vec<_> = repo
            .references()
            .unwrap()
            .map(|r| r.unwrap().name().unwrap().to_owned())
            .collect();
        assert_eq!(branches, vec!["refs/heads/main".to_owned()]);
        let mut history = Vec::new();
        let mut next = repo.find_reference("refs/heads/main").unwrap().target();
        while let Some(oid) = next {
            let commit = repo.find_commit(oid).unwrap();
            assert!(commit.parent_count() <= 1, "merge commit {oid}");
            history.push(CommitInfo {
                id: CommitId(oid.to_string()),
                message: commit.message().unwrap().to_owned(),
                author: commit.author().name().unwrap().to_owned(),
                email: commit.author().email().unwrap().to_owned(),
            });
            next = commit.parent_ids().next();
        }
        history.reverse();
        history
    }
}

#[derive(Debug)]
struct CommitInfo {
    id: CommitId,
    message: String,
    author: String,
    email: String,
}

fn author(name: &str) -> Author {
    Author {
        name: name.to_owned(),
        email: format!("{}@example.org", name.to_lowercase()),
    }
}

fn editor(device: &str) -> Identity {
    Identity {
        role: Role::Editor,
        client: "lentil-test/0.1.0".to_owned(),
        device: device.to_owned(),
    }
}

fn admin() -> Identity {
    Identity {
        role: Role::Admin,
        client: "lentil-test/0.1.0".to_owned(),
        device: "adminxyz".to_owned(),
    }
}

fn message(role: &str, subject: &str, device: &str) -> String {
    format!(
        "{role}: {subject}\n\nLentil-Spec: 0\nLentil-Role: {role}\nLentil-Client: lentil-test/0.1.0\nLentil-Device: {device}\n"
    )
}

fn confirm(key: &str, n: usize) -> PendingIntent {
    PendingIntent {
        key: key.to_owned(),
        intent: Intent::Confirm {
            id: id(n).to_owned(),
        },
    }
}

fn set_account(key: &str, n: usize, new: &str) -> PendingIntent {
    PendingIntent {
        key: key.to_owned(),
        intent: Intent::SetAccount {
            id: id(n).to_owned(),
            posting: 1,
            old: "Expenses:Uncategorized".to_owned(),
            new: new.to_owned(),
        },
    }
}

fn load(snapshot: &Snapshot) -> Ledger {
    ledger::load(snapshot, "main.beancount")
}

fn flag(ledger: &Ledger, n: usize) -> char {
    ledger.find(id(n)).unwrap().flag
}

fn counter_account(ledger: &Ledger, n: usize) -> String {
    ledger.find(id(n)).unwrap().postings[1].account.clone()
}

fn outcomes(sync: &EditorSync) -> Vec<(&str, &Outcome)> {
    sync.outcomes.iter().map(|(k, o)| (k.as_str(), o)).collect()
}

/// Records waits instead of sleeping.
#[derive(Debug, Default)]
struct Recorder(Vec<Duration>);

impl Sleeper for Recorder {
    fn sleep(&mut self, duration: Duration) {
        self.0.push(duration);
    }
}

fn secs(list: &[u64]) -> Vec<Duration> {
    list.iter().map(|s| Duration::from_secs(*s)).collect()
}

type Hook<'a> = Box<dyn FnMut() + 'a>;

/// A backend that runs one hook before each push, so another client can
/// push in between this client's fetch and push.
struct Interleaved<'a> {
    inner: Git2Backend,
    before_push: VecDeque<Hook<'a>>,
    pushes: Vec<PushOutcome>,
}

impl<'a> Interleaved<'a> {
    fn new(inner: Git2Backend, hooks: Vec<Hook<'a>>) -> Self {
        Self {
            inner,
            before_push: hooks.into(),
            pushes: Vec::new(),
        }
    }
}

impl GitBackend for Interleaved<'_> {
    fn fetch(&mut self) -> Result<Option<CommitId>, GitError> {
        self.inner.fetch()
    }

    fn snapshot(&self, commit: Option<&CommitId>) -> Result<Snapshot, GitError> {
        self.inner.snapshot(commit)
    }

    fn commit(
        &mut self,
        parent: Option<&CommitId>,
        before: &Snapshot,
        after: &Snapshot,
        message: &str,
        author: &Author,
    ) -> Result<CommitId, GitError> {
        self.inner.commit(parent, before, after, message, author)
    }

    fn push(&mut self, commit: &CommitId) -> Result<PushOutcome, GitError> {
        if let Some(mut hook) = self.before_push.pop_front() {
            hook();
        }
        let outcome = self.inner.push(commit)?;
        self.pushes.push(outcome);
        Ok(outcome)
    }

    fn finish(&mut self, commit: &CommitId) -> Result<(), GitError> {
        self.inner.finish(commit)
    }
}

struct Client {
    dir: TempDir,
    identity: Identity,
    author: Author,
    sleeper: Recorder,
}

impl Client {
    fn new(device: &str, name: &str) -> Self {
        Self {
            dir: TempDir::new().unwrap(),
            identity: editor(device),
            author: author(name),
            sleeper: Recorder::default(),
        }
    }

    fn path(&self) -> PathBuf {
        self.dir.path().join("clone")
    }

    fn clone_from(&self, remote: &Remote) -> Git2Backend {
        Git2Backend::clone_from(&remote.url(), &self.path(), Credentials::Default).unwrap()
    }

    fn sync(
        &mut self,
        git: &mut dyn GitBackend,
        intents: &[PendingIntent],
    ) -> Result<EditorSync, SyncError> {
        sync_intents(
            git,
            &mut self.sleeper,
            &self.identity,
            &self.author,
            intents,
        )
    }

    fn file(&self, path: &str) -> String {
        std::fs::read_to_string(self.path().join(path)).unwrap()
    }
}

fn write(dir: &Path, path: &str, content: &str) {
    let full = dir.join(path);
    std::fs::create_dir_all(full.parent().unwrap()).unwrap();
    std::fs::write(full, content).unwrap();
}

// ---------------------------------------------------------------------------
// Backend
// ---------------------------------------------------------------------------

#[test]
fn clone_snapshot_commit_push_and_finish() {
    let remote = Remote::with(&fixture());
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("clone");
    let mut git = Git2Backend::clone_from(&remote.file_url(), &path, Credentials::Default).unwrap();

    // The clone checked out main.
    assert_eq!(
        std::fs::read_to_string(path.join("main.beancount")).unwrap(),
        MAIN
    );

    let head = git.fetch().unwrap();
    assert_eq!(head, remote.head());
    let before = git.snapshot(head.as_ref()).unwrap();
    assert_eq!(before, fixture());

    let mut after = before.clone();
    after.insert("manual/aaaaaaaa/2026-10.beancount", "; new\n");
    after.insert(
        "accounts.beancount",
        format!("{ACCOUNTS}2020-01-01 open Assets:Cash\n"),
    );
    let commit = git
        .commit(
            head.as_ref(),
            &before,
            &after,
            "editor: x\n",
            &author("Alice"),
        )
        .unwrap();

    // Creating the commit moves no ref.
    let repo = Repository::open(&path).unwrap();
    let local_main = |repo: &Repository| {
        CommitId(
            repo.find_reference("refs/heads/main")
                .unwrap()
                .target()
                .unwrap()
                .to_string(),
        )
    };
    assert_eq!(Some(local_main(&repo)), head);
    assert_eq!(remote.head(), head);
    assert!(!path.join("manual/aaaaaaaa/2026-10.beancount").exists());

    assert_eq!(git.push(&commit).unwrap(), PushOutcome::Pushed);
    assert_eq!(remote.head(), Some(commit.clone()));
    assert_eq!(remote.snapshot(), after);

    git.finish(&commit).unwrap();
    assert_eq!(local_main(&repo), commit);
    assert_eq!(
        repo.find_reference("refs/remotes/origin/main")
            .unwrap()
            .target()
            .unwrap()
            .to_string(),
        commit.0
    );
    assert_eq!(
        std::fs::read_to_string(path.join("manual/aaaaaaaa/2026-10.beancount")).unwrap(),
        "; new\n"
    );
    git.ensure_no_local_changes().unwrap();

    let history = remote.history();
    assert_eq!(history.len(), 2);
    assert_eq!(history[1].message, "editor: x\n");
    assert_eq!(history[1].author, "Alice");
    assert_eq!(history[1].email, "alice@example.org");
}

#[test]
fn a_stale_push_is_rejected_and_changes_nothing() {
    let remote = Remote::with(&fixture());
    let a = Client::new("aaaaaaaa", "Alice");
    let b = Client::new("bbbbbbbb", "Bob");
    let mut git_a = a.clone_from(&remote);
    let mut git_b = b.clone_from(&remote);

    let head = git_a.fetch().unwrap();
    assert_eq!(git_b.fetch().unwrap(), head);
    let before = git_a.snapshot(head.as_ref()).unwrap();

    let mut after_a = before.clone();
    after_a.insert("manual/aaaaaaaa/2026-10.beancount", "; a\n");
    let commit_a = git_a
        .commit(head.as_ref(), &before, &after_a, "editor: a\n", &a.author)
        .unwrap();
    assert_eq!(git_a.push(&commit_a).unwrap(), PushOutcome::Pushed);

    // B still builds on the old head.
    let mut after_b = before.clone();
    after_b.insert("manual/bbbbbbbb/2026-10.beancount", "; b\n");
    let commit_b = git_b
        .commit(head.as_ref(), &before, &after_b, "editor: b\n", &b.author)
        .unwrap();
    assert_eq!(git_b.push(&commit_b).unwrap(), PushOutcome::Rejected);
    assert_eq!(remote.head(), Some(commit_a.clone()));

    // After fetching, B's rebuilt commit goes through.
    let head = git_b.fetch().unwrap();
    assert_eq!(head, Some(commit_a));
    let before = git_b.snapshot(head.as_ref()).unwrap();
    let mut after_b = before.clone();
    after_b.insert("manual/bbbbbbbb/2026-10.beancount", "; b\n");
    let commit_b = git_b
        .commit(head.as_ref(), &before, &after_b, "editor: b\n", &b.author)
        .unwrap();
    assert_eq!(git_b.push(&commit_b).unwrap(), PushOutcome::Pushed);
    assert_eq!(remote.history().len(), 3);
}

#[test]
fn local_mode_moves_main_by_compare_and_swap() {
    let dir = TempDir::new().unwrap();
    let mut git = Git2Backend::init(dir.path()).unwrap();
    assert_eq!(git.fetch().unwrap(), None);
    assert_eq!(git.snapshot(None).unwrap(), Snapshot::new());

    let files = fixture();
    let root = git
        .commit(
            None,
            &Snapshot::new(),
            &files,
            "admin: init\n",
            &author("Admin"),
        )
        .unwrap();
    // A second root commit, built while main did not exist yet.
    let other_root = git
        .commit(
            None,
            &Snapshot::new(),
            &Snapshot::from_files([("a", "b")]),
            "admin: other\n",
            &author("Admin"),
        )
        .unwrap();
    assert_eq!(git.fetch().unwrap(), None, "commit must not move main");

    assert_eq!(git.push(&root).unwrap(), PushOutcome::Pushed);
    git.finish(&root).unwrap();
    assert_eq!(git.fetch().unwrap(), Some(root.clone()));
    assert_eq!(
        std::fs::read_to_string(dir.path().join(RUN_FILE)).unwrap(),
        run_file(BALANCE)
    );
    assert_eq!(git.push(&other_root).unwrap(), PushOutcome::Rejected);

    let mut next = files.clone();
    next.insert("manual/aaaaaaaa/2026-10.beancount", "; 1\n");
    let first = git
        .commit(Some(&root), &files, &next, "editor: 1\n", &author("A"))
        .unwrap();
    let mut stale = files.clone();
    stale.insert("manual/bbbbbbbb/2026-10.beancount", "; 2\n");
    let second = git
        .commit(Some(&root), &files, &stale, "editor: 2\n", &author("B"))
        .unwrap();
    assert_eq!(git.push(&first).unwrap(), PushOutcome::Pushed);
    git.finish(&first).unwrap();
    assert_eq!(git.push(&second).unwrap(), PushOutcome::Rejected);
    assert_eq!(git.fetch().unwrap(), Some(first.clone()));
    assert_eq!(
        std::fs::read_to_string(dir.path().join("manual/aaaaaaaa/2026-10.beancount")).unwrap(),
        "; 1\n"
    );
    assert!(!dir.path().join("manual/bbbbbbbb").exists());
    git.ensure_no_local_changes().unwrap();

    // Another process holding the lock on main counts as a lost race.
    let mut third_files = next.clone();
    third_files.insert("manual/aaaaaaaa/2026-11.beancount", "; 3\n");
    let third = git
        .commit(
            Some(&first),
            &next,
            &third_files,
            "editor: 3\n",
            &author("A"),
        )
        .unwrap();
    let lock = dir.path().join(".git/refs/heads/main.lock");
    std::fs::write(&lock, "").unwrap();
    assert_eq!(git.push(&third).unwrap(), PushOutcome::Rejected);
    std::fs::remove_file(&lock).unwrap();
    assert_eq!(git.push(&third).unwrap(), PushOutcome::Pushed);
}

#[test]
fn finish_updates_the_working_tree() {
    let remote = Remote::with(&fixture());
    let a = Client::new("aaaaaaaa", "Alice");
    let b = Client::new("bbbbbbbb", "Bob");
    let mut git_a = a.clone_from(&remote);
    let mut git_b = b.clone_from(&remote);

    // B adds, modifies and removes files, including a whole directory.
    let head = git_b.fetch().unwrap();
    let before = git_b.snapshot(head.as_ref()).unwrap();
    let mut after = before.clone();
    after.insert("manual/bbbbbbbb/2026-10.beancount", "; b\n");
    after.insert(
        "accounts.beancount",
        format!("{ACCOUNTS}2020-01-01 open Assets:Cash\n"),
    );
    after.remove("sources/giro.toml");
    let commit = git_b
        .commit(head.as_ref(), &before, &after, "admin: b\n", &b.author)
        .unwrap();
    assert_eq!(git_b.push(&commit).unwrap(), PushOutcome::Pushed);

    // A only learns about it by fetching; finish moves its clone.
    let head = git_a.fetch().unwrap();
    assert_eq!(head, Some(commit.clone()));
    assert!(!a.path().join("manual/bbbbbbbb").exists());
    git_a.finish(&commit).unwrap();
    assert_eq!(a.file("manual/bbbbbbbb/2026-10.beancount"), "; b\n");
    assert!(a.file("accounts.beancount").ends_with("open Assets:Cash\n"));
    assert!(!a.path().join("sources").exists());
    git_a.ensure_no_local_changes().unwrap();
    assert_eq!(Snapshot::from_dir(&a.path()).unwrap(), after);
}

#[test]
fn finish_never_overwrites_uncommitted_changes() {
    let remote = Remote::with(&fixture());
    let a = Client::new("aaaaaaaa", "Alice");
    let b = Client::new("bbbbbbbb", "Bob");
    let mut git_a = a.clone_from(&remote);
    let mut git_b = b.clone_from(&remote);

    let head = git_b.fetch().unwrap();
    let before = git_b.snapshot(head.as_ref()).unwrap();
    let mut after = before.clone();
    after.insert(
        "accounts.beancount",
        format!("{ACCOUNTS}2020-01-01 open Assets:Cash\n"),
    );
    let commit = git_b
        .commit(head.as_ref(), &before, &after, "admin: b\n", &b.author)
        .unwrap();
    assert_eq!(git_b.push(&commit).unwrap(), PushOutcome::Pushed);

    write(&a.path(), "accounts.beancount", "; edited by hand\n");
    let head = git_a.fetch().unwrap().unwrap();
    assert!(matches!(
        git_a.finish(&head),
        Err(GitError::LocalChanges(_))
    ));
    assert_eq!(a.file("accounts.beancount"), "; edited by hand\n");
}

#[test]
fn ensure_no_local_changes_finds_edits_and_unpushed_commits() {
    let remote = Remote::with(&fixture());
    let a = Client::new("aaaaaaaa", "Alice");
    let git = a.clone_from(&remote);
    git.ensure_no_local_changes().unwrap();

    // Untracked files are fine.
    write(&a.path(), "notes.txt", "x");
    git.ensure_no_local_changes().unwrap();

    // Changes to tracked files are not, staged or not.
    write(&a.path(), "rules.toml", "# changed\n");
    assert_eq!(
        git.ensure_no_local_changes(),
        Err(GitError::LocalChanges("rules.toml".to_owned()))
    );
    std::fs::remove_file(a.path().join("rules.toml")).unwrap();
    assert!(matches!(
        git.ensure_no_local_changes(),
        Err(GitError::LocalChanges(_))
    ));

    // Restore, then commit locally without pushing.
    let repo = Repository::open(a.path()).unwrap();
    let mut checkout = git2::build::CheckoutBuilder::new();
    checkout.force();
    repo.checkout_head(Some(&mut checkout)).unwrap();
    git.ensure_no_local_changes().unwrap();

    let mut index = repo.index().unwrap();
    write(&a.path(), "rules.toml", "# committed locally\n");
    index.add_path(Path::new("rules.toml")).unwrap();
    index.write().unwrap();
    assert!(matches!(
        git.ensure_no_local_changes(),
        Err(GitError::LocalChanges(_))
    ));
    let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
    let parent = repo.head().unwrap().peel_to_commit().unwrap();
    let signature = git2::Signature::now("A", "a@example.org").unwrap();
    repo.commit(
        Some("HEAD"),
        &signature,
        &signature,
        "local",
        &tree,
        &[&parent],
    )
    .unwrap();
    assert_eq!(
        git.ensure_no_local_changes(),
        Err(GitError::LocalChanges("refs/heads/main".to_owned()))
    );
}

#[test]
fn commits_rewrite_only_changed_paths() {
    let remote = Remote::with(&fixture());
    let dir = TempDir::new().unwrap();
    let mut git = Git2Backend::clone_bare(&remote.url(), dir.path(), Credentials::Default).unwrap();
    let repo = Repository::open(dir.path()).unwrap();

    // Add an executable file, a symlink and a submodule entry by hand: they
    // lie outside the snapshot model but must survive Lentil's commits.
    let head = git.fetch().unwrap().unwrap();
    let base = repo
        .find_commit(git2::Oid::from_str(&head.0).unwrap())
        .unwrap();
    let mut builder = repo.treebuilder(Some(&base.tree().unwrap())).unwrap();
    let script = repo.blob(b"#!/bin/sh\n").unwrap();
    let target = repo.blob(b"main.beancount").unwrap();
    builder.insert("run.sh", script, 0o100_755).unwrap();
    builder.insert("link", target, 0o120_000).unwrap();
    builder.insert("module", base.id(), 0o160_000).unwrap();
    let tree = repo.find_tree(builder.write().unwrap()).unwrap();
    let signature = git2::Signature::now("Admin", "admin@example.org").unwrap();
    let extended = repo
        .commit(
            None,
            &signature,
            &signature,
            "admin: extras\n",
            &tree,
            &[&base],
        )
        .unwrap();
    let extended = CommitId(extended.to_string());

    let before = git.snapshot(Some(&extended)).unwrap();
    assert!(before.contains("run.sh"));
    assert!(!before.contains("link"));
    assert!(!before.contains("module"));

    let mut after = before.clone();
    after.insert("run.sh", "#!/bin/sh\necho\n");
    after.insert("manual/aaaaaaaa/2026-10.beancount", "; a\n");
    after.remove("prices/keep/keep.beancount");
    let commit = git
        .commit(
            Some(&extended),
            &before,
            &after,
            "admin: y\n",
            &author("Admin"),
        )
        .unwrap();
    assert_eq!(git.snapshot(Some(&commit)).unwrap(), after);

    let old = repo
        .find_commit(git2::Oid::from_str(&extended.0).unwrap())
        .unwrap();
    let new = repo
        .find_commit(git2::Oid::from_str(&commit.0).unwrap())
        .unwrap();
    assert_eq!(new.parent_ids().collect::<Vec<_>>(), vec![old.id()]);
    let (old_tree, new_tree) = (old.tree().unwrap(), new.tree().unwrap());
    let entry = |tree: &git2::Tree<'_>, path: &str| tree.get_path(Path::new(path)).ok();
    // Untouched subtrees keep their objects.
    for path in ["imports", "sources", "link", "module"] {
        assert_eq!(
            entry(&old_tree, path).map(|e| (e.id(), e.filemode())),
            entry(&new_tree, path).map(|e| (e.id(), e.filemode())),
            "{path}"
        );
    }
    assert_eq!(entry(&new_tree, "run.sh").unwrap().filemode(), 0o100_755);
    // A directory whose last file was removed disappears.
    assert!(entry(&new_tree, "prices").is_none());
    assert_eq!(new.message().unwrap(), "admin: y\n");
}

#[test]
fn bare_clones_sync_and_only_move_refs() {
    let remote = Remote::with(&fixture());
    let dir = TempDir::new().unwrap();
    let mut git = Git2Backend::clone_bare(&remote.url(), dir.path(), Credentials::Default).unwrap();
    git.ensure_no_local_changes().unwrap();
    let head = git.fetch().unwrap();
    assert_eq!(git.snapshot(head.as_ref()).unwrap(), fixture());
    let before = fixture();
    let mut after = before.clone();
    after.insert("manual/aaaaaaaa/2026-10.beancount", "; a\n");
    let commit = git
        .commit(head.as_ref(), &before, &after, "editor: a\n", &author("A"))
        .unwrap();
    assert_eq!(git.push(&commit).unwrap(), PushOutcome::Pushed);
    git.finish(&commit).unwrap();
    let repo = Repository::open(dir.path()).unwrap();
    assert!(repo.is_bare());
    for name in ["refs/heads/main", "refs/remotes/origin/main"] {
        assert_eq!(
            repo.find_reference(name)
                .unwrap()
                .target()
                .unwrap()
                .to_string(),
            commit.0
        );
    }
    git.ensure_no_local_changes().unwrap();
}

#[test]
fn cloning_an_empty_remote_gives_an_unborn_main() {
    let remote = Remote::empty();
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("clone");
    let mut git = Git2Backend::clone_from(&remote.url(), &path, Credentials::Default).unwrap();
    assert_eq!(git.fetch().unwrap(), None);
    git.ensure_no_local_changes().unwrap();

    let outcome = run_sync(
        &mut git,
        &mut Recorder::default(),
        &admin(),
        &author("Admin"),
        |before| {
            assert!(before.is_empty());
            Work::Commit {
                after: fixture(),
                subject: "init".to_owned(),
                result: (),
            }
        },
    )
    .unwrap();
    assert_eq!(outcome.attempts, 1);
    assert_eq!(remote.head(), outcome.commit);
    assert_eq!(remote.snapshot(), fixture());
    assert_eq!(
        remote.history()[0].message,
        message("admin", "init", "adminxyz")
    );
    assert_eq!(Snapshot::from_dir(&path).unwrap(), fixture());
}

#[test]
fn clone_failures_leave_nothing_behind() {
    let remote = Remote::with(&fixture());
    let dir = TempDir::new().unwrap();

    let occupied = dir.path().join("occupied");
    write(&occupied, "keep.txt", "mine");
    assert!(matches!(
        Git2Backend::clone_from(&remote.url(), &occupied, Credentials::Default),
        Err(GitError::Other(_))
    ));
    assert_eq!(
        std::fs::read_to_string(occupied.join("keep.txt")).unwrap(),
        "mine"
    );
    assert!(!occupied.join(".git").exists());

    let missing_remote = dir
        .path()
        .join("nowhere.git")
        .to_string_lossy()
        .into_owned();
    let fresh = dir.path().join("fresh");
    assert!(Git2Backend::clone_from(&missing_remote, &fresh, Credentials::Default).is_err());
    assert!(!fresh.exists());

    let empty = dir.path().join("empty");
    std::fs::create_dir(&empty).unwrap();
    assert!(Git2Backend::clone_from(&missing_remote, &empty, Credentials::Default).is_err());
    assert!(empty.exists());
    assert_eq!(std::fs::read_dir(&empty).unwrap().count(), 0);
}

#[test]
fn open_init_and_author() {
    let dir = TempDir::new().unwrap();
    assert!(matches!(
        Git2Backend::open(dir.path(), Credentials::Default),
        Err(GitError::NotFound(_))
    ));
    let git = Git2Backend::init(dir.path()).unwrap();
    assert!(
        Git2Backend::init(dir.path()).is_err(),
        "never re-initializes"
    );
    let repo = Repository::open(dir.path()).unwrap();
    assert_eq!(
        repo.find_reference("HEAD")
            .unwrap()
            .symbolic_target()
            .unwrap(),
        Some("refs/heads/main")
    );
    let mut config = repo.config().unwrap();
    config.set_str("user.name", "Tim Example").unwrap();
    config.set_str("user.email", "tim@example.org").unwrap();
    assert_eq!(
        git.configured_author(),
        Some(Author {
            name: "Tim Example".into(),
            email: "tim@example.org".into()
        })
    );

    let mut git = Git2Backend::open(dir.path(), Credentials::Default).unwrap();
    git.set_remote("/nowhere/a.git").unwrap();
    git.set_remote("/nowhere/b.git").unwrap();
    let remote = repo.find_remote("origin").unwrap();
    assert_eq!(remote.url().unwrap(), "/nowhere/b.git");
    assert!(git.fetch().is_err());
}

// ---------------------------------------------------------------------------
// The sync loop
// ---------------------------------------------------------------------------

#[test]
fn three_writers_interleaved_lose_nothing() {
    let remote = Remote::with(&fixture());
    let mut a = Client::new("aaaaaaaa", "Alice");
    let mut b = Client::new("bbbbbbbb", "Bob");
    let mut c = Client::new("cccccccc", "Carol");
    let intents_a = [
        confirm("a1", 1),
        confirm("a2", 2),
        set_account("a3", 3, "Expenses:Food:Groceries"),
    ];
    let intents_b = [
        confirm("b1", 2),
        confirm("b2", 4),
        set_account("b3", 5, "Expenses:Household"),
    ];
    let intents_c = [confirm("c1", 6), set_account("c2", 7, "Expenses:Utilities")];

    // C pushes while B is between fetch and push, and B while A is.
    let git_c = c.clone_from(&remote);
    let git_b = b.clone_from(&remote);
    let git_a = a.clone_from(&remote);
    let (mut result_b, mut result_c) = (None, None);
    let (result_a, pushes_a, pushes_b) = {
        let mut git_c = git_c;
        let mut git_b = Interleaved::new(
            git_b,
            vec![Box::new(|| result_c = Some(c.sync(&mut git_c, &intents_c)))],
        );
        let mut git_a = Interleaved::new(
            git_a,
            vec![Box::new(|| {
                result_b = Some(b.sync(&mut git_b, &intents_b));
            })],
        );
        let result_a = a.sync(&mut git_a, &intents_a);
        let pushes_a = git_a.pushes.clone();
        drop(git_a);
        (result_a, pushes_a, git_b.pushes.clone())
    };
    let result_a = result_a.unwrap();
    let result_b = result_b.unwrap().unwrap();
    let result_c = result_c.unwrap().unwrap();

    // Every intent was applied once; none was lost.
    assert_eq!(
        outcomes(&result_c),
        vec![("c1", &Outcome::Applied), ("c2", &Outcome::Applied)]
    );
    assert_eq!(
        outcomes(&result_b),
        vec![
            ("b1", &Outcome::Applied),
            ("b2", &Outcome::Applied),
            ("b3", &Outcome::Applied)
        ]
    );
    assert_eq!(
        outcomes(&result_a),
        vec![
            ("a1", &Outcome::Applied),
            ("a2", &Outcome::AlreadyApplied),
            ("a3", &Outcome::Applied)
        ]
    );

    // A and B each lost one race and waited once.
    assert_eq!(pushes_a, vec![PushOutcome::Rejected, PushOutcome::Pushed]);
    assert_eq!(pushes_b, vec![PushOutcome::Rejected, PushOutcome::Pushed]);
    assert_eq!(a.sleeper.0, secs(&[1]));
    assert_eq!(b.sleeper.0, secs(&[1]));
    assert!(c.sleeper.0.is_empty());

    // One linear chain: seed, C, B, A, with labelled commits.
    let history = remote.history();
    let ids: Vec<_> = history.iter().map(|c| Some(c.id.clone())).collect();
    assert_eq!(
        ids[1..],
        [
            result_c.commit.clone(),
            result_b.commit.clone(),
            result_a.commit.clone()
        ]
    );
    assert_eq!(
        history[1].message,
        message("editor", "confirm 1, set-account 1", "cccccccc")
    );
    assert_eq!(
        history[2].message,
        message("editor", "confirm 2, set-account 1", "bbbbbbbb")
    );
    assert_eq!(
        history[3].message,
        message("editor", "confirm 1, set-account 1", "aaaaaaaa")
    );
    assert_eq!(
        history
            .iter()
            .map(|c| c.author.as_str())
            .collect::<Vec<_>>(),
        vec!["Admin", "Carol", "Bob", "Alice"]
    );

    // The final state holds every change exactly once.
    let state = remote.snapshot();
    let ledger = load(&state);
    assert!(ledger.errors.is_empty(), "{:?}", ledger.errors);
    let ids: Vec<_> = ledger
        .transactions
        .iter()
        .filter_map(|t| t.id.clone())
        .collect();
    assert_eq!(ids.len(), TRANSACTIONS.len());
    assert_eq!(
        ids.iter().collect::<BTreeSet<_>>().len(),
        TRANSACTIONS.len()
    );
    for n in [1, 2, 4, 6] {
        assert_eq!(flag(&ledger, n), '*', "transaction {n}");
    }
    assert_eq!(flag(&ledger, 8), '!');
    assert_eq!(counter_account(&ledger, 3), "Expenses:Food:Groceries");
    assert_eq!(counter_account(&ledger, 5), "Expenses:Household");
    assert_eq!(counter_account(&ledger, 7), "Expenses:Utilities");
    assert_eq!(counter_account(&ledger, 8), "Expenses:Uncategorized");

    // Each clone was moved to its own commit; A's is the remote's head.
    assert_eq!(Snapshot::from_dir(&a.path()).unwrap(), state);
    let mut git_b = Git2Backend::open(&b.path(), Credentials::Default).unwrap();
    assert_ne!(Snapshot::from_dir(&b.path()).unwrap(), state);
    // A sync without intents only brings B up to date.
    let caught_up = b.sync(&mut git_b, &[]).unwrap();
    assert_eq!(caught_up.commit, None);
    assert!(caught_up.outcomes.is_empty());
    assert_eq!(Snapshot::from_dir(&b.path()).unwrap(), state);
    git_b.ensure_no_local_changes().unwrap();
    assert_eq!(remote.history().len(), 4);
}

#[test]
fn a_conflict_is_reported_and_the_other_intents_still_apply() {
    let remote = Remote::with(&fixture());
    let mut a = Client::new("aaaaaaaa", "Alice");
    let mut b = Client::new("bbbbbbbb", "Bob");
    let intents_a = [
        set_account("a1", 3, "Expenses:Food:Groceries"),
        confirm("a2", 1),
    ];
    let intents_b = [set_account("b1", 3, "Expenses:Household")];

    let mut git_b = b.clone_from(&remote);
    let mut result_b = None;
    let result_a = {
        let mut git_a = Interleaved::new(
            a.clone_from(&remote),
            vec![Box::new(|| result_b = Some(b.sync(&mut git_b, &intents_b)))],
        );
        a.sync(&mut git_a, &intents_a).unwrap()
    };
    let result_b = result_b.unwrap().unwrap();

    assert_eq!(outcomes(&result_b), vec![("b1", &Outcome::Applied)]);
    assert_eq!(
        outcomes(&result_a),
        vec![
            (
                "a1",
                &Outcome::Conflict(Conflict {
                    code: ConflictCode::AccountChanged,
                    current: Some("Expenses:Household".to_owned()),
                })
            ),
            ("a2", &Outcome::Applied)
        ]
    );
    assert_eq!(a.sleeper.0, secs(&[1]));

    let history = remote.history();
    assert_eq!(history.len(), 3);
    assert_eq!(
        history[1].message,
        message("editor", "set-account 1", "bbbbbbbb")
    );
    assert_eq!(
        history[2].message,
        message("editor", "confirm 1", "aaaaaaaa")
    );
    assert_eq!(Some(history[2].id.clone()), result_a.commit);

    let ledger = load(&remote.snapshot());
    assert_eq!(counter_account(&ledger, 3), "Expenses:Household");
    assert_eq!(flag(&ledger, 1), '*');
}

/// A client that pushes one confirmation per call, to make others lose races.
fn rival_hooks<'a>(
    rival: &'a mut Client,
    git: &'a mut Git2Backend,
    transactions: &[usize],
) -> Vec<Hook<'a>> {
    let rival = std::rc::Rc::new(std::cell::RefCell::new((rival, git)));
    transactions
        .iter()
        .map(|&n| {
            let rival = std::rc::Rc::clone(&rival);
            Box::new(move || {
                let (client, git) = &mut *rival.borrow_mut();
                let sync = client.sync(&mut **git, &[confirm("r", n)]).unwrap();
                assert!(sync.commit.is_some());
            }) as Hook<'a>
        })
        .collect()
}

#[test]
fn a_sync_retries_with_exponential_backoff() {
    let remote = Remote::with(&fixture());
    let mut a = Client::new("aaaaaaaa", "Alice");
    let mut rival = Client::new("rrrrrrrr", "Rival");
    let mut git_rival = rival.clone_from(&remote);
    let git_a = a.clone_from(&remote);

    // Four lost races, then the fifth and last attempt succeeds.
    let mut git_a = Interleaved::new(
        git_a,
        rival_hooks(&mut rival, &mut git_rival, &[2, 3, 4, 5]),
    );
    let result = a.sync(&mut git_a, &[confirm("a1", 1)]).unwrap();
    drop(git_a);
    assert_eq!(outcomes(&result), vec![("a1", &Outcome::Applied)]);
    assert_eq!(a.sleeper.0, secs(&[1, 2, 4, 8]));
    let history = remote.history();
    assert_eq!(history.len(), 6);
    assert_eq!(Some(history[5].id.clone()), result.commit);
    let ledger = load(&remote.snapshot());
    for n in 1..=5 {
        assert_eq!(flag(&ledger, n), '*', "transaction {n}");
    }
}

#[test]
fn a_sync_gives_up_after_five_lost_races() {
    let remote = Remote::with(&fixture());
    let mut a = Client::new("aaaaaaaa", "Alice");
    let mut rival = Client::new("rrrrrrrr", "Rival");
    let mut git_rival = rival.clone_from(&remote);
    let git_a = a.clone_from(&remote);

    let mut git_a = Interleaved::new(
        git_a,
        rival_hooks(&mut rival, &mut git_rival, &[2, 3, 4, 5, 6]),
    );
    let result = a.sync(&mut git_a, &[confirm("a1", 1)]);
    assert_eq!(git_a.pushes.len(), MAX_ATTEMPTS as usize);
    drop(git_a);
    assert!(matches!(result, Err(SyncError::TooManyAttempts)));
    assert_eq!(a.sleeper.0, secs(&[1, 2, 4, 8]));

    // Nothing of A's reached the remote; the caller keeps its intent.
    let history = remote.history();
    assert_eq!(history.len(), 6);
    assert!(history.iter().all(|c| !c.message.contains("aaaaaaaa")));
    assert_eq!(flag(&load(&remote.snapshot()), 1), '!');
}

#[test]
fn nothing_to_commit_still_moves_the_clone() {
    let remote = Remote::with(&fixture());
    let mut a = Client::new("aaaaaaaa", "Alice");
    let mut b = Client::new("bbbbbbbb", "Bob");
    let mut git_a = a.clone_from(&remote);
    let mut git_b = b.clone_from(&remote);

    let pushed = a.sync(&mut git_a, &[confirm("a1", 1)]).unwrap();
    assert!(pushed.commit.is_some());

    // B's intent was applied by A already: no commit, but B is up to date.
    let synced = b.sync(&mut git_b, &[confirm("b1", 1)]).unwrap();
    assert_eq!(outcomes(&synced), vec![("b1", &Outcome::AlreadyApplied)]);
    assert_eq!(synced.commit, None);
    assert_eq!(Snapshot::from_dir(&b.path()).unwrap(), remote.snapshot());
    assert_eq!(remote.history().len(), 2);

    // A commit whose state equals the fetched one is no commit either.
    let outcome = run_sync(
        &mut git_b,
        &mut b.sleeper,
        &b.identity,
        &b.author,
        |before| Work::Commit {
            after: before.clone(),
            subject: "nothing".to_owned(),
            result: 7,
        },
    )
    .unwrap();
    assert_eq!(
        (outcome.result, outcome.commit, outcome.attempts),
        (7, None, 1)
    );
    assert_eq!(remote.history().len(), 2);
}

#[test]
fn a_read_only_repository_stops_editors_but_not_the_admin() {
    let mut files = fixture();
    files.insert(
        "lentil.toml",
        LENTIL_TOML.replace("spec_version = 0", "spec_version = 1"),
    );
    let remote = Remote::with(&files);
    let mut a = Client::new("aaaaaaaa", "Alice");
    let mut git = a.clone_from(&remote);

    let mut called = false;
    let result = run_sync(&mut git, &mut a.sleeper, &a.identity, &a.author, |_| {
        called = true;
        Work::Nothing(())
    });
    let Err(SyncError::ReadOnly(findings)) = result else {
        panic!("expected ReadOnly, got {result:?}");
    };
    assert!(!findings.is_empty());
    assert!(!called, "work must not run on a read-only state");
    assert!(matches!(
        a.sync(&mut git, &[confirm("a1", 1)]),
        Err(SyncError::ReadOnly(_))
    ));

    let outcome = run_sync(&mut git, &mut a.sleeper, &admin(), &a.author, |before| {
        assert_eq!(*before, files);
        Work::Nothing("seen")
    })
    .unwrap();
    assert_eq!(outcome.result, "seen");
    assert_eq!(remote.history().len(), 1);
}

#[test]
fn blocking_findings_stop_the_commit() {
    let remote = Remote::with(&fixture());
    let mut a = Client::new("aaaaaaaa", "Alice");
    let mut git = a.clone_from(&remote);
    let head = remote.head();

    // Editors must not change main.beancount (spec sections 3 and 7).
    let result = run_sync(&mut git, &mut a.sleeper, &a.identity, &a.author, |before| {
        let mut after = before.clone();
        after.insert(
            "main.beancount",
            format!("{MAIN}include \"extra.beancount\"\n"),
        );
        after.insert("extra.beancount", "");
        Work::Commit {
            after,
            subject: "sneak".to_owned(),
            result: (),
        }
    });
    let Err(SyncError::Blocked(findings)) = result else {
        panic!("expected Blocked, got {result:?}");
    };
    assert!(findings.iter().any(|f| f.severity == Severity::Blocking));
    assert_eq!(remote.head(), head);
    assert_eq!(remote.history().len(), 1);
    assert_eq!(a.file("main.beancount"), MAIN);
}

#[test]
fn warnings_of_the_committed_state_are_returned() {
    // The assertion is one cent off: reported, never blocking (spec section 13).
    let remote = Remote::with(&fixture_with("2318.20"));
    let mut a = Client::new("aaaaaaaa", "Alice");
    let mut git = a.clone_from(&remote);
    let synced = a.sync(&mut git, &[confirm("a1", 1)]).unwrap();
    assert!(synced.commit.is_some());
    assert!(
        synced
            .warnings
            .iter()
            .any(|f| f.code == FindingCode::BalanceAssertionFailed && !f.is_blocking()),
        "{:?}",
        synced.warnings
    );
}

#[test]
fn a_bare_client_syncs_like_the_app() {
    let remote = Remote::with(&fixture());
    let mut app = Client::new("appappap", "App");
    let mut git =
        Git2Backend::clone_bare(&remote.file_url(), &app.path(), Credentials::Default).unwrap();
    let synced = app
        .sync(
            &mut git,
            &[
                confirm("k1", 8),
                set_account("k2", 8, "Expenses:Food:Groceries"),
            ],
        )
        .unwrap();
    assert_eq!(
        outcomes(&synced),
        vec![("k1", &Outcome::Applied), ("k2", &Outcome::Applied)]
    );
    let history = remote.history();
    assert_eq!(history.len(), 2);
    assert_eq!(
        history[1].message,
        message("editor", "confirm 1, set-account 1", "appappap")
    );
    let ledger = load(&remote.snapshot());
    assert_eq!(flag(&ledger, 8), '*');
    assert_eq!(counter_account(&ledger, 8), "Expenses:Food:Groceries");
}

#[test]
fn a_local_repository_syncs_without_a_remote() {
    let dir = TempDir::new().unwrap();
    let mut git = Git2Backend::init(dir.path()).unwrap();
    let mut client = Client::new("aaaaaaaa", "Alice");

    let init = run_sync(
        &mut git,
        &mut client.sleeper,
        &admin(),
        &client.author,
        |_| Work::Commit {
            after: fixture(),
            subject: "init".to_owned(),
            result: (),
        },
    )
    .unwrap();
    assert!(init.commit.is_some());
    assert_eq!(Snapshot::from_dir(dir.path()).unwrap(), fixture());

    let synced = client.sync(&mut git, &[confirm("a1", 2)]).unwrap();
    assert_eq!(outcomes(&synced), vec![("a1", &Outcome::Applied)]);
    let state = Snapshot::from_dir(dir.path()).unwrap();
    assert_eq!(flag(&load(&state), 2), '*');
    git.ensure_no_local_changes().unwrap();

    let repo = Repository::open(dir.path()).unwrap();
    let head = repo.head().unwrap().peel_to_commit().unwrap();
    assert_eq!(head.id().to_string(), synced.commit.unwrap().0);
    assert_eq!(head.parent_count(), 1);
    assert_eq!(
        head.message().unwrap(),
        message("editor", "confirm 1", "aaaaaaaa")
    );
}
