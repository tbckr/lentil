//! Synchronization by operation replay (spec sections 11 and 12).
//!
//! A sync never merges text: it fetches `main`, reads the fetched tree into a
//! [`Snapshot`], lets the caller compute its change on that fresh state,
//! validates, commits once and pushes without force. If the push is rejected,
//! it starts over, with exponential backoff and at most [`MAX_ATTEMPTS`].

pub mod git;

use std::time::Duration;

use thiserror::Error;

use crate::finding::Finding;
use crate::intent::{Outcome, PendingIntent};
use crate::snapshot::Snapshot;
use crate::validate::Role;

/// Attempts before a sync gives up (spec section 11, step 6).
pub const MAX_ATTEMPTS: u32 = 5;

/// The only branch (spec section 12).
pub const BRANCH: &str = "main";

/// A commit hash.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct CommitId(pub String);

/// Commit author, from the user's client settings (spec section 12).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Author {
    /// Name.
    pub name: String,
    /// E-mail address.
    pub email: String,
}

/// Who writes: the values of the `Lentil-*` trailers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Identity {
    /// `Lentil-Role`.
    pub role: Role,
    /// `Lentil-Client`, such as `lentil-cli/0.1.0`.
    pub client: String,
    /// `Lentil-Device`.
    pub device: String,
}

/// Credentials for the remote. Never stored in the repository.
#[derive(Clone, PartialEq, Eq)]
pub enum Credentials {
    /// SSH agent and Git credential helpers, as configured for the user.
    Default,
    /// HTTPS user name and password or token.
    UserPassword {
        /// User name.
        username: String,
        /// Password or token.
        password: String,
    },
    /// An SSH private key in memory.
    SshKey {
        /// SSH user, usually `git`.
        username: String,
        /// PEM or OpenSSH private key.
        private_key: String,
        /// Passphrase, if the key has one.
        passphrase: Option<String>,
    },
}

impl std::fmt::Debug for Credentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never print secrets.
        f.write_str(match self {
            Self::Default => "Credentials::Default",
            Self::UserPassword { .. } => "Credentials::UserPassword(..)",
            Self::SshKey { .. } => "Credentials::SshKey(..)",
        })
    }
}

/// Why a Git operation failed. Codes, not text: each client translates.
#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum GitError {
    /// The remote is unreachable (offline, DNS, TLS).
    #[error("network: {0}")]
    Network(String),
    /// The remote rejected the credentials.
    #[error("authentication failed")]
    Auth,
    /// The remote has no `main` branch or the repository does not exist.
    #[error("not found: {0}")]
    NotFound(String),
    /// The local clone has changes or commits that a sync would discard.
    #[error("local changes: {0}")]
    LocalChanges(String),
    /// Anything else, with libgit2's message as detail.
    #[error("git: {0}")]
    Other(String),
}

/// Result of a push without force.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PushOutcome {
    /// The remote accepted the commit.
    Pushed,
    /// The remote moved on; start over.
    Rejected,
}

/// The Git boundary. Tests replace it with a fake or a local bare repository.
pub trait GitBackend {
    /// Fetches `main` and returns its head, or `None` if the remote is empty.
    /// In local mode (no remote) returns the local `main`.
    ///
    /// # Errors
    ///
    /// Returns a [`GitError`] if fetching fails.
    fn fetch(&mut self) -> Result<Option<CommitId>, GitError>;

    /// Reads the tree of `commit` into a snapshot; `None` gives an empty snapshot.
    ///
    /// # Errors
    ///
    /// Returns a [`GitError`] if the commit cannot be read.
    fn snapshot(&self, commit: Option<&CommitId>) -> Result<Snapshot, GitError>;

    /// Creates a commit on top of `parent` whose tree is `after`. `before` is
    /// the parent's snapshot, so only changed files need writing.
    ///
    /// # Errors
    ///
    /// Returns a [`GitError`] if the commit cannot be written.
    fn commit(
        &mut self,
        parent: Option<&CommitId>,
        before: &Snapshot,
        after: &Snapshot,
        message: &str,
        author: &Author,
    ) -> Result<CommitId, GitError>;

    /// Pushes `commit` to `main` without force. In local mode, moves the local `main`.
    ///
    /// # Errors
    ///
    /// Returns a [`GitError`] for failures other than a rejection.
    fn push(&mut self, commit: &CommitId) -> Result<PushOutcome, GitError>;

    /// After a successful push: moves the local `main` (and the working tree,
    /// if there is one) to `commit`.
    ///
    /// # Errors
    ///
    /// Returns a [`GitError`] if the local state cannot be updated.
    fn finish(&mut self, commit: &CommitId) -> Result<(), GitError>;
}

/// Waiting between attempts; a boundary so tests do not sleep.
pub trait Sleeper {
    /// Waits for `duration`.
    fn sleep(&mut self, duration: Duration);
}

/// Sleeps with the operating system.
#[derive(Clone, Copy, Debug, Default)]
pub struct ThreadSleeper;

impl Sleeper for ThreadSleeper {
    fn sleep(&mut self, duration: Duration) {
        std::thread::sleep(duration);
    }
}

/// What the caller wants to commit on a fresh state.
#[derive(Clone, Debug)]
pub enum Work<T> {
    /// Commit `after` with `subject` (without the role prefix).
    Commit {
        /// The complete new state.
        after: Snapshot,
        /// Readable summary, such as `confirm 12, set-account 3`.
        subject: String,
        /// Caller data returned on success.
        result: T,
    },
    /// Nothing to commit (for example an import with nothing new).
    Nothing(T),
}

/// A finished sync.
#[derive(Clone, Debug)]
pub struct SyncOutcome<T> {
    /// The caller data of the attempt that succeeded.
    pub result: T,
    /// The pushed commit, `None` if there was nothing to commit.
    pub commit: Option<CommitId>,
    /// Attempts used, 1 to [`MAX_ATTEMPTS`].
    pub attempts: u32,
    /// Warnings of the committed state, such as failing balance assertions.
    pub warnings: Vec<Finding>,
}

/// Why a sync stopped.
#[derive(Clone, Debug, Error)]
pub enum SyncError {
    /// A Git operation failed; pending intents stay.
    #[error(transparent)]
    Git(#[from] GitError),
    /// The fetched state is read-only for this role (spec section 13).
    #[error("repository is read-only")]
    ReadOnly(Vec<Finding>),
    /// The new state has blocking findings; nothing was committed.
    #[error("validation failed")]
    Blocked(Vec<Finding>),
    /// Every attempt was rejected because others pushed first.
    #[error("gave up after {MAX_ATTEMPTS} attempts")]
    TooManyAttempts,
}

/// The commit message of spec section 12: `<role>: <subject>`, a blank line
/// and the four trailers.
#[must_use]
pub fn commit_message(subject: &str, identity: &Identity) -> String {
    let _ = (subject, identity);
    todo!("task D")
}

/// Runs the loop of spec section 11 around `work`.
///
/// `work` is called once per attempt with the freshly fetched state and
/// computes the complete new state from it. The result is validated for
/// `identity.role` against the fetched state before it is committed.
///
/// # Errors
///
/// See [`SyncError`].
pub fn run_sync<G, T>(
    git: &mut G,
    sleeper: &mut dyn Sleeper,
    identity: &Identity,
    author: &Author,
    work: impl FnMut(&Snapshot) -> Work<T>,
) -> Result<SyncOutcome<T>, SyncError>
where
    G: GitBackend + ?Sized,
{
    let _ = (git, sleeper, identity, author, work);
    todo!("task D")
}

/// Result of an editor sync.
#[derive(Clone, Debug)]
pub struct EditorSync {
    /// One outcome per intent. The caller removes applied and already-applied
    /// intents and moves conflicts to the user's conflict list.
    pub outcomes: Vec<(String, Outcome)>,
    /// The pushed commit, if anything changed.
    pub commit: Option<CommitId>,
    /// Warnings of the committed state.
    pub warnings: Vec<Finding>,
}

/// Syncs pending editor intents: replay on the fetched state, validate,
/// commit `editor: confirm 3, …` and push (spec section 11).
///
/// # Errors
///
/// See [`SyncError`]. On error, the caller keeps every intent.
pub fn sync_intents<G>(
    git: &mut G,
    sleeper: &mut dyn Sleeper,
    identity: &Identity,
    author: &Author,
    intents: &[PendingIntent],
) -> Result<EditorSync, SyncError>
where
    G: GitBackend + ?Sized,
{
    let _ = (git, sleeper, identity, author, intents);
    todo!("task D")
}
