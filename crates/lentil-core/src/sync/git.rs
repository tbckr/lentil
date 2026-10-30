//! [`GitBackend`] on libgit2.

use std::path::Path;

use super::{Author, CommitId, Credentials, GitBackend, GitError, PushOutcome};
use crate::snapshot::Snapshot;

/// A local clone driven through libgit2.
///
/// With a remote named `origin`, `fetch` and `push` talk to it. Without one
/// (a repository that only lives locally), `fetch` reads the local `main` and
/// `push` moves it.
pub struct Git2Backend {
    // Fields defined in task D.
}

impl std::fmt::Debug for Git2Backend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Git2Backend")
    }
}

impl Git2Backend {
    /// Opens an existing clone at `path`.
    ///
    /// # Errors
    ///
    /// Returns a [`GitError`] if `path` is not a Git repository.
    pub fn open(path: &Path, credentials: Credentials) -> Result<Self, GitError> {
        let _ = (path, credentials);
        todo!("task D")
    }

    /// Clones `url` into `path` and checks out `main`.
    ///
    /// # Errors
    ///
    /// Returns a [`GitError`] if cloning fails.
    pub fn clone_from(url: &str, path: &Path, credentials: Credentials) -> Result<Self, GitError> {
        let _ = (url, path, credentials);
        todo!("task D")
    }

    /// Creates a new repository at `path` with `main` as its initial branch.
    ///
    /// # Errors
    ///
    /// Returns a [`GitError`] if the repository cannot be created.
    pub fn init(path: &Path) -> Result<Self, GitError> {
        let _ = path;
        todo!("task D")
    }

    /// Sets `origin` to `url`.
    ///
    /// # Errors
    ///
    /// Returns a [`GitError`] if the remote cannot be configured.
    pub fn set_remote(&mut self, url: &str) -> Result<(), GitError> {
        let _ = url;
        todo!("task D")
    }

    /// The author from the repository's Git configuration (`user.name`, `user.email`).
    #[must_use]
    pub fn configured_author(&self) -> Option<Author> {
        todo!("task D")
    }

    /// Fails with [`GitError::LocalChanges`] if the working tree has
    /// uncommitted changes to tracked files or local commits not on the
    /// remote's `main`, which a sync would otherwise discard.
    ///
    /// # Errors
    ///
    /// See above.
    pub fn ensure_no_local_changes(&self) -> Result<(), GitError> {
        todo!("task D")
    }
}

impl GitBackend for Git2Backend {
    fn fetch(&mut self) -> Result<Option<CommitId>, GitError> {
        todo!("task D")
    }

    fn snapshot(&self, commit: Option<&CommitId>) -> Result<Snapshot, GitError> {
        let _ = commit;
        todo!("task D")
    }

    fn commit(
        &mut self,
        parent: Option<&CommitId>,
        before: &Snapshot,
        after: &Snapshot,
        message: &str,
        author: &Author,
    ) -> Result<CommitId, GitError> {
        let _ = (parent, before, after, message, author);
        todo!("task D")
    }

    fn push(&mut self, commit: &CommitId) -> Result<PushOutcome, GitError> {
        let _ = commit;
        todo!("task D")
    }

    fn finish(&mut self, commit: &CommitId) -> Result<(), GitError> {
        let _ = commit;
        todo!("task D")
    }
}
