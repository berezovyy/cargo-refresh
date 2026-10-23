//! Content snapshots are compared to the last acknowledged (applied) version.
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    fs, io,
    path::{Path, PathBuf},
    sync::Arc,
};

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PathKind {
    Rs,
    Manifest,
    Other,
}

pub fn classify(path: &Path, build_inputs: &BTreeSet<PathBuf>) -> PathKind {
    if build_inputs.contains(path)
        || matches!(
            path.file_name().and_then(|p| p.to_str()),
            Some("Cargo.toml" | "Cargo.lock" | "build.rs")
        )
        || (path.parent().is_some_and(|p| p.ends_with(".cargo"))
            && matches!(
                path.file_name().and_then(|p| p.to_str()),
                Some("config.toml" | "config")
            ))
    {
        PathKind::Manifest
    } else if path.extension().is_some_and(|ext| ext == "rs") {
        PathKind::Rs
    } else {
        PathKind::Other
    }
}

#[derive(Debug, Clone)]
pub struct FileChange {
    pub path: PathBuf,
    pub kind: PathKind,
    /// None means deletion. Consumers compile exactly these bytes, then acknowledge this snapshot.
    pub content: Option<Arc<[u8]>>,
    hash: Option<blake3::Hash>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Disposition {
    Skip,
    Changed,
}

#[derive(Debug, Clone)]
pub struct ChangeSet {
    pub revision: u64,
    pub disposition: Disposition,
    pub changes: Vec<FileChange>,
}

pub struct ChangeTracker {
    applied: HashMap<PathBuf, blake3::Hash>,
    pending: BTreeMap<PathBuf, FileChange>,
    unread: BTreeSet<PathBuf>,
    build_inputs: BTreeSet<PathBuf>,
    revision: u64,
}

impl ChangeTracker {
    pub fn new(
        paths: impl IntoIterator<Item = PathBuf>,
        build_inputs: BTreeSet<PathBuf>,
    ) -> io::Result<Self> {
        let mut tracker = Self {
            applied: HashMap::new(),
            pending: BTreeMap::new(),
            unread: BTreeSet::new(),
            build_inputs,
            revision: 0,
        };
        for path in paths {
            if let Some(hash) = read_snapshot(&path)?.1 {
                tracker.applied.insert(path, hash);
            }
        }
        Ok(tracker)
    }

    /// A runtime rollback changes the applied baseline without touching editor files.
    pub fn restore_applied(&mut self, paths: &[(PathBuf, Option<Arc<[u8]>>)]) -> io::Result<()> {
        for (path, content) in paths {
            if let Some(content) = content {
                self.applied.insert(path.clone(), blake3::hash(content));
            } else {
                self.applied.remove(path);
            }
            self.unread.insert(path.clone());
        }
        // Keep the editor's still-invalid bytes pending after runtime revert.
        // Do not manufacture a save/revision or automatically reapply them.
        self.refresh_pending()
    }

    pub fn set_build_inputs(&mut self, inputs: BTreeSet<PathBuf>) {
        self.build_inputs = inputs;
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn is_current(&self, revision: u64) -> bool {
        self.revision == revision
    }

    /// Only named paths are read. Pending edits from earlier unacknowledged revisions are retained.
    pub fn observe(&mut self, paths: impl IntoIterator<Item = PathBuf>) -> io::Result<ChangeSet> {
        // Fence old work even if a new source cannot be read.
        self.revision = self
            .revision
            .checked_add(1)
            .expect("source revision exhausted");
        self.unread.extend(paths);
        self.refresh_pending()?;
        Ok(ChangeSet {
            revision: self.revision,
            disposition: if self.pending.is_empty() {
                Disposition::Skip
            } else {
                Disposition::Changed
            },
            changes: self.pending.values().cloned().collect(),
        })
    }

    fn refresh_pending(&mut self) -> io::Result<()> {
        let snapshots: io::Result<Vec<_>> = self
            .unread
            .iter()
            .cloned()
            .map(|path| {
                let (content, hash) = read_snapshot(&path)?;
                Ok(FileChange {
                    kind: classify(&path, &self.build_inputs),
                    path,
                    content,
                    hash,
                })
            })
            .collect();
        for snapshot in snapshots? {
            if self.applied.get(&snapshot.path).copied() == snapshot.hash {
                self.pending.remove(&snapshot.path);
            } else {
                self.pending.insert(snapshot.path.clone(), snapshot);
            }
        }
        self.unread.clear();
        Ok(())
    }

    /// Overflow recovery must also revisit known paths that have disappeared since the last scan.
    pub fn rehash_all(
        &mut self,
        paths: impl IntoIterator<Item = PathBuf>,
    ) -> io::Result<ChangeSet> {
        let paths: BTreeSet<_> = paths
            .into_iter()
            .chain(self.applied.keys().cloned())
            .chain(self.pending.keys().cloned())
            .collect();
        self.observe(paths)
    }

    /// A failed/superseded operation must never advance the applied baseline.
    /// The caller must drain queued watcher input before accepting long-running work.
    pub fn mark_applied(&mut self, changes: &ChangeSet) -> bool {
        if !self.is_current(changes.revision) {
            return false;
        }
        for change in &changes.changes {
            if let Some(hash) = change.hash {
                self.applied.insert(change.path.clone(), hash);
            } else {
                self.applied.remove(&change.path);
            }
        }
        self.pending.clear();
        true
    }
}

type Snapshot = (Option<Arc<[u8]>>, Option<blake3::Hash>);

fn read_snapshot(path: &Path) -> io::Result<Snapshot> {
    match fs::read(path) {
        Ok(bytes) => {
            let hash = blake3::hash(&bytes);
            Ok((Some(bytes.into()), Some(hash)))
        }
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::NotFound
                    | io::ErrorKind::NotADirectory
                    | io::ErrorKind::IsADirectory
            ) =>
        {
            Ok((None, None))
        }
        Err(error) => Err(io::Error::new(
            error.kind(),
            format!("{}: {error}", path.display()),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unchanged_rewrites_skip_and_revisions_increase() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.rs");
        fs::write(&path, "old").unwrap();
        let mut tracker = ChangeTracker::new([path.clone()], BTreeSet::new()).unwrap();
        for expected in 1..=3 {
            fs::write(&path, "old").unwrap();
            let change = tracker.observe([path.clone()]).unwrap();
            assert_eq!(change.revision, expected);
            assert_eq!(change.disposition, Disposition::Skip);
            assert!(change.changes.is_empty());
        }
    }

    #[test]
    fn only_acknowledged_hashes_become_the_baseline() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.rs");
        fs::write(&path, "old").unwrap();
        let mut tracker = ChangeTracker::new([path.clone()], BTreeSet::new()).unwrap();
        fs::write(&path, "new").unwrap();
        let first = tracker.observe([path.clone()]).unwrap();
        assert_eq!(first.disposition, Disposition::Changed);
        let second = tracker.observe([path.clone()]).unwrap();
        assert_eq!(second.disposition, Disposition::Changed);
        assert!(!tracker.is_current(first.revision));
        assert!(!tracker.mark_applied(&first));
        assert!(tracker.mark_applied(&second));
        assert_eq!(
            tracker.observe([path]).unwrap().disposition,
            Disposition::Skip
        );
    }

    #[test]
    fn snapshots_survive_a_later_write_and_pending_paths_are_retained() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.rs");
        let b = dir.path().join("b.rs");
        fs::write(&a, "old").unwrap();
        fs::write(&b, "old").unwrap();
        let mut tracker = ChangeTracker::new([a.clone(), b.clone()], BTreeSet::new()).unwrap();
        fs::write(&a, "first").unwrap();
        let first = tracker.observe([a.clone()]).unwrap();
        fs::write(&a, "later").unwrap();
        assert_eq!(
            first.changes[0].content.as_deref(),
            Some(b"first".as_slice())
        );
        fs::write(&b, "new").unwrap();
        let second = tracker.observe([b]).unwrap();
        assert_eq!(second.changes.len(), 2);
        // A rehash captures the unseen write before acknowledging a compiler result.
        let final_set = tracker.rehash_all([a]).unwrap();
        assert_eq!(final_set.changes.len(), 2);
        assert!(!tracker.mark_applied(&second));
        assert!(tracker.mark_applied(&final_set));
    }

    #[test]
    fn round_trip_to_applied_content_skips_without_acknowledging_failed_work() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.rs");
        fs::write(&path, "old").unwrap();
        let mut tracker = ChangeTracker::new([path.clone()], BTreeSet::new()).unwrap();
        fs::write(&path, "new").unwrap();
        let pending = tracker.observe([path.clone()]).unwrap();
        fs::write(&path, "old").unwrap();
        let reverted = tracker.observe([path]).unwrap();
        assert_eq!(reverted.disposition, Disposition::Skip);
        assert!(!tracker.mark_applied(&pending));
    }

    #[test]
    fn overflow_rehash_finds_additions_changes_and_deletions() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.rs");
        let b = dir.path().join("b.rs");
        let c = dir.path().join("c.rs");
        fs::write(&a, "old").unwrap();
        fs::write(&b, "old").unwrap();
        let mut tracker = ChangeTracker::new([a.clone(), b.clone()], BTreeSet::new()).unwrap();
        fs::remove_file(&a).unwrap();
        fs::write(&b, "new").unwrap();
        fs::write(&c, "added").unwrap();
        let set = tracker.rehash_all([b, c]).unwrap();
        assert_eq!(set.changes.len(), 3);
        assert_eq!(
            set.changes
                .iter()
                .find(|change| change.path == a)
                .unwrap()
                .content,
            None
        );
        assert!(tracker.mark_applied(&set));
        assert_eq!(
            tracker.rehash_all([]).unwrap().disposition,
            Disposition::Skip
        );
    }

    #[test]
    fn unreadable_new_input_fences_old_work_and_is_retried_before_other_edits() {
        use std::os::unix::fs::symlink;
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.rs");
        let b = dir.path().join("b.rs");
        fs::write(&a, "initial").unwrap();
        fs::write(&b, "initial").unwrap();
        let mut tracker = ChangeTracker::new([a.clone(), b.clone()], BTreeSet::new()).unwrap();
        fs::write(&a, "pending").unwrap();
        let old = tracker.observe([a.clone()]).unwrap();
        fs::remove_file(&a).unwrap();
        symlink(&a, &a).unwrap(); // A deterministic read error without relying on permission bits.
        assert!(tracker.observe([a.clone()]).is_err());
        assert!(!tracker.is_current(old.revision));
        assert!(!tracker.mark_applied(&old));
        fs::write(&b, "changed b").unwrap();
        assert!(tracker.observe([b.clone()]).is_err());
        fs::remove_file(&a).unwrap();
        fs::write(&a, "recovered a").unwrap();
        let current = tracker.observe([b]).unwrap();
        assert_eq!(current.changes.len(), 2);
        assert_eq!(
            current
                .changes
                .iter()
                .find(|change| change.path == a)
                .unwrap()
                .content
                .as_deref(),
            Some(b"recovered a".as_slice())
        );
        assert!(tracker.mark_applied(&current));
    }

    #[test]
    fn build_inputs_take_priority_over_rs() {
        let custom = PathBuf::from("tools/build-custom.rs");
        let inputs = BTreeSet::from([custom.clone()]);
        for path in [
            "Cargo.toml",
            "Cargo.lock",
            "build.rs",
            ".cargo/config.toml",
            ".cargo/config",
        ] {
            assert_eq!(classify(Path::new(path), &inputs), PathKind::Manifest);
        }
        assert_eq!(classify(&custom, &inputs), PathKind::Manifest);
        assert_eq!(classify(Path::new("src/lib.rs"), &inputs), PathKind::Rs);
        assert_eq!(
            classify(Path::new("src/data.txt"), &inputs),
            PathKind::Other
        );
    }
    #[test]
    fn runtime_revert_keeps_unfixed_editor_bytes_pending_for_other_file_saves() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.rs");
        let b = dir.path().join("b.rs");
        fs::write(&a, "checked").unwrap();
        fs::write(&b, "old b").unwrap();
        let mut tracker = ChangeTracker::new([a.clone(), b.clone()], BTreeSet::new()).unwrap();
        fs::write(&a, "invalid but optimistically applied").unwrap();
        let applied = tracker.observe([a.clone()]).unwrap();
        assert!(tracker.mark_applied(&applied));
        tracker
            .restore_applied(&[(a.clone(), Some(Arc::from(b"checked".as_slice())))])
            .unwrap();
        assert_eq!(fs::read(&a).unwrap(), b"invalid but optimistically applied");
        fs::write(&b, "new b").unwrap();
        let changes = tracker.observe([b]).unwrap();
        assert_eq!(changes.changes.len(), 2);
        assert!(changes.changes.iter().any(|c| c.path == a
            && c.content.as_deref() == Some(b"invalid but optimistically applied".as_slice())));
    }
    #[test]
    fn late_runtime_commit_reconciles_a_round_trip_to_the_old_baseline() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.rs");
        fs::write(&a, "one").unwrap();
        let mut tracker = ChangeTracker::new([a.clone()], BTreeSet::new()).unwrap();
        fs::write(&a, "two").unwrap();
        let committing = tracker.observe([a.clone()]).unwrap();
        fs::write(&a, "one").unwrap();
        assert_eq!(
            tracker.observe([a.clone()]).unwrap().disposition,
            Disposition::Skip
        );
        assert!(!tracker.mark_applied(&committing));
        tracker
            .restore_applied(&[(a.clone(), Some(Arc::from(b"two".as_slice())))])
            .unwrap();
        let reconciled = tracker.observe([a]).unwrap();
        assert_eq!(reconciled.disposition, Disposition::Changed);
        assert_eq!(
            reconciled.changes[0].content.as_deref(),
            Some(b"one".as_slice())
        );
    }
}
