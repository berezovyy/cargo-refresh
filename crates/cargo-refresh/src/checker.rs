//! Frozen private workspaces: checks and auto-revert never write editor sources.
use crate::{
    changes::ChangeSet,
    config::Config,
    fallback::{self, CargoResult, Fence},
};
use anyhow::{Context, Result, ensure};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
};

pub type Sources = BTreeMap<PathBuf, Arc<[u8]>>;
pub struct Workspace {
    _directory: tempfile::TempDir,
    pub root: PathBuf,
    original: PathBuf,
}
impl Workspace {
    pub fn copy(config: &Config) -> Result<Self> {
        let directory = tempfile::Builder::new()
            .prefix("refresh-check-")
            .tempdir()?;
        let mut original = config.workspace_root.clone();
        for package in &config.package_roots {
            while !package.starts_with(&original) {
                original.pop();
            }
        }
        ensure!(
            original.parent().is_some(),
            "local dependencies span filesystem root; cannot snapshot safely"
        );
        let tree = directory.path().join("tree");
        fs::create_dir(&tree)?;
        let root = tree.join(config.workspace_root.strip_prefix(&original)?);
        let mut roots = vec![config.workspace_root.clone()];
        for package in &config.package_roots {
            if !roots.iter().any(|p| package.starts_with(p)) {
                roots.push(package.clone());
            }
        }
        for source in &roots {
            let entries = walkdir::WalkDir::new(source)
                .into_iter()
                .filter_entry(|entry| {
                    entry.depth() == 0
                        || (!entry.path().starts_with(&config.target_directory)
                            && !matches!(entry.file_name().to_str(), Some(".git" | "target"))
                            && !entry.file_name().to_string_lossy().ends_with("-target"))
                });
            for entry in entries {
                let entry = entry?;
                let dst = tree.join(entry.path().strip_prefix(&original)?);
                ensure!(
                    !entry.file_type().is_symlink(),
                    "snapshot workspace contains a symlink: {}",
                    entry.path().display()
                );
                if entry.file_type().is_dir() {
                    fs::create_dir_all(dst)?;
                } else {
                    fs::copy(entry.path(), dst)?;
                }
            }
        }
        Ok(Self {
            _directory: directory,
            root,
            original,
        })
    }
    pub fn write(&self, path: &Path, bytes: Option<&[u8]>) -> Result<()> {
        let relative = path
            .strip_prefix(&self.original)
            .context("source outside workspace cannot be snapshotted")?;
        ensure!(
            relative
                .components()
                .all(|part| matches!(part, std::path::Component::Normal(_))),
            "invalid snapshot-relative source path"
        );
        let dst = self._directory.path().join("tree").join(relative);
        if let Some(bytes) = bytes {
            fs::create_dir_all(dst.parent().unwrap())?;
            fs::write(dst, bytes)?;
        } else if dst.is_file() {
            fs::remove_file(dst)?;
        }
        Ok(())
    }
    pub fn update(&self, changes: &ChangeSet) -> Result<()> {
        for change in &changes.changes {
            self.write(&change.path, change.content.as_deref())?;
        }
        Ok(())
    }
}
pub struct Checker {
    cancel: Arc<AtomicBool>,
    task: Option<JoinHandle<Result<CargoResult>>>,
    pub revision: u64,
}
impl Checker {
    pub fn start(config: &Config, workspace: &Workspace, fence: Fence, floor: u64) -> Self {
        let storage = config.target_directory.join("refresh");
        let command = fallback::cargo(config, &workspace.root, &storage.join("check"), "check");
        let cancel = Arc::new(AtomicBool::new(false));
        let stopped = cancel.clone();
        let revision = fence.revision;
        let task = thread::spawn(move || {
            // A small monitor combines local cancellation with the session shutdown fence.
            let local = Arc::new(AtomicBool::new(false));
            let observed = local.clone();
            let upstream = fence.stopping.clone();
            let done = Arc::new(AtomicBool::new(false));
            let finished = done.clone();
            let monitor = thread::spawn(move || {
                while !finished.load(Ordering::SeqCst) {
                    if stopped.load(Ordering::SeqCst) || upstream.load(Ordering::SeqCst) {
                        observed.store(true, Ordering::SeqCst);
                        break;
                    }
                    thread::sleep(std::time::Duration::from_millis(10));
                }
            });
            let fence = Fence {
                stopping: local,
                ..fence
            };
            let result = fallback::run(command, &storage.join("check.log"), &fence, floor);
            done.store(true, Ordering::SeqCst);
            let _ = monitor.join();
            result
        });
        Self {
            cancel,
            task: Some(task),
            revision,
        }
    }
    pub fn finished(&self) -> bool {
        self.task.as_ref().is_some_and(|task| task.is_finished())
    }
    pub fn finish(&mut self) -> Result<CargoResult> {
        self.task
            .take()
            .context("checker already consumed")?
            .join()
            .map_err(|_| anyhow::anyhow!("checker thread panicked"))?
    }
    pub fn cancel(&mut self) {
        self.cancel.store(true, Ordering::SeqCst);
        if let Some(task) = self.task.take() {
            let _ = task.join();
        }
    }
}
impl Drop for Checker {
    fn drop(&mut self) {
        self.cancel();
    }
}
pub fn apply(sources: &mut Sources, changes: &ChangeSet) {
    for change in &changes.changes {
        if let Some(bytes) = &change.content {
            sources.insert(change.path.clone(), bytes.clone());
        } else {
            sources.remove(&change.path);
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn json_diagnostics_and_artifacts() {
        let text = "{\"reason\":\"compiler-message\",\"message\":{\"level\":\"error\",\"message\":\"borrow conflict\",\"code\":{\"code\":\"E0502\"}}}\n{\"reason\":\"compiler-artifact\",\"executable\":\"/tmp/app\"}\n";
        let result = fallback::parse_output(text, false);
        assert!(!result.success);
        assert_eq!(result.diagnostic, "E0502 borrow conflict");
        assert_eq!(result.executable.unwrap(), PathBuf::from("/tmp/app"));
    }
    #[test]
    fn snapshot_with_external_path_dependency_never_aliases_editor_files() {
        let dir = tempfile::tempdir().unwrap();
        let app = dir.path().join("app");
        let dep = dir.path().join("dep");
        for root in [&app, &dep] {
            fs::create_dir_all(root.join("src")).unwrap();
            fs::write(
                root.join("Cargo.toml"),
                "[package]\nname='fixture'\nversion='0.0.0'\n",
            )
            .unwrap();
            fs::write(root.join("src/main.rs"), "fn main() {}\n").unwrap();
        }
        let metadata:crate::config::Metadata=serde_json::from_value(serde_json::json!({
            "workspace_root":app,"target_directory":app.join("target"),"workspace_default_members":["app"],
            "packages":[{"id":"app","name":"app","manifest_path":app.join("Cargo.toml"),"source":null,"targets":[{"name":"app","kind":["bin"],"src_path":app.join("src/main.rs")}],"metadata":{},"default_run":null},
            {"id":"dep","name":"dep","manifest_path":dep.join("Cargo.toml"),"source":null,"targets":[{"name":"dep","kind":["lib"],"src_path":dep.join("src/main.rs")}],"metadata":{},"default_run":null}]})).unwrap();
        let config = Config::resolve(crate::cli::RunArgs::default(), &metadata, &app).unwrap();
        let workspace = Workspace::copy(&config).unwrap();
        let source = dep.join("src/main.rs");
        let original = fs::read(&source).unwrap();
        workspace
            .write(&source, Some(b"checked snapshot\n"))
            .unwrap();
        assert_eq!(fs::read(&source).unwrap(), original);
        assert_eq!(
            fs::read(workspace.root.join("../dep/src/main.rs")).unwrap(),
            b"checked snapshot\n"
        );
        workspace.write(&source, None).unwrap();
        assert_eq!(fs::read(&source).unwrap(), original);
        assert!(
            workspace
                .write(&app.join("../escaped.rs"), Some(b"bad"))
                .is_err()
        );
    }
}
