//! Filtered nonrecursive inotify watches, with a deadline exactly 1 ms after the last event.
use std::{
    collections::{BTreeSet, HashMap},
    fs, io,
    os::unix::fs::MetadataExt,
    path::{Component, Path, PathBuf},
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail};
use crossbeam_channel::{Receiver, RecvTimeoutError, unbounded};
use notify::{
    Event, EventKind, INotifyWatcher, RecursiveMode, Watcher,
    event::{AccessKind, AccessMode, CreateKind, ModifyKind, RemoveKind},
};
use walkdir::WalkDir;

pub const DEBOUNCE: Duration = Duration::from_millis(1);

#[derive(Debug, Clone)]
pub struct WatchSpec {
    source_roots: Vec<PathBuf>,
    input_files: BTreeSet<PathBuf>,
    excluded_dirs: Vec<PathBuf>,
    excluded_files: BTreeSet<PathBuf>,
}

impl WatchSpec {
    pub fn new(
        source_roots: Vec<PathBuf>,
        input_files: Vec<PathBuf>,
        excluded_dirs: Vec<PathBuf>,
    ) -> Self {
        Self {
            source_roots,
            input_files: input_files.into_iter().collect(),
            excluded_dirs,
            excluded_files: BTreeSet::new(),
        }
    }

    pub fn exclude_file(&mut self, path: PathBuf) {
        if let Ok(canonical) = path.canonicalize() {
            self.excluded_files.insert(canonical);
        }
        self.excluded_files.insert(path);
    }

    pub fn build_inputs(&self) -> BTreeSet<PathBuf> {
        self.input_files.clone()
    }

    fn excluded(&self, path: &Path) -> bool {
        self.excluded_files.contains(path)
            || self.excluded_dirs.iter().any(|dir| path.starts_with(dir))
    }

    fn source_path(&self, path: &Path, directory: bool) -> bool {
        if self.excluded(path) {
            return false;
        }
        self.source_roots.iter().any(|root| {
            let Ok(relative) = path.strip_prefix(root) else {
                return false;
            };
            let directories = if directory {
                relative
            } else {
                relative.parent().unwrap_or(Path::new(""))
            };
            directories.components().all(|part| match part {
                Component::Normal(name) => !skip_directory(name.to_string_lossy().as_ref()),
                _ => false,
            })
        })
    }

    fn accepts_file(&self, path: &Path) -> bool {
        !self.excluded_files.contains(path)
            && (self.input_files.contains(path) || self.source_path(path, false))
    }

    fn topology_path(&self, path: &Path) -> bool {
        !self.excluded_files.contains(path)
            && ((!self.excluded(path)
                && (self.source_path(path, true)
                    || self.source_roots.iter().any(|root| root.starts_with(path))))
                || self.input_files.iter().any(|input| input.starts_with(path)))
    }

    fn inventory(&self) -> Result<Inventory> {
        let mut inventory = Inventory::default();
        for root in &self.source_roots {
            // Parent sentinels keep roots observable across deletion and replacement.
            if let Some(parent) = root.parent().and_then(existing_directory) {
                inventory.directories.insert(parent);
            }
            if !root.is_dir() || self.excluded(root) {
                continue;
            }
            let entries = WalkDir::new(root)
                .follow_links(false)
                .into_iter()
                .filter_entry(|entry| {
                    !self.excluded(entry.path())
                        && (!entry.file_type().is_dir() || self.source_path(entry.path(), true))
                });
            for entry in entries {
                let entry = match entry {
                    Ok(entry) => entry,
                    Err(error)
                        if error
                            .io_error()
                            .is_some_and(|e| e.kind() == io::ErrorKind::NotFound) =>
                    {
                        continue;
                    }
                    Err(error) => {
                        return Err(error)
                            .with_context(|| format!("scan source root {}", root.display()));
                    }
                };
                if entry.file_type().is_dir() {
                    inventory.directories.insert(entry.path().to_path_buf());
                } else if entry.path().is_file() && self.accepts_file(entry.path()) {
                    inventory.files.insert(entry.path().to_path_buf());
                }
            }
        }
        for file in &self.input_files {
            // Explicit rustc inputs may be generated beneath target/. Watch the
            // named input alone; output-directory recursion remains excluded.
            if self.excluded_files.contains(file) {
                continue;
            }
            if file.is_file() {
                inventory.files.insert(file.clone());
            }
            // Watch the nearest existing parent so initially absent Cargo.lock/build.rs/.cargo work.
            if let Some(parent) = file.parent().and_then(existing_directory) {
                inventory.directories.insert(parent);
            }
        }
        Ok(inventory)
    }
}

fn existing_directory(path: &Path) -> Option<PathBuf> {
    path.ancestors()
        .find(|path| path.is_dir())
        .map(Path::to_path_buf)
}

fn skip_directory(name: &str) -> bool {
    name == "target" || name.ends_with("-target") || (name.starts_with('.') && name != ".cargo")
}

fn ignored_kind(kind: EventKind) -> bool {
    matches!(kind, EventKind::Access(access) if access != AccessKind::Close(AccessMode::Write))
        || matches!(kind, EventKind::Modify(ModifyKind::Data(_)))
}

fn directory_identity(path: &Path) -> io::Result<Option<(u64, u64)>> {
    match fs::metadata(path) {
        Ok(metadata) => Ok(metadata.is_dir().then(|| (metadata.dev(), metadata.ino()))),
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
            ) =>
        {
            Ok(None)
        }
        Err(error) => Err(error),
    }
}

#[derive(Default)]
struct Inventory {
    files: BTreeSet<PathBuf>,
    directories: BTreeSet<PathBuf>,
}

struct Notice {
    event: notify::Result<Event>,
    at: Instant,
}

#[derive(Debug)]
pub struct WatchBatch {
    pub paths: BTreeSet<PathBuf>,
    /// On overflow or topology changes, compare the entire inventory including deleted known files.
    pub full_rehash: bool,
    pub noticed_at: Instant,
}

#[derive(Default)]
struct Pending {
    paths: BTreeSet<PathBuf>,
    full_rehash: bool,
    refresh_watches: bool,
    first: Option<Instant>,
    deadline: Option<Instant>,
}

impl Pending {
    fn accept(
        &mut self,
        event: &Event,
        at: Instant,
        spec: &WatchSpec,
        watched: &BTreeSet<PathBuf>,
    ) {
        if event.need_rescan() || (matches!(event.kind, EventKind::Any) && event.paths.is_empty()) {
            self.full_rehash = true;
            self.refresh_watches = true;
            self.bump(at);
            return;
        }
        // Match the prototype's close-write contract. In particular, ignore our own read opens/closes.
        // Data writes are observed on close, avoiding a second change set for a single completed save.
        if ignored_kind(event.kind) {
            return;
        }
        let topology_kind = matches!(
            event.kind,
            EventKind::Create(_) | EventKind::Remove(_) | EventKind::Modify(ModifyKind::Name(_))
        );
        let folder_kind = matches!(
            event.kind,
            EventKind::Create(CreateKind::Folder) | EventKind::Remove(RemoveKind::Folder)
        );
        let mut relevant = false;
        for path in &event.paths {
            let directory = folder_kind || watched.contains(path) || path.is_dir();
            if topology_kind && directory && spec.topology_path(path) {
                self.full_rehash = true;
                self.refresh_watches = true;
                relevant = true;
            } else if !directory && spec.accepts_file(path) {
                self.paths.insert(path.clone());
                relevant = true;
            }
        }
        if relevant {
            self.bump(at);
        }
    }

    fn bump(&mut self, at: Instant) {
        self.first.get_or_insert(at);
        self.deadline = Some(at + DEBOUNCE);
    }
}

pub struct SourceWatcher {
    watcher: INotifyWatcher,
    notices: Receiver<Notice>,
    watched: BTreeSet<PathBuf>,
    identities: HashMap<PathBuf, (u64, u64)>,
    spec: WatchSpec,
}

impl SourceWatcher {
    pub fn new(spec: WatchSpec) -> Result<Self> {
        let (sender, notices) = unbounded();
        let watcher = INotifyWatcher::new(
            move |event: notify::Result<Event>| {
                // Do not queue the opens/read-closes generated by our own hashing and scans.
                if event
                    .as_ref()
                    .is_ok_and(|event| !event.need_rescan() && ignored_kind(event.kind))
                {
                    return;
                }
                let _ = sender.send(Notice {
                    event,
                    at: Instant::now(),
                });
            },
            notify::Config::default(),
        )
        .context("initialize inotify watcher")?;
        let mut result = Self {
            watcher,
            notices,
            watched: BTreeSet::new(),
            identities: HashMap::new(),
            spec,
        };
        result.refresh_watches()?;
        Ok(result)
    }

    /// Preserve queued close-write notices while capture adds dependency inputs.
    pub fn reconfigure(&mut self, spec: WatchSpec) -> Result<()> {
        self.spec = spec;
        self.refresh_watches()
    }

    /// Take the baseline after installing watches; events during hashing remain queued.
    pub fn files(&self) -> Result<BTreeSet<PathBuf>> {
        Ok(self.spec.inventory()?.files)
    }

    pub fn build_inputs(&self) -> BTreeSet<PathBuf> {
        self.spec.build_inputs()
    }

    /// Wait using channel deadlines, never a polling/sleep loop. Timeout is an idle bound only.
    pub fn next_batch(&mut self, timeout: Duration) -> Result<Option<WatchBatch>> {
        let idle_deadline = Instant::now() + timeout;
        let mut pending = Pending::default();
        loop {
            let deadline = pending.deadline.unwrap_or(idle_deadline);
            match self.notices.recv_deadline(deadline) {
                Ok(notice) => {
                    let event = notice.event.context("inotify event error")?;
                    pending.accept(&event, notice.at, &self.spec, &self.watched);
                }
                Err(RecvTimeoutError::Timeout) => {
                    let Some(noticed_at) = pending.first else {
                        return Ok(None);
                    };
                    if pending.refresh_watches {
                        self.refresh_watches()?;
                    }
                    if pending.full_rehash {
                        pending.paths.extend(self.files()?);
                    }
                    return Ok(Some(WatchBatch {
                        paths: pending.paths,
                        full_rehash: pending.full_rehash,
                        noticed_at,
                    }));
                }
                Err(RecvTimeoutError::Disconnected) => bail!("inotify watcher disconnected"),
            }
        }
    }

    fn refresh_watches(&mut self) -> Result<()> {
        loop {
            let directories = self.spec.inventory()?.directories;
            let mut obsolete = Vec::new();
            for old in &self.watched {
                if !directories.contains(old)
                    || directory_identity(old)? != self.identities.get(old).copied()
                {
                    obsolete.push(old.clone());
                }
            }
            for old in obsolete {
                let _ = self.watcher.unwatch(&old);
                // notify removes descendant watches when unwatching an ancestor.
                self.watched.retain(|path| !path.starts_with(&old));
                self.identities.retain(|path, _| !path.starts_with(&old));
            }
            let mut added = false;
            for directory in directories {
                if self.watched.contains(&directory) {
                    continue;
                }
                let Some(identity) = directory_identity(&directory)? else {
                    continue;
                };
                match self.watcher.watch(&directory, RecursiveMode::NonRecursive) {
                    Ok(()) => {
                        self.identities.insert(directory.clone(), identity);
                        self.watched.insert(directory);
                        added = true;
                    }
                    Err(error) if matches!(error.kind, notify::ErrorKind::PathNotFound) => continue,
                    Err(error) => {
                        return Err(error)
                            .with_context(|| format!("watch {}", directory.display()));
                    }
                }
            }
            // Keep existing parent watches alive. Rescan after adding watches to catch directories
            // created between inventory and installation, including prepopulated moved-in trees.
            if !added {
                break;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use notify::event::{Flag, RenameMode};
    use std::fs;

    fn spec(root: &Path) -> WatchSpec {
        WatchSpec::new(
            vec![root.join("src")],
            vec![root.join("Cargo.toml"), root.join(".cargo/config.toml")],
            vec![root.join("custom-build-output")],
        )
    }

    #[test]
    fn inventory_prunes_output_and_hidden_directories_but_keeps_cargo() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let keep = [
            "src/main.rs",
            "src/data.txt",
            "src/.cargo/config.toml",
            "Cargo.toml",
        ];
        let skip = [
            "src/target/a.rs",
            "src/stock-target/a.rs",
            "src/.git/a.rs",
            "src/.hidden/a.rs",
        ];
        for path in keep.into_iter().chain(skip) {
            let path = root.join(path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, "x").unwrap();
        }
        let inventory = spec(root).inventory().unwrap();
        assert_eq!(
            inventory.files,
            keep.into_iter().map(|p| root.join(p)).collect()
        );
        assert!(!inventory.directories.contains(&root.join("src/.hidden")));
        assert!(!inventory.directories.contains(&root.join("src/target")));
        assert!(inventory.directories.contains(root));
    }

    #[test]
    fn explicit_generated_input_is_watched_without_watching_other_outputs() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("target/build/out")).unwrap();
        let input = root.join("target/build/out/data.bin");
        fs::write(&input, "before").unwrap();
        let spec = WatchSpec::new(vec![], vec![input.clone()], vec![root.join("target")]);
        assert!(spec.accepts_file(&input));
        assert!(!spec.accepts_file(&root.join("target/build/out/unrelated")));
        let mut watcher = SourceWatcher::new(spec).unwrap();
        fs::write(&input, "after").unwrap();
        assert!(
            watcher
                .next_batch(Duration::from_secs(2))
                .unwrap()
                .unwrap()
                .paths
                .contains(&input)
        );
    }

    #[test]
    fn dependency_discovery_does_not_swallow_a_save_queued_during_capture() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir(root.join("src")).unwrap();
        let input = root.join("message.txt");
        fs::write(&input, "before").unwrap();
        let mut watcher = SourceWatcher::new(spec(root)).unwrap();
        let mut tracker =
            crate::changes::ChangeTracker::new(watcher.files().unwrap(), watcher.build_inputs())
                .unwrap();
        fs::write(&input, "after").unwrap();
        let mut updated = spec(root);
        updated.input_files.insert(input.clone());
        watcher.reconfigure(updated).unwrap();
        tracker.set_build_inputs(watcher.build_inputs());
        let batch = watcher.next_batch(Duration::from_secs(2)).unwrap().unwrap();
        let changes = tracker.observe(batch.paths).unwrap();
        assert_eq!(changes.changes.len(), 1);
        assert_eq!(
            changes.changes[0].content.as_deref(),
            Some(b"after".as_slice())
        );
        assert_eq!(changes.changes[0].kind, crate::changes::PathKind::Manifest);
    }

    #[test]
    fn reconfiguration_preserves_already_queued_source_saves() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir(root.join("src")).unwrap();
        let input = root.join("src/main.rs");
        fs::write(&input, "before").unwrap();
        let mut watcher = SourceWatcher::new(spec(root)).unwrap();
        fs::write(&input, "after").unwrap();
        watcher.reconfigure(spec(root)).unwrap();
        assert!(
            watcher
                .next_batch(Duration::from_secs(2))
                .unwrap()
                .unwrap()
                .paths
                .contains(&input)
        );
    }

    #[test]
    fn deadline_moves_to_exactly_one_ms_after_last_relevant_event() {
        let root = Path::new("/project");
        let spec = spec(root);
        let at = Instant::now();
        let event = Event::new(EventKind::Access(AccessKind::Close(AccessMode::Write)))
            .add_path(root.join("src/main.rs"));
        let mut pending = Pending::default();
        pending.accept(&event, at, &spec, &BTreeSet::new());
        assert_eq!(pending.deadline, Some(at + Duration::from_millis(1)));
        let last = at + Duration::from_micros(750);
        pending.accept(&event, last, &spec, &BTreeSet::new());
        assert_eq!(pending.deadline, Some(last + Duration::from_millis(1)));
        assert_eq!(pending.first, Some(at));
        let read = Event::new(EventKind::Access(AccessKind::Close(AccessMode::Read)))
            .add_path(root.join("src/main.rs"));
        pending.accept(
            &read,
            last + Duration::from_secs(1),
            &spec,
            &BTreeSet::new(),
        );
        assert_eq!(pending.deadline, Some(last + Duration::from_millis(1)));
    }

    #[test]
    fn overflow_and_directory_rename_request_full_rehash() {
        let root = Path::new("/project");
        let spec = spec(root);
        let mut pending = Pending::default();
        pending.accept(
            &Event::new(EventKind::Other).set_flag(Flag::Rescan),
            Instant::now(),
            &spec,
            &BTreeSet::new(),
        );
        assert!(pending.full_rehash && pending.refresh_watches);
        let mut pending = Pending::default();
        let path = root.join("src/nested");
        let rename = Event::new(EventKind::Modify(ModifyKind::Name(RenameMode::From)))
            .add_path(path.clone());
        pending.accept(&rename, Instant::now(), &spec, &BTreeSet::from([path]));
        assert!(pending.full_rehash && pending.refresh_watches);
    }

    #[test]
    fn source_output_and_log_exclusions_apply_to_events() {
        let root = Path::new("/project");
        let mut spec = spec(root);
        spec.exclude_file(root.join("src/events.jsonl"));
        for path in [
            "src/.git/index",
            "src/target/output",
            "src/foo-target/output",
            "src/events.jsonl",
            "unrelated.rs",
        ] {
            assert!(
                !spec.accepts_file(&root.join(path)),
                "unexpected input: {path}"
            );
        }
        assert!(spec.accepts_file(&root.join("src/.cargo/config.toml")));
        assert!(spec.accepts_file(&root.join("src/data.txt")));
    }
}
