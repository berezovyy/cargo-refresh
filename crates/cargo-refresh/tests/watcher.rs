//! Real inotify integration, not a mocked filesystem or an elapsed-time benchmark.
use std::{
    collections::BTreeSet,
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant},
};

use cargo_refresh::{
    changes::{ChangeSet, ChangeTracker, Disposition, PathKind},
    watcher::{SourceWatcher, WatchSpec},
};

struct Fixture {
    _directory: tempfile::TempDir,
    root: PathBuf,
    source: PathBuf,
    watcher: SourceWatcher,
    tracker: ChangeTracker,
}

impl Fixture {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().to_path_buf();
        fs::create_dir(root.join("src")).unwrap();
        let source = root.join("src/main.rs");
        fs::write(&source, "initial").unwrap();
        fs::write(root.join("Cargo.toml"), "initial manifest").unwrap();
        let watcher = SourceWatcher::new(WatchSpec::new(
            vec![root.join("src")],
            vec![
                root.join("Cargo.toml"),
                root.join("Cargo.lock"),
                root.join("build.rs"),
                root.join(".cargo/config.toml"),
            ],
            vec![root.join("custom-target-dir")],
        ))
        .unwrap();
        let tracker = ChangeTracker::new(watcher.files().unwrap(), watcher.build_inputs()).unwrap();
        Self {
            _directory: directory,
            root,
            source,
            watcher,
            tracker,
        }
    }

    fn next(&mut self) -> Option<ChangeSet> {
        self.watcher
            .next_batch(Duration::from_millis(100))
            .unwrap()
            .map(|batch| {
                if batch.full_rehash {
                    self.tracker.rehash_all(batch.paths).unwrap()
                } else {
                    self.tracker.observe(batch.paths).unwrap()
                }
            })
    }

    fn wait_for(&mut self, path: &Path, expected: Option<&[u8]>) -> ChangeSet {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if let Some(set) = self.next() {
                let found = set
                    .changes
                    .iter()
                    .any(|change| change.path == path && change.content.as_deref() == expected);
                assert!(self.tracker.mark_applied(&set));
                if found {
                    return set;
                }
            }
        }
        panic!("missed final content for {}", path.display());
    }

    fn drain(&mut self) {
        while let Some(set) = self.next() {
            assert!(self.tracker.mark_applied(&set));
        }
    }
}

#[test]
fn one_hundred_tight_writes_coalesce_and_never_lose_final_content() {
    let mut fixture = Fixture::new();
    let path = fixture.source.clone();
    let writer = thread::spawn(move || {
        for revision in 1..=100 {
            fs::write(&path, format!("version-{revision}")).unwrap();
        }
    });
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut sets = 0;
    let mut final_seen = false;
    while Instant::now() < deadline {
        if let Some(set) = fixture.next() {
            sets += 1;
            final_seen |= set.changes.iter().any(|change| {
                change.path == fixture.source
                    && change.content.as_deref() == Some(b"version-100".as_slice())
            });
            assert!(fixture.tracker.mark_applied(&set));
        } else if writer.is_finished() && final_seen {
            break;
        }
    }
    writer.join().unwrap();
    assert!(final_seen, "final version never observed");
    assert!(
        (1..=100).contains(&sets),
        "100 writes produced {sets} change sets"
    );
    assert_eq!(fs::read(&fixture.source).unwrap(), b"version-100");
    // All notifications through the final close must also be drained and counted.
    while let Some(set) = fixture.next() {
        sets += 1;
        assert!(fixture.tracker.mark_applied(&set));
    }
    assert!(
        sets <= 100,
        "100 writes produced {sets} change sets after draining"
    );
}

#[test]
fn identical_rewrites_produce_skip_from_real_inotify() {
    let mut fixture = Fixture::new();
    fs::write(&fixture.source, "initial").unwrap();
    let set = fixture.next().expect("close-write must be noticed");
    assert_eq!(set.disposition, Disposition::Skip);
    assert!(set.changes.is_empty());
}

#[test]
fn truncate_then_write_waits_for_close_and_observes_the_complete_file() {
    let mut fixture = Fixture::new();
    let mut file = OpenOptions::new()
        .write(true)
        .truncate(true)
        .open(&fixture.source)
        .unwrap();
    // No fixed delay is needed to combine syscalls: close-write is the source event.
    assert!(fixture.next().is_none());
    file.write_all(b"after truncate").unwrap();
    drop(file);
    fixture.wait_for(&fixture.source.clone(), Some(b"after truncate"));
}

#[test]
fn temp_file_rename_replaces_a_file_and_remains_watched() {
    let mut fixture = Fixture::new();
    for content in [b"rename one".as_slice(), b"rename two".as_slice()] {
        let temporary = fixture.root.join("src/.main.rs.tmp");
        fs::write(&temporary, content).unwrap();
        fs::rename(&temporary, &fixture.source).unwrap();
        fixture.wait_for(&fixture.source.clone(), Some(content));
        fixture.drain();
    }
    fs::write(&fixture.source, "after rename").unwrap();
    fixture.wait_for(&fixture.source.clone(), Some(b"after rename"));
}

#[test]
fn created_and_renamed_directories_are_scanned_and_watched() {
    let mut fixture = Fixture::new();
    let original = fixture.root.join("src/new");
    fs::create_dir_all(original.join("nested")).unwrap();
    let file = original.join("nested/added.rs");
    fs::write(&file, "added").unwrap();
    fixture.wait_for(&file, Some(b"added"));
    fixture.drain();
    let renamed = fixture.root.join("src/renamed");
    fs::rename(&original, &renamed).unwrap();
    let moved = renamed.join("nested/added.rs");
    fixture.wait_for(&moved, Some(b"added"));
    fixture.drain();
    fs::write(&moved, "after directory rename").unwrap();
    fixture.wait_for(&moved, Some(b"after directory rename"));
    fixture.drain();
    fs::remove_dir_all(&renamed).unwrap();
    fixture.wait_for(&moved, None);
}

#[test]
fn source_root_replacement_is_observed_by_parent_sentinel() {
    let mut fixture = Fixture::new();
    let source_root = fixture.root.join("src");
    fs::remove_dir_all(&source_root).unwrap();
    fixture.wait_for(&fixture.source.clone(), None);
    fixture.drain();
    fs::create_dir(&source_root).unwrap();
    fs::write(&fixture.source, "recreated").unwrap();
    fixture.wait_for(&fixture.source.clone(), Some(b"recreated"));
    fixture.drain();
    fs::write(&fixture.source, "recreated again").unwrap();
    fixture.wait_for(&fixture.source.clone(), Some(b"recreated again"));
}

#[test]
fn manifest_lock_build_script_and_initially_absent_cargo_config_are_build_inputs() {
    let mut fixture = Fixture::new();
    for name in ["Cargo.toml", "Cargo.lock", "build.rs", ".cargo/config.toml"] {
        let path = fixture.root.join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "first content").unwrap();
        let set = fixture.wait_for(&path, Some(b"first content"));
        assert_eq!(
            set.changes
                .iter()
                .find(|change| change.path == path)
                .unwrap()
                .kind,
            PathKind::Manifest
        );
        fixture.drain();
        fs::write(&path, "second content").unwrap();
        fixture.wait_for(&path, Some(b"second content"));
        fixture.drain();
    }
}

#[test]
fn hidden_and_target_trees_are_ignored_but_other_source_files_are_tracked() {
    let mut fixture = Fixture::new();
    for directory in [
        "src/target",
        "src/check-target",
        "src/.git",
        "src/.editor",
        "custom-target-dir",
    ] {
        let path = fixture.root.join(directory).join("noise.rs");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, "noise").unwrap();
    }
    assert!(fixture.next().is_none());
    let other = fixture.root.join("src/payload.txt");
    fs::write(&other, "data").unwrap();
    let set = fixture.wait_for(&other, Some(b"data"));
    assert_eq!(
        set.changes
            .iter()
            .find(|change| change.path == other)
            .unwrap()
            .kind,
        PathKind::Other
    );
    assert_eq!(
        set.changes
            .iter()
            .map(|change| &change.path)
            .collect::<BTreeSet<_>>(),
        BTreeSet::from([&other])
    );
}
