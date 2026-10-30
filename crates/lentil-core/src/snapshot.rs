//! An in-memory copy of a repository's files.
//!
//! Everything the core computes works on a [`Snapshot`]: it is read from a Git
//! tree or a directory, edited by replaying intents, validated against the
//! snapshot it came from and finally written back as a commit. Paths are
//! relative to the repository root and always use `/`.

use std::collections::BTreeMap;
use std::io;
use std::path::Path;

/// The files of one repository state, keyed by repository-relative path.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Snapshot {
    files: BTreeMap<String, Vec<u8>>,
}

/// One file that differs between two snapshots.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Change {
    /// The file exists only in the newer snapshot.
    Added(String),
    /// The file exists in both snapshots with different content.
    Modified(String),
    /// The file exists only in the older snapshot.
    Removed(String),
}

impl Change {
    /// The repository-relative path of the changed file.
    #[must_use]
    pub fn path(&self) -> &str {
        match self {
            Self::Added(p) | Self::Modified(p) | Self::Removed(p) => p,
        }
    }
}

impl Snapshot {
    /// An empty snapshot.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// A snapshot holding the given files.
    pub fn from_files<P, C, I>(files: I) -> Self
    where
        P: Into<String>,
        C: Into<Vec<u8>>,
        I: IntoIterator<Item = (P, C)>,
    {
        let mut snapshot = Self::new();
        for (path, content) in files {
            snapshot.insert(path, content);
        }
        snapshot
    }

    /// Reads every file below `root`.
    ///
    /// Hidden directories such as `.git` are skipped, hidden files such as
    /// `.gitignore` are kept, and load caches (`.*.cache`, `.*.picklecache`)
    /// are skipped because they never belong in the repository.
    ///
    /// # Errors
    ///
    /// Returns any I/O error from walking the directory or reading a file.
    pub fn from_dir(root: &Path) -> io::Result<Self> {
        let mut snapshot = Self::new();
        read_dir_into(root, "", &mut snapshot)?;
        Ok(snapshot)
    }

    /// The content of `path`, if the file exists.
    #[must_use]
    pub fn get(&self, path: &str) -> Option<&[u8]> {
        self.files.get(path).map(Vec::as_slice)
    }

    /// The content of `path` as UTF-8 text, if the file exists and is valid UTF-8.
    #[must_use]
    pub fn text(&self, path: &str) -> Option<&str> {
        self.get(path).and_then(|b| std::str::from_utf8(b).ok())
    }

    /// Whether `path` exists.
    #[must_use]
    pub fn contains(&self, path: &str) -> bool {
        self.files.contains_key(path)
    }

    /// Writes `content` to `path`, replacing any previous content.
    pub fn insert(&mut self, path: impl Into<String>, content: impl Into<Vec<u8>>) {
        self.files.insert(path.into(), content.into());
    }

    /// Removes `path` and returns its previous content.
    pub fn remove(&mut self, path: &str) -> Option<Vec<u8>> {
        self.files.remove(path)
    }

    /// All paths in sorted order.
    pub fn paths(&self) -> impl Iterator<Item = &str> {
        self.files.keys().map(String::as_str)
    }

    /// All files in path order.
    pub fn files(&self) -> impl Iterator<Item = (&str, &[u8])> {
        self.files.iter().map(|(p, c)| (p.as_str(), c.as_slice()))
    }

    /// Number of files.
    #[must_use]
    pub fn len(&self) -> usize {
        self.files.len()
    }

    /// Whether the snapshot holds no files.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    /// The files that differ from `before`, in path order.
    #[must_use]
    pub fn changes_from(&self, before: &Snapshot) -> Vec<Change> {
        let mut changes = Vec::new();
        for (path, content) in &self.files {
            match before.files.get(path) {
                None => changes.push(Change::Added(path.clone())),
                Some(old) if old != content => changes.push(Change::Modified(path.clone())),
                Some(_) => {}
            }
        }
        for path in before.files.keys() {
            if !self.files.contains_key(path) {
                changes.push(Change::Removed(path.clone()));
            }
        }
        changes.sort();
        changes
    }

    /// Paths matching an include pattern (spec section 3), in sorted order.
    #[must_use]
    pub fn glob(&self, pattern: &str) -> Vec<&str> {
        self.paths().filter(|p| glob_match(pattern, p)).collect()
    }
}

/// Whether `path` matches the include `pattern`.
///
/// Patterns follow spec section 3: segments separated by `/`, at most one `*`
/// per segment, and a `*` never crosses a `/`. A `*` does not match a leading
/// dot, like shell globs.
#[must_use]
pub fn glob_match(pattern: &str, path: &str) -> bool {
    let pattern_segments: Vec<&str> = pattern.split('/').collect();
    let path_segments: Vec<&str> = path.split('/').collect();
    pattern_segments.len() == path_segments.len()
        && pattern_segments
            .iter()
            .zip(&path_segments)
            .all(|(pat, seg)| segment_match(pat, seg))
}

fn segment_match(pattern: &str, segment: &str) -> bool {
    match pattern.split_once('*') {
        None => pattern == segment,
        Some((prefix, suffix)) => {
            if prefix.is_empty() && segment.starts_with('.') {
                return false;
            }
            segment.len() >= prefix.len() + suffix.len()
                && segment.starts_with(prefix)
                && segment.ends_with(suffix)
        }
    }
}

/// Whether a file name is a load cache of rustledger or Python Beancount.
#[must_use]
pub fn is_load_cache(name: &str) -> bool {
    name.starts_with('.') && (name.ends_with(".cache") || name.ends_with(".picklecache"))
}

fn read_dir_into(dir: &Path, prefix: &str, snapshot: &mut Snapshot) -> io::Result<()> {
    let mut entries = std::fs::read_dir(dir)?.collect::<Result<Vec<_>, _>>()?;
    entries.sort_by_key(std::fs::DirEntry::file_name);
    for entry in entries {
        let name = entry.file_name().to_string_lossy().into_owned();
        let path = if prefix.is_empty() {
            name.clone()
        } else {
            format!("{prefix}/{name}")
        };
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            if !name.starts_with('.') {
                read_dir_into(&entry.path(), &path, snapshot)?;
            }
        } else if file_type.is_file() && !is_load_cache(&name) {
            snapshot.insert(path, std::fs::read(entry.path())?);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn star_matches_within_one_segment_only() {
        assert!(glob_match(
            "imports/*/*/*.beancount",
            "imports/giro/2026/20261020T071502Z.beancount"
        ));
        assert!(!glob_match(
            "imports/*/*/*.beancount",
            "imports/giro/2026/x/y.beancount"
        ));
        assert!(!glob_match(
            "imports/*/*/*.beancount",
            "imports/giro/y.beancount"
        ));
        assert!(!glob_match("manual/*/*.beancount", "manual/dev/a.txt"));
        assert!(glob_match("main.beancount", "main.beancount"));
    }

    #[test]
    fn star_skips_hidden_names() {
        assert!(!glob_match(
            "manual/*/*.beancount",
            "manual/dev/.x.beancount"
        ));
        assert!(glob_match(
            "manual/*/.*.beancount",
            "manual/dev/.x.beancount"
        ));
    }

    #[test]
    fn changes_are_sorted_and_complete() {
        let before = Snapshot::from_files([("a", "1"), ("b", "2"), ("c", "3")]);
        let after = Snapshot::from_files([("a", "1"), ("b", "x"), ("d", "4")]);
        assert_eq!(
            after.changes_from(&before),
            vec![
                Change::Added("d".into()),
                Change::Modified("b".into()),
                Change::Removed("c".into()),
            ]
        );
    }

    #[test]
    fn from_dir_skips_hidden_dirs_and_caches() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".git/objects")).unwrap();
        std::fs::create_dir_all(dir.path().join("imports/giro")).unwrap();
        std::fs::write(dir.path().join(".git/HEAD"), "x").unwrap();
        std::fs::write(dir.path().join(".gitignore"), "x").unwrap();
        std::fs::write(dir.path().join(".main.beancount.cache"), "x").unwrap();
        std::fs::write(dir.path().join("imports/giro/a.beancount"), "y").unwrap();
        let snapshot = Snapshot::from_dir(dir.path()).unwrap();
        assert_eq!(
            snapshot.paths().collect::<Vec<_>>(),
            vec![".gitignore", "imports/giro/a.beancount"]
        );
    }
}
