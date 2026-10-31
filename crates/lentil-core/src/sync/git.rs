//! [`GitBackend`] on libgit2.
//!
//! The remote is always `origin` and the branch always `main` (spec section
//! 12). Fetching updates only `refs/remotes/origin/main`; commits are created
//! without moving any ref; pushes never force; and only [`GitBackend::finish`]
//! moves the local `main` and the working tree, after the remote accepted the
//! commit.

use std::cell::RefCell;
use std::path::Path;
use std::sync::LazyLock;

use git2::build::{CheckoutBuilder, TreeUpdateBuilder};
use git2::{
    AutotagOption, Cred, CredentialType, ErrorClass, ErrorCode, FetchOptions, FileMode, ObjectType,
    Oid, PushOptions, Remote, RemoteCallbacks, Repository, RepositoryInitOptions, Signature,
    StatusOptions, Tree,
};
use regex::Regex;

use super::{Author, BRANCH, CommitId, Credentials, GitBackend, GitError, PushOutcome};
use crate::snapshot::{Change, Snapshot};

/// The only remote.
const REMOTE: &str = "origin";
/// The local branch, `refs/heads/` + [`BRANCH`].
const LOCAL_REF: &str = "refs/heads/main";
/// The remote-tracking branch of [`LOCAL_REF`].
const TRACKING_REF: &str = "refs/remotes/origin/main";
/// Fetches only `main`, so a clone never carries other branches.
const FETCH_REFSPEC: &str = "+refs/heads/main:refs/remotes/origin/main";
/// Reflog message of every ref a sync moves.
const REFLOG: &str = "lentil: sync";
/// Reflog message of the remote-tracking branch after a fetch.
const REFLOG_FETCH: &str = "lentil: fetch";
/// libgit2 asks again after rejected credentials; give up after this many requests.
const MAX_CREDENTIAL_REQUESTS: u32 = 4;

/// A local clone driven through libgit2.
///
/// With a remote named `origin`, `fetch` and `push` talk to it. Without one
/// (a repository that only lives locally), `fetch` reads the local `main` and
/// `push` moves it.
pub struct Git2Backend {
    repo: Repository,
    credentials: Credentials,
}

impl std::fmt::Debug for Git2Backend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Credentials are never printed.
        f.debug_struct("Git2Backend")
            .field("path", &self.repo.path())
            .finish_non_exhaustive()
    }
}

impl Git2Backend {
    /// Opens an existing clone at `path`.
    ///
    /// # Errors
    ///
    /// Returns a [`GitError`] if `path` is not a Git repository.
    pub fn open(path: &Path, credentials: Credentials) -> Result<Self, GitError> {
        let repo = Repository::open(path).map_err(|e| map_error(&e))?;
        Ok(Self { repo, credentials })
    }

    /// Clones `url` into `path` and checks out `main`.
    ///
    /// `path` must not exist or be an empty directory. An empty remote gives
    /// a clone whose `main` is unborn; the first sync creates it. On failure,
    /// nothing is left behind at `path`.
    ///
    /// # Errors
    ///
    /// Returns a [`GitError`] if cloning fails.
    pub fn clone_from(url: &str, path: &Path, credentials: Credentials) -> Result<Self, GitError> {
        Self::clone_into(url, path, credentials, false)
    }

    /// Clones `url` into `path` without a working tree, as the Android app
    /// does: it reads snapshots from the object database only.
    ///
    /// Otherwise like [`Git2Backend::clone_from`].
    ///
    /// # Errors
    ///
    /// Returns a [`GitError`] if cloning fails.
    pub fn clone_bare(url: &str, path: &Path, credentials: Credentials) -> Result<Self, GitError> {
        Self::clone_into(url, path, credentials, true)
    }

    /// Creates a new repository at `path` with `main` as its initial branch.
    ///
    /// # Errors
    ///
    /// Returns a [`GitError`] if the repository cannot be created, including
    /// when `path` already holds one.
    pub fn init(path: &Path) -> Result<Self, GitError> {
        Self::create(path, false, Credentials::Default)
    }

    /// Points an unborn HEAD (a repository without commits, for example
    /// created with another default branch) at `main`, so a later checkout
    /// fills the working tree.
    ///
    /// # Errors
    ///
    /// [`GitError::LocalChanges`] if HEAD already points at commits on
    /// another branch.
    pub fn point_unborn_head_to_main(&mut self) -> Result<(), GitError> {
        match self.repo.head() {
            Ok(head) if head.name().ok() == Some(LOCAL_REF) => Ok(()),
            Ok(head) => Err(GitError::LocalChanges(format!(
                "HEAD is on {}, not {BRANCH}",
                head.shorthand().unwrap_or("?")
            ))),
            Err(e) if e.code() == ErrorCode::UnbornBranch => {
                self.repo.set_head(LOCAL_REF).map_err(|e| map_error(&e))
            }
            Err(e) => Err(map_error(&e)),
        }
    }

    /// Sets `origin` to `url`.
    ///
    /// # Errors
    ///
    /// Returns a [`GitError`] if the remote cannot be configured.
    pub fn set_remote(&mut self, url: &str) -> Result<(), GitError> {
        match self.repo.find_remote(REMOTE) {
            Ok(_) => self.repo.remote_set_url(REMOTE, url),
            Err(e) if e.code() == ErrorCode::NotFound => self
                .repo
                .remote_with_fetch(REMOTE, url, FETCH_REFSPEC)
                .map(|_| ()),
            Err(e) => Err(e),
        }
        .map_err(|e| map_error(&e))
    }

    /// The author from the repository's Git configuration (`user.name`, `user.email`).
    #[must_use]
    pub fn configured_author(&self) -> Option<Author> {
        let config = self.repo.config().ok()?;
        let name = config.get_string("user.name").ok()?;
        let email = config.get_string("user.email").ok()?;
        (!name.trim().is_empty() && !email.trim().is_empty()).then_some(Author { name, email })
    }

    /// Fails with [`GitError::LocalChanges`] if the working tree has
    /// uncommitted changes to tracked files or local commits not on the
    /// remote's `main`, which a sync would otherwise discard.
    ///
    /// Untracked files are fine. Local commits are judged against the
    /// remote-tracking branch of the last fetch; without a remote there are
    /// none to lose.
    ///
    /// # Errors
    ///
    /// See above.
    pub fn ensure_no_local_changes(&self) -> Result<(), GitError> {
        if !self.repo.is_bare() {
            let mut options = StatusOptions::new();
            options
                .include_untracked(false)
                .include_ignored(false)
                .exclude_submodules(true);
            let statuses = self
                .repo
                .statuses(Some(&mut options))
                .map_err(|e| map_error(&e))?;
            if let Some(entry) = statuses
                .iter()
                .find(|e| !e.status().is_empty() && !e.status().is_ignored())
            {
                let path = String::from_utf8_lossy(entry.path_bytes()).into_owned();
                return Err(GitError::LocalChanges(path));
            }
        }
        if self.origin()?.is_some()
            && let Some(local) = self.target_of(LOCAL_REF)?
        {
            let pushed = match self.target_of(TRACKING_REF)? {
                Some(remote) => {
                    remote == local
                        || self
                            .repo
                            .graph_descendant_of(remote, local)
                            .map_err(|e| map_error(&e))?
                }
                None => false,
            };
            if !pushed {
                return Err(GitError::LocalChanges(LOCAL_REF.to_owned()));
            }
        }
        Ok(())
    }

    fn create(path: &Path, bare: bool, credentials: Credentials) -> Result<Self, GitError> {
        let mut options = RepositoryInitOptions::new();
        options
            .bare(bare)
            .initial_head(BRANCH)
            .no_reinit(true)
            .mkpath(true);
        let repo = Repository::init_opts(path, &options).map_err(|e| map_error(&e))?;
        Ok(Self { repo, credentials })
    }

    fn clone_into(
        url: &str,
        path: &Path,
        credentials: Credentials,
        bare: bool,
    ) -> Result<Self, GitError> {
        let existed = path.exists();
        if existed {
            let mut entries = std::fs::read_dir(path).map_err(|e| io_error(&e))?;
            if entries.next().is_some() {
                return Err(GitError::Other(format!(
                    "destination {} is not empty",
                    path.display()
                )));
            }
        }
        let cloned = Self::create(path, bare, credentials).and_then(|mut backend| {
            backend.set_remote(url)?;
            if let Some(head) = backend.fetch()? {
                backend.finish(&head)?;
            }
            Ok(backend)
        });
        if cloned.is_err() {
            // Best effort: leave nothing behind, as `git clone` does.
            if existed {
                if let Ok(entries) = std::fs::read_dir(path) {
                    for entry in entries.flatten() {
                        let p = entry.path();
                        let _ = if p.is_dir() {
                            std::fs::remove_dir_all(&p)
                        } else {
                            std::fs::remove_file(&p)
                        };
                    }
                }
            } else {
                let _ = std::fs::remove_dir_all(path);
            }
        }
        cloned
    }

    /// The `origin` remote, or `None` in local mode.
    fn origin(&self) -> Result<Option<Remote<'_>>, GitError> {
        match self.repo.find_remote(REMOTE) {
            Ok(remote) => Ok(Some(remote)),
            Err(e) if e.code() == ErrorCode::NotFound => Ok(None),
            Err(e) => Err(map_error(&e)),
        }
    }

    /// The commit a ref points to, or `None` if the ref does not exist.
    fn target_of(&self, name: &str) -> Result<Option<Oid>, GitError> {
        match self.repo.find_reference(name) {
            Ok(reference) => reference
                .resolve()
                .map(|r| r.target())
                .map_err(|e| map_error(&e)),
            Err(e) if e.code() == ErrorCode::NotFound => Ok(None),
            Err(e) => Err(map_error(&e)),
        }
    }

    fn callbacks(&self) -> RemoteCallbacks<'_> {
        let mut callbacks = RemoteCallbacks::new();
        let mut auth = Authenticator::new(&self.credentials, self.repo.config().ok());
        callbacks.credentials(move |url, username, allowed| auth.next(url, username, allowed));
        callbacks
    }

    /// Moves `main` to `target` while holding its lock, and the working tree
    /// with it if HEAD is on `main`. With `expected`, `main` must currently
    /// point there (`None`: must not exist) and not be locked by another
    /// process; returns `false` if it does not.
    fn move_main(&self, target: Oid, expected: Option<Option<Oid>>) -> Result<bool, GitError> {
        let mut transaction = self.repo.transaction().map_err(|e| map_error(&e))?;
        if let Err(e) = transaction.lock_ref(LOCAL_REF) {
            // Another process holds the lock: it is moving `main` right now.
            return if e.code() == ErrorCode::Locked && expected.is_some() {
                Ok(false)
            } else {
                Err(map_error(&e))
            };
        }
        let current = self.target_of(LOCAL_REF)?;
        if expected.is_some_and(|expected| expected != current) {
            return Ok(false);
        }
        if current != Some(target) {
            self.check_out(target)?;
            transaction
                .set_target(LOCAL_REF, target, None, REFLOG)
                .map_err(|e| map_error(&e))?;
            transaction.commit().map_err(|e| map_error(&e))?;
        }
        Ok(true)
    }

    /// Checks out `target` if there is a working tree and HEAD is on `main`.
    ///
    /// Must run before `main` moves: the safe strategy compares the working
    /// tree with HEAD's tree and refuses to overwrite uncommitted changes.
    fn check_out(&self, target: Oid) -> Result<(), GitError> {
        if self.repo.is_bare() {
            return Ok(());
        }
        let head = self
            .repo
            .find_reference("HEAD")
            .map_err(|e| map_error(&e))?;
        if head.symbolic_target_bytes() != Some(LOCAL_REF.as_bytes()) {
            return Ok(());
        }
        let tree = self
            .repo
            .find_commit(target)
            .and_then(|c| c.tree())
            .map_err(|e| map_error(&e))?;
        let mut checkout = CheckoutBuilder::new();
        checkout.safe();
        self.repo
            .checkout_tree(tree.as_object(), Some(&mut checkout))
            .map_err(|e| match e.code() {
                ErrorCode::Conflict => GitError::LocalChanges(redact(e.message())),
                _ => map_error(&e),
            })
    }

    fn push_local(&self, commit: Oid) -> Result<PushOutcome, GitError> {
        let parent = self
            .repo
            .find_commit(commit)
            .map_err(|e| map_error(&e))?
            .parent_ids()
            .next();
        Ok(if self.move_main(commit, Some(parent))? {
            PushOutcome::Pushed
        } else {
            PushOutcome::Rejected
        })
    }

    fn push_remote(&self, mut remote: Remote<'_>, commit: Oid) -> Result<PushOutcome, GitError> {
        init_tls()?;
        let status: RefCell<Option<String>> = RefCell::new(None);
        let refspec = format!("{commit}:{LOCAL_REF}");
        {
            let mut callbacks = self.callbacks();
            callbacks.push_update_reference(|_, message| {
                if let Some(message) = message {
                    *status.borrow_mut() = Some(message.to_owned());
                }
                Ok(())
            });
            let mut options = PushOptions::new();
            options.remote_callbacks(callbacks);
            match remote.push(&[refspec.as_str()], Some(&mut options)) {
                Ok(()) => {}
                // libgit2 checks fast-forward itself before sending anything.
                Err(e) if e.code() == ErrorCode::NotFastForward => {
                    return Ok(PushOutcome::Rejected);
                }
                Err(e) => return Err(map_error(&e)),
            }
        }
        match status.into_inner() {
            None => Ok(PushOutcome::Pushed),
            Some(message) if is_retryable_rejection(&message) => Ok(PushOutcome::Rejected),
            Some(message) => Err(GitError::Other(redact(&message))),
        }
    }
}

impl GitBackend for Git2Backend {
    fn fetch(&mut self) -> Result<Option<CommitId>, GitError> {
        let Some(mut remote) = self.origin()? else {
            return Ok(self.target_of(LOCAL_REF)?.map(commit_id));
        };
        init_tls()?;
        let mut options = FetchOptions::new();
        options
            .remote_callbacks(self.callbacks())
            .download_tags(AutotagOption::None)
            .update_fetchhead(false);
        remote
            .fetch(&[FETCH_REFSPEC], Some(&mut options), Some(REFLOG_FETCH))
            .map_err(|e| map_error(&e))?;
        // The refs the remote advertised stay readable after the fetch; an
        // empty remote advertises no `main`.
        let head = remote
            .list()
            .map_err(|e| map_error(&e))?
            .iter()
            .find(|h| h.name() == LOCAL_REF)
            .map(git2::RemoteHead::oid);
        Ok(head.map(commit_id))
    }

    fn snapshot(&self, commit: Option<&CommitId>) -> Result<Snapshot, GitError> {
        let mut snapshot = Snapshot::new();
        if let Some(commit) = commit {
            let tree = self
                .repo
                .find_commit(parse_oid(commit)?)
                .and_then(|c| c.tree())
                .map_err(|e| map_error(&e))?;
            read_tree(&self.repo, &tree, "", &mut snapshot).map_err(|e| map_error(&e))?;
        }
        Ok(snapshot)
    }

    fn commit(
        &mut self,
        parent: Option<&CommitId>,
        before: &Snapshot,
        after: &Snapshot,
        message: &str,
        author: &Author,
    ) -> Result<CommitId, GitError> {
        let repo = &self.repo;
        let empty = Snapshot::new();
        let (parent, baseline, before) = match parent {
            Some(id) => {
                let commit = repo
                    .find_commit(parse_oid(id)?)
                    .map_err(|e| map_error(&e))?;
                let tree = commit.tree().map_err(|e| map_error(&e))?;
                (Some(commit), tree, before)
            }
            None => (None, empty_tree(repo).map_err(|e| map_error(&e))?, &empty),
        };

        let mut update = TreeUpdateBuilder::new();
        for change in after.changes_from(before) {
            check_path(change.path())?;
            match change {
                Change::Added(path) | Change::Modified(path) => {
                    let Some(content) = after.get(&path) else {
                        continue;
                    };
                    let blob = repo.blob(content).map_err(|e| map_error(&e))?;
                    update.upsert(path.as_str(), blob, file_mode(&baseline, &path));
                }
                Change::Removed(path) => {
                    update.remove(path.as_str());
                }
            }
        }
        let tree_id = update
            .create_updated(repo, &baseline)
            .map_err(|e| map_error(&e))?;
        let tree = repo.find_tree(tree_id).map_err(|e| map_error(&e))?;

        let signature = Signature::now(&author.name, &author.email).map_err(|e| map_error(&e))?;
        let parents: Vec<&git2::Commit<'_>> = parent.iter().collect();
        let oid = repo
            .commit(None, &signature, &signature, message, &tree, &parents)
            .map_err(|e| map_error(&e))?;
        Ok(commit_id(oid))
    }

    fn push(&mut self, commit: &CommitId) -> Result<PushOutcome, GitError> {
        let oid = parse_oid(commit)?;
        match self.origin()? {
            Some(remote) => self.push_remote(remote, oid),
            None => self.push_local(oid),
        }
    }

    fn finish(&mut self, commit: &CommitId) -> Result<(), GitError> {
        let oid = parse_oid(commit)?;
        if self.origin()?.is_some() {
            self.repo
                .reference(TRACKING_REF, oid, true, REFLOG)
                .map_err(|e| map_error(&e))?;
        }
        self.move_main(oid, None).map(|_| ())
    }
}

/// Answers libgit2's credential requests, trying each source once.
struct Authenticator<'a> {
    credentials: &'a Credentials,
    config: Option<git2::Config>,
    requests: u32,
    tried_agent: bool,
    tried_helper: bool,
    tried_default: bool,
    tried_given: bool,
}

impl<'a> Authenticator<'a> {
    fn new(credentials: &'a Credentials, config: Option<git2::Config>) -> Self {
        Self {
            credentials,
            config,
            requests: 0,
            tried_agent: false,
            tried_helper: false,
            tried_default: false,
            tried_given: false,
        }
    }

    fn next(
        &mut self,
        url: &str,
        url_username: Option<&str>,
        allowed: CredentialType,
    ) -> Result<Cred, git2::Error> {
        self.requests += 1;
        if self.requests > MAX_CREDENTIAL_REQUESTS {
            return Err(auth_failed());
        }
        let ssh_user = match self.credentials {
            Credentials::SshKey { username, .. } => username.as_str(),
            _ => url_username.unwrap_or("git"),
        };
        if allowed.contains(CredentialType::USERNAME) {
            // SSH asks for the user first when the URL names none.
            return Cred::username(ssh_user);
        }
        match self.credentials {
            Credentials::Default => {
                if allowed.contains(CredentialType::SSH_KEY) && !self.tried_agent {
                    self.tried_agent = true;
                    return Cred::ssh_key_from_agent(ssh_user);
                }
                if allowed.contains(CredentialType::USER_PASS_PLAINTEXT) && !self.tried_helper {
                    self.tried_helper = true;
                    if let Some(config) = &self.config
                        && let Ok(cred) = Cred::credential_helper(config, url, url_username)
                    {
                        return Ok(cred);
                    }
                }
                if allowed.contains(CredentialType::DEFAULT) && !self.tried_default {
                    self.tried_default = true;
                    return Cred::default();
                }
            }
            Credentials::UserPassword { username, password } => {
                if allowed.contains(CredentialType::USER_PASS_PLAINTEXT) && !self.tried_given {
                    self.tried_given = true;
                    return Cred::userpass_plaintext(username, password);
                }
            }
            Credentials::SshKey {
                private_key,
                passphrase,
                ..
            } => {
                if allowed.intersects(CredentialType::SSH_MEMORY | CredentialType::SSH_KEY)
                    && !self.tried_given
                {
                    self.tried_given = true;
                    return Cred::ssh_key_from_memory(
                        ssh_user,
                        None,
                        private_key,
                        passphrase.as_deref(),
                    );
                }
            }
        }
        Err(auth_failed())
    }
}

fn auth_failed() -> git2::Error {
    git2::Error::new(
        ErrorCode::Auth,
        ErrorClass::Callback,
        "no further credentials to try",
    )
}

/// A PEM bundle of trusted CA certificates, set by [`use_ca_bundle`].
static CA_BUNDLE: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();

/// Makes every later fetch and push trust the certificates in the PEM file at
/// `path` instead of OpenSSL's defaults. The Android app exports the system's
/// trusted certificates to such a file at start, because the vendored OpenSSL
/// cannot read Android's CA directories. Only the first call takes effect.
pub fn use_ca_bundle(path: &Path) {
    let _ = CA_BUNDLE.set(path.to_path_buf());
}

/// Applies the TLS settings once per process: the CA bundle from
/// [`use_ca_bundle`] if set, otherwise on Android the system CA directories.
fn init_tls() -> Result<(), GitError> {
    use std::sync::OnceLock;
    static INIT: OnceLock<Result<(), GitError>> = OnceLock::new();
    INIT.get_or_init(|| {
        if let Some(bundle) = CA_BUNDLE.get() {
            // SAFETY: git2 marks this unsafe because libgit2 changes its global
            // TLS settings without synchronization. Every network operation of
            // this crate calls `init_tls` first, and `OnceLock` blocks
            // concurrent callers until the setting is made, so no fetch or push
            // of this crate runs while it changes.
            return unsafe { git2::opts::set_ssl_cert_file(bundle) }.map_err(|e| map_error(&e));
        }
        system_ca_dirs()
    })
    .clone()
}

/// Points OpenSSL at Android's CA directories: the vendored OpenSSL has no CA
/// bundle of its own there. May not match OpenSSL 3's hash names; the app
/// therefore sets a bundle with [`use_ca_bundle`].
#[cfg(target_os = "android")]
fn system_ca_dirs() -> Result<(), GitError> {
    /// Updatable store (Android 14+) first, then the system image's.
    const CA_DIRS: &str = "/apex/com.android.conscrypt/cacerts:/system/etc/security/cacerts";
    // SAFETY: as in `init_tls`, which is the only caller.
    unsafe { git2::opts::set_ssl_cert_dir(CA_DIRS) }.map_err(|e| map_error(&e))
}

#[cfg(not(target_os = "android"))]
fn system_ca_dirs() -> Result<(), GitError> {
    Ok(())
}

/// Reads regular files below `tree` into `snapshot`. Symlinks, submodules
/// and names that are not UTF-8 lie outside the snapshot model and are
/// skipped; commits keep them, because they rewrite only changed paths.
fn read_tree(
    repo: &Repository,
    tree: &Tree<'_>,
    prefix: &str,
    snapshot: &mut Snapshot,
) -> Result<(), git2::Error> {
    for entry in tree {
        let Ok(name) = std::str::from_utf8(entry.name_bytes()) else {
            continue;
        };
        let path = if prefix.is_empty() {
            name.to_owned()
        } else {
            format!("{prefix}/{name}")
        };
        match entry.kind() {
            Some(ObjectType::Tree) => {
                read_tree(repo, &repo.find_tree(entry.id())?, &path, snapshot)?;
            }
            Some(ObjectType::Blob) if entry.filemode() != i32::from(FileMode::Link) => {
                snapshot.insert(path, repo.find_blob(entry.id())?.content());
            }
            _ => {}
        }
    }
    Ok(())
}

fn empty_tree(repo: &Repository) -> Result<Tree<'_>, git2::Error> {
    let oid = repo.treebuilder(None)?.write()?;
    repo.find_tree(oid)
}

/// The mode for a written file: executable files stay executable.
fn file_mode(baseline: &Tree<'_>, path: &str) -> FileMode {
    match baseline.get_path(Path::new(path)) {
        Ok(entry) if entry.filemode() == i32::from(FileMode::BlobExecutable) => {
            FileMode::BlobExecutable
        }
        _ => FileMode::Blob,
    }
}

/// Refuses paths that do not name a file inside the repository.
fn check_path(path: &str) -> Result<(), GitError> {
    let valid = path
        .split('/')
        .all(|s| !s.is_empty() && s != "." && s != ".." && !s.eq_ignore_ascii_case(".git"));
    if valid {
        Ok(())
    } else {
        Err(GitError::Other(format!("invalid path {path:?}")))
    }
}

/// Whether a push status from the remote means "someone pushed first".
fn is_retryable_rejection(status: &str) -> bool {
    const MARKERS: [&str; 7] = [
        "non-fast-forward",
        "fetch first",
        "failed to update ref",
        "failed to lock",
        "cannot lock",
        "stale info",
        "already exists",
    ];
    let status = status.to_ascii_lowercase();
    MARKERS.iter().any(|m| status.contains(m))
}

fn commit_id(oid: Oid) -> CommitId {
    CommitId(oid.to_string())
}

fn parse_oid(commit: &CommitId) -> Result<Oid, GitError> {
    Oid::from_str(&commit.0).map_err(|_| GitError::NotFound(commit.0.clone()))
}

fn io_error(error: &std::io::Error) -> GitError {
    GitError::Other(error.to_string())
}

fn map_error(error: &git2::Error) -> GitError {
    let message = redact(error.message());
    match (error.code(), error.class()) {
        (ErrorCode::Auth, _) => GitError::Auth,
        (ErrorCode::Certificate, _)
        | (_, ErrorClass::Net | ErrorClass::Ssl | ErrorClass::Ssh | ErrorClass::Http) => {
            GitError::Network(message)
        }
        (ErrorCode::NotFound, _) => GitError::NotFound(message),
        _ => GitError::Other(message),
    }
}

/// Removes passwords and tokens embedded in URLs (`https://user:token@host`)
/// from libgit2 messages, which may quote the remote URL.
/// Replaces credentials embedded in URLs (`https://user:token@host`) with `***`.
#[must_use]
pub fn redact(message: &str) -> String {
    static USERINFO: LazyLock<Regex> = LazyLock::new(|| {
        #[allow(clippy::unwrap_used)] // A constant pattern.
        Regex::new(r"(?i)\b([a-z][a-z0-9+.-]*://)[^/@\s]+@").unwrap()
    });
    USERINFO.replace_all(message, "${1}***@").into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refs_name_the_main_branch() {
        assert_eq!(LOCAL_REF, format!("refs/heads/{BRANCH}"));
        assert_eq!(TRACKING_REF, format!("refs/remotes/{REMOTE}/{BRANCH}"));
        assert_eq!(FETCH_REFSPEC, format!("+{LOCAL_REF}:{TRACKING_REF}"));
    }

    #[test]
    fn redacts_userinfo_in_urls() {
        assert_eq!(
            redact("failed to connect to https://tim:ghp_secret@example.org/ledger.git: 401"),
            "failed to connect to https://***@example.org/ledger.git: 401"
        );
        assert_eq!(
            redact("ssh://git@example.org/x and http://a:b@c/d"),
            "ssh://***@example.org/x and http://***@c/d"
        );
        assert_eq!(redact("no url here"), "no url here");
    }

    #[test]
    fn only_lost_races_are_retried() {
        assert!(is_retryable_rejection("non-fast-forward"));
        assert!(is_retryable_rejection("failed to update ref"));
        assert!(is_retryable_rejection(
            "failed to write reference 'refs/heads/main': a reference with that name already exists."
        ));
        assert!(!is_retryable_rejection("pre-receive hook declined"));
        assert!(!is_retryable_rejection("protected branch hook declined"));
    }

    #[test]
    fn paths_stay_inside_the_repository() {
        assert!(check_path("manual/k7p2q9xd/2026-10.beancount").is_ok());
        assert!(check_path(".gitignore").is_ok());
        for bad in [
            "",
            "/abs",
            "a//b",
            "a/",
            "../x",
            "a/./b",
            ".git/config",
            "x/.GIT/y",
        ] {
            assert!(check_path(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn errors_map_to_codes() {
        let auth = git2::Error::new(ErrorCode::Auth, ErrorClass::Http, "401");
        assert_eq!(map_error(&auth), GitError::Auth);
        let net = git2::Error::new(ErrorCode::GenericError, ErrorClass::Net, "unreachable");
        assert_eq!(map_error(&net), GitError::Network("unreachable".into()));
        let ssl = git2::Error::new(ErrorCode::Certificate, ErrorClass::Ssl, "bad cert");
        assert!(matches!(map_error(&ssl), GitError::Network(_)));
        let missing = git2::Error::new(ErrorCode::NotFound, ErrorClass::Repository, "gone");
        assert_eq!(map_error(&missing), GitError::NotFound("gone".into()));
        let other = git2::Error::new(ErrorCode::Locked, ErrorClass::Reference, "locked");
        assert_eq!(map_error(&other), GitError::Other("locked".into()));
    }

    #[test]
    fn credentials_are_tried_once_and_then_given_up() {
        let credentials = Credentials::UserPassword {
            username: "u".into(),
            password: "p".into(),
        };
        let mut auth = Authenticator::new(&credentials, None);
        let allowed = CredentialType::USER_PASS_PLAINTEXT;
        assert!(auth.next("https://example.org/x", None, allowed).is_ok());
        let Err(err) = auth.next("https://example.org/x", None, allowed) else {
            panic!("credentials offered twice");
        };
        assert_eq!(map_error(&err), GitError::Auth);
    }

    #[test]
    fn credential_requests_are_bounded() {
        let credentials = Credentials::Default;
        let mut auth = Authenticator::new(&credentials, None);
        // SSH keeps asking for a user name; the loop must end.
        let results: Vec<_> = (0..10)
            .map(|_| {
                auth.next("ssh://h/x", None, CredentialType::USERNAME)
                    .is_ok()
            })
            .collect();
        assert_eq!(results.iter().filter(|ok| **ok).count(), 4);
    }

    #[test]
    fn ssh_key_uses_the_configured_user() {
        let credentials = Credentials::SshKey {
            username: "git".into(),
            private_key: "not a key".into(),
            passphrase: None,
        };
        let mut auth = Authenticator::new(&credentials, None);
        let cred = auth
            .next("ssh://other@h/x", Some("other"), CredentialType::USERNAME)
            .unwrap();
        assert!(cred.has_username());
        // The key itself is only offered once.
        let _ = auth.next("ssh://h/x", None, CredentialType::SSH_MEMORY);
        assert!(
            auth.next("ssh://h/x", None, CredentialType::SSH_MEMORY)
                .is_err()
        );
    }
}
