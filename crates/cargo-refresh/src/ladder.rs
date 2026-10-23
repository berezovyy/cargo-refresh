//! Serialized runtime ownership. The watcher advances the fence during blocking work.
use crate::{
    agent,
    changes::{ChangeSet, Disposition, PathKind},
    checker::{self, Checker, Sources, Workspace},
    config::Config,
    elf::Image,
    events::{Phase, SaveEvent, SavePath, monotonic_seconds},
    extractor::ManifestExtractor,
    fallback::{self, Fence},
    manifest::State,
    rustd,
    supervisor::Supervisor,
};
use anyhow::{Context, Result, ensure};
use refresh_guard::{Admission, AdmissionRule, Class, FileSnapshot, Guard, Reason};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::PathBuf,
    sync::Arc,
    thread::{self, JoinHandle},
};

#[derive(Clone)]
pub struct Job {
    pub changes: ChangeSet,
    pub phases: BTreeMap<Phase, f64>,
}
pub enum Update {
    Ready,
    AgentReply {
        reply: agent::Reply,
        revision: u64,
    },
    Reconfigure(Box<Config>, u64),
    Reverted {
        paths: Vec<(PathBuf, Option<Arc<[u8]>>)>,
        revision: u64,
    },
    Save {
        event: SaveEvent,
        applied: Option<ChangeSet>,
        generation: u64,
    },
    Event {
        name: &'static str,
        message: String,
        revision: u64,
    },
    Fatal(String),
}
pub struct Fast {
    client: rustd::Client,
    extractor: ManifestExtractor,
    manifest: State,
    control: PathBuf,
    promotions: u32,
}
struct Reset {
    task: JoinHandle<Result<Fast>>,
    revision: u64,
    cancel: Arc<std::sync::atomic::AtomicBool>,
}
pub struct Pipeline {
    config: Config,
    runtime: Supervisor,
    fast: Option<Fast>,
    reset: Option<Reset>,
    reset_seq: u64,
    sources: Sources,
    checked: Sources,
    checked_available: bool,
    workspace: Option<Workspace>,
    checker: Option<(Checker, ChangeSet)>,
    guard: Guard,
    generation: u64,
    floor: u64,
    sender: crossbeam_channel::Sender<Update>,
    fence: Fence,
    /// Paths whose runtime bytes differ from the last checked snapshot.
    unchecked: BTreeSet<PathBuf>,
    hook_missing: bool,
    recovery_restart: bool,
    view_dirty: BTreeSet<PathBuf>,
    session_home: PathBuf,
}
fn mark(phases: &mut BTreeMap<Phase, f64>, phase: Phase) {
    phases.insert(phase, monotonic_seconds().unwrap_or(0.0) * 1000.0);
}
fn compiler_path() -> Result<PathBuf> {
    if let Some(path) = std::env::var_os("REFRESH_RUSTD") {
        return Ok(path.into());
    }
    for dir in std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()) {
        let path = dir.join("rustd");
        if path.is_file() {
            return Ok(path);
        }
    }
    anyhow::bail!("rustd is not installed; set REFRESH_RUSTD or put rustd on PATH")
}
fn native_binary(config: &Config) -> Result<PathBuf> {
    // Cargo's published executable can still be its rustc fallback. Direct sync
    // updates the worker's Plan.binary, so the baseline must be that exact image.
    let mut found = Vec::new();
    let prefix = format!("{}-", config.bin.replace('-', "_"));
    for entry in walkdir::WalkDir::new(config.workspace_root.join("target/rustd"))
        .min_depth(4)
        .max_depth(4)
    {
        let entry = entry?;
        if entry.file_name() == "bin"
            && entry
                .path()
                .parent()
                .and_then(|p| p.file_name())
                .is_some_and(|n| n.to_string_lossy().starts_with(&prefix))
            && entry.file_type().is_file()
        {
            found.push((entry.metadata()?.modified()?, entry.into_path()));
        }
    }
    found.sort_by_key(|p| p.0);
    found
        .pop()
        .map(|p| p.1)
        .context("captured worker executable missing")
}
fn start_fast(
    config: &Config,
    session_home: &std::path::Path,
    seq: u64,
    floor: u64,
    cancel: Arc<std::sync::atomic::AtomicBool>,
) -> Result<Fast> {
    let control = session_home.join(format!("promote-{seq}"));
    fs::create_dir_all(control.parent().unwrap())?;
    fs::write(&control, "0")?;
    let mut cfg = rustd::Config::new(&config.workspace_root, compiler_path()?, &config.bin)?;
    cfg.home = session_home.join(format!("d-{seq}"));
    cfg.memory_min_bytes = floor;
    cfg.cancel = Some(cancel);
    if let Some(value) = std::env::var_os("REFRESH_RUSTD_ENV") {
        let env: BTreeMap<String, String> = serde_json::from_str(&value.to_string_lossy())?;
        cfg.extra_env = env.into_iter().map(|(k, v)| (k.into(), v.into())).collect();
    }
    cfg.extra_env.insert(
        "RUSTD_PROMOTE_CONTROL".into(),
        control.as_os_str().to_owned(),
    );
    cfg.package = Some(config.package.clone());
    cfg.edit = config.edit.clone();
    let mut client = rustd::Client::new(cfg);
    client.start()?;
    let binary = native_binary(config)?;
    // Fresh reset capture must be a native baseline, never a stale splice from a prior daemon.
    ensure!(
        crate::elf::u64_at(&fs::read(&binary)?, 32)? == 64,
        "capture retained a spliced binary; native recapture required"
    );
    let extractor = ManifestExtractor::new(&binary, config.allow_tracing_metadata)?;
    Ok(Fast {
        client,
        extractor,
        manifest: State::default(),
        control,
        promotions: 0,
    })
}
fn reason_text(reason: &Reason) -> String {
    match reason {
        Reason::Layout { path } => format!("types or layout changed in {}", path.display()),
        Reason::Async { function, .. } => format!("async function {function} changed"),
        Reason::FileSet { .. } => "source files added or removed".into(),
        Reason::MacroBody { function, .. } => format!("macro code in {function} changed"),
        Reason::NewComplexFn { function, .. } => {
            format!("new function {function} needs a fresh process")
        }
        _ => format!("{} changed", reason.code()),
    }
}
fn user_error(text: &str) -> bool {
    [
        "E0004", "E0005", "E0023", "E0046", "E0057", "E0061", "E0063", "E0308", "E0407", "E0425",
        "E0432", "E0559", "E0560", "E0583", "E0599",
    ]
    .iter()
    .any(|code| text.contains(code))
}
impl Pipeline {
    pub fn new(
        config: Config,
        sources: Sources,
        fence: Fence,
        sender: crossbeam_channel::Sender<Update>,
    ) -> Result<Self> {
        let floor = std::env::var("REFRESH_MEMORY_MIN_BYTES")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(2 * 1024 * 1024 * 1024);
        let runtime = Supervisor::new(
            &config.workspace_root,
            &config.target_directory.join("refresh"),
            config.args.clone(),
            config.after_patch_hook,
            config.bin.clone().into(),
        )?;
        let session_home = tempfile::Builder::new()
            .prefix("session-")
            .tempdir_in(config.target_directory.join("refresh"))?
            .keep();
        let workspace = if config.check || config.strict {
            Some(Workspace::copy(&config)?)
        } else {
            None
        };
        Ok(Self {
            config,
            runtime,
            fast: None,
            reset: None,
            reset_seq: 0,
            checked: sources.clone(),
            sources,
            checked_available: false,
            workspace,
            checker: None,
            guard: Guard::new(),
            generation: 0,
            floor,
            sender,
            fence,
            unchecked: BTreeSet::new(),
            hook_missing: false,
            recovery_restart: false,
            view_dirty: BTreeSet::new(),
            session_home,
        })
    }
    fn event(&self, name: &'static str, message: impl Into<String>) {
        let _ = self.sender.send(Update::Event {
            name,
            message: message.into(),
            revision: self.fence.revision,
        });
    }
    pub fn setup(&mut self) -> Result<()> {
        match start_fast(
            &self.config,
            &self.session_home,
            0,
            self.floor,
            self.fence.stopping.clone(),
        ) {
            Ok(fast) => {
                self.runtime.start(&native_binary(&self.config)?)?;
                self.event(
                    "compiler_ready",
                    fast.client.config.home.display().to_string(),
                );
                self.fast = Some(fast);
            }
            Err(error) => {
                self.event("fast_unavailable", format!("{error:#}"));
                let result = fallback::build(
                    &self.config,
                    &self.config.workspace_root,
                    &self.fence,
                    self.floor,
                )?;
                ensure!(result.success, "{}", result.diagnostic);
                self.runtime
                    .start(&result.executable.context("stock build has no executable")?)?;
            }
        }
        let reply = agent::Client::new(&self.runtime.socket).ping()?;
        self.hook_missing = self.config.after_patch_hook
            && reply
                .counters
                .get("callback_present")
                .and_then(|v| v.as_bool())
                != Some(true);
        if self.hook_missing {
            self.event(
                "hook_unavailable",
                "after-patch callback is missing; edits require a restart",
            );
        }
        if let Some(workspace) = &self.workspace {
            let mut check = Checker::start(&self.config, workspace, self.fence.clone(), self.floor);
            let result = check.finish()?;
            ensure!(result.success, "initial cargo check: {}", result.diagnostic);
            self.checked_available = true;
        }
        self.event(
            "ready",
            format!("application pid {}", self.runtime.pid().unwrap_or(0)),
        );
        self.sender.send(Update::Ready)?;
        Ok(())
    }
    fn quarantine(&mut self) {
        // Dropping an owned client stops the daemon and workers before stock starts.
        self.fast = None;
        if let Some(reset) = &self.reset {
            reset
                .cancel
                .store(true, std::sync::atomic::Ordering::SeqCst);
        }
    }
    fn begin_reset(&mut self) {
        if self.reset.is_some() || self.fast.is_some() || !self.fence.current() {
            return;
        }
        self.reset_seq += 1;
        let seq = self.reset_seq;
        let config = self.config.clone();
        let floor = self.floor;
        let session_home = self.session_home.clone();
        let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let task_cancel = cancel.clone();
        self.reset = Some(Reset {
            revision: self.fence.revision,
            cancel,
            task: thread::spawn(move || {
                start_fast(&config, &session_home, seq, floor, task_cancel)
            }),
        });
        self.event(
            "worker_reset_start",
            "warming a fresh compiler in the background",
        );
    }
    fn publish(&mut self, job: &Job, path: SavePath, reason: String, applied: bool) -> Result<()> {
        let notice = job.phases.get(&Phase::Notice).copied().unwrap_or(0.0);
        if applied {
            checker::apply(&mut self.sources, &job.changes);
            self.unchecked
                .extend(job.changes.changes.iter().map(|c| c.path.clone()));
            self.generation += 1;
        }
        self.sender.send(Update::Save {
            event: SaveEvent {
                path,
                reason,
                latency_ms: monotonic_seconds()? * 1000.0 - notice,
                phases: job.phases.clone(),
                revision: job.changes.revision,
                files: job.changes.changes.iter().map(|c| c.path.clone()).collect(),
            },
            applied: applied.then(|| job.changes.clone()),
            generation: self.generation,
        })?;
        Ok(())
    }
    fn stock(&mut self, job: &mut Job, reason: String) -> Result<()> {
        self.quarantine();
        if let Some((mut check, _)) = self.checker.take() {
            check.cancel();
        }
        let manifest_change = job
            .changes
            .changes
            .iter()
            .any(|c| c.kind == PathKind::Manifest);
        if manifest_change {
            self.config = Config::load(self.config.invocation.clone())?;
            self.runtime
                .configure(self.config.after_patch_hook, self.config.bin.clone().into());
            self.sender.send(Update::Reconfigure(
                Box::new(self.config.clone()),
                job.changes.revision,
            ))?;
        }
        let stock_only = !job.phases.contains_key(&Phase::BuildStart);
        if stock_only {
            mark(&mut job.phases, Phase::BuildStart);
        }
        mark(&mut job.phases, Phase::StockStart);
        let result = fallback::build(
            &self.config,
            &self.config.workspace_root,
            &self.fence,
            self.floor,
        )?;
        mark(&mut job.phases, Phase::StockEnd);
        if stock_only {
            mark(&mut job.phases, Phase::BuildEnd);
        }
        if !self.fence.current() {
            return Ok(());
        }
        if !result.success {
            self.publish(job, SavePath::Error, result.diagnostic, false)?;
            return Ok(());
        }
        self.runtime
            .start(&result.executable.context("stock binary missing")?)?;
        mark(&mut job.phases, Phase::SpawnEnd);
        if manifest_change {
            let reply = agent::Client::new(&self.runtime.socket).ping()?;
            self.hook_missing = self.config.after_patch_hook
                && reply
                    .counters
                    .get("callback_present")
                    .and_then(|v| v.as_bool())
                    != Some(true);
        }
        self.recovery_restart = true;
        self.publish(job, SavePath::Stock, reason, true)?;
        // Stock build ran borrow checking and compiled the current source snapshot.
        self.advance_checked(&job.changes);
        self.event("check_ok", "stock build passed borrow checking");
        if manifest_change {
            if self.workspace.is_some() {
                self.workspace = Some(Workspace::copy(&self.config)?);
                self.view_dirty.clear();
            }
            self.sender.send(Update::Reconfigure(
                Box::new(self.config.clone()),
                job.changes.revision,
            ))?;
        }
        self.begin_reset();
        Ok(())
    }
    fn restart_candidate(
        &mut self,
        job: &mut Job,
        build: &rustd::Build,
        patch: crate::extractor::PreparedPatch,
        reason: String,
    ) -> Result<()> {
        if !self.fence.current() {
            return Ok(());
        }
        self.runtime.start(&build.candidate_path)?;
        mark(&mut job.phases, Phase::SpawnEnd);
        // The restarted ELF already contains this complete cumulative patch.
        // Keep the native baseline and mark its pages applied offline: subsequent
        // append-only saves send only genuinely new pages. No inherited mmap is
        // overwritten or adopted by the agent.
        self.fast
            .as_mut()
            .context("compiler baseline unavailable")?
            .extractor
            .commit(patch)?;
        self.recovery_restart = false;
        self.publish(job, SavePath::Restart, reason, true)
    }
    fn snapshots(&self, changes: &ChangeSet) -> (Vec<FileSnapshot>, Vec<FileSnapshot>) {
        let mut before = Vec::new();
        let mut after = Vec::new();
        for change in &changes.changes {
            if let Some(bytes) = self.sources.get(&change.path) {
                before.push(FileSnapshot::new(&change.path, bytes.to_vec()));
            }
            if let Some(bytes) = &change.content {
                after.push(FileSnapshot::new(&change.path, bytes.to_vec()));
            }
        }
        (before, after)
    }
    fn advance_checked(&mut self, changes: &ChangeSet) {
        for path in &self.unchecked {
            if let Some(bytes) = self.sources.get(path) {
                self.checked.insert(path.clone(), bytes.clone());
            } else {
                self.checked.remove(path);
            }
        }
        checker::apply(&mut self.checked, changes);
        self.checked_available = true;
        self.unchecked.clear();
    }
    pub fn process(&mut self, mut job: Job) -> Result<()> {
        self.fence.revision = job.changes.revision;
        if let Some((mut old, _)) = self.checker.take() {
            old.cancel();
        }
        if !self.fence.current() {
            self.event("superseded", "newer save already queued");
            return Ok(());
        }
        // A rejected buffer may round-trip back to the applied bytes, producing
        // an empty change set. Restore paths absent from this new delta in the
        // private checker tree before checking that identical save.
        if let Some(workspace) = &self.workspace {
            let current: BTreeSet<_> = job.changes.changes.iter().map(|c| c.path.clone()).collect();
            for path in self.view_dirty.difference(&current) {
                workspace.write(path, self.sources.get(path).map(|b| b.as_ref()))?;
            }
            self.view_dirty = current;
        }
        if job.changes.disposition == Disposition::Skip {
            if let Some(workspace) = &self.workspace {
                self.checker = Some((
                    Checker::start(&self.config, workspace, self.fence.clone(), self.floor),
                    job.changes.clone(),
                ));
            }
            return self.publish(&job, SavePath::Skip, String::new(), false);
        }
        if job
            .changes
            .changes
            .iter()
            .any(|c| c.kind == PathKind::Manifest)
        {
            return self.stock(
                &mut job,
                "Cargo configuration changed; recapturing the project".into(),
            );
        }
        if job.changes.changes.iter().any(|c| {
            c.kind != PathKind::Rs
                || !self
                    .config
                    .frontier_roots
                    .iter()
                    .any(|root| c.path.starts_with(root))
        }) {
            return self.stock(
                &mut job,
                "file is outside the editable compiler frontier".into(),
            );
        }
        if let Some(workspace) = &self.workspace {
            workspace.update(&job.changes)?;
        }
        let (before, after) = self.snapshots(&job.changes);
        let admission = self.guard.admit(&before, &after);
        mark(&mut job.phases, Phase::Gate);
        let mut admission_checked = false;
        if let Admission::Reject { rule, detail } = admission {
            // EXP-119 intentionally errs toward waiting. A frozen stock check
            // can prove a saved, syntactically complete buffer valid without
            // exposing a rejected buffer to rustd (e.g. a local `weight` whose
            // spelling prefixes `weighted_total`). Parse failures still wait.
            if rule != AdmissionRule::Parse
                && let Some(workspace) = &self.workspace
            {
                let mut check =
                    Checker::start(&self.config, workspace, self.fence.clone(), self.floor);
                let result = check.finish()?;
                if !self.fence.current() {
                    return Ok(());
                }
                if !result.success {
                    return self.publish(
                        &job,
                        SavePath::Error,
                        format!("borrowck: {}", result.diagnostic),
                        false,
                    );
                }
                self.event(
                    "admission_checked",
                    "stock check proved the saved buffer valid",
                );
                self.advance_checked(&job.changes);
                admission_checked = true;
                self.event("check_ok", "stock check proved the saved buffer valid");
            } else {
                return self.publish(
                    &job,
                    SavePath::Waiting,
                    format!("waiting for valid code: {detail}"),
                    false,
                );
            }
        }
        let class = self.guard.classify(&before, &after);
        mark(&mut job.phases, Phase::Classify);
        if !admission_checked && let Some(workspace) = &self.workspace {
            let mut check = Checker::start(&self.config, workspace, self.fence.clone(), self.floor);
            if self.config.strict {
                let result = check.finish()?;
                if !self.fence.current() {
                    return Ok(());
                }
                if !result.success {
                    return self.publish(
                        &job,
                        SavePath::Error,
                        format!("borrowck: {}", result.diagnostic),
                        false,
                    );
                }
                self.advance_checked(&job.changes);
            } else {
                self.checker = Some((check, job.changes.clone()));
            }
        }
        if self.fast.is_none() {
            return self.stock(&mut job, "compiler is recovering; using Cargo".into());
        }
        let restart_reason = match class {
            Class::Restart { reason } => Some(reason_text(&reason)),
            Class::Stock { reason } => return self.stock(&mut job, reason_text(&reason)),
            Class::LiveEligible { .. } => {
                if self.hook_missing {
                    Some("after-patch callback is unavailable".into())
                } else if self.recovery_restart {
                    Some("compiler recovered; establishing the live baseline".into())
                } else {
                    None
                }
            }
        };
        let fast = self.fast.as_mut().unwrap();
        let promote = restart_reason.is_some()
            && fast.promotions < 10
            && fs::metadata(&self.runtime.executable)?.len() < 256 * 1024 * 1024;
        fs::write(&fast.control, if promote { "1" } else { "0" })?;
        mark(&mut job.phases, Phase::BuildStart);
        let build = fast.client.build();
        mark(&mut job.phases, Phase::BuildEnd);
        if !self.fence.current() {
            self.event("superseded", "compiler result superseded by a newer save");
            // Deltas and semantic promotions must be consumed before any later build.
            if let Ok(build) = build {
                let fast = self.fast.as_mut().unwrap();
                let _ = fast.manifest.update(&build.manifest);
                if promote {
                    self.fast = None;
                }
            }
            return Ok(());
        }
        let build = match build {
            Ok(build) => build,
            Err(error) => {
                let detail = format!("{error:#}");
                if user_error(&detail) {
                    return self.publish(&job, SavePath::Error, detail, false);
                }
                return self.stock(
                    &mut job,
                    format!(
                        "compiler failed: {}",
                        detail.lines().next().unwrap_or("unknown error")
                    ),
                );
            }
        };
        let result = (|| -> Result<_> {
            let fast = self.fast.as_mut().unwrap();
            ensure!(
                fast.manifest.update(&build.manifest)?,
                "stale compiler manifest"
            );
            let candidate = Image::parse(fs::read(&build.candidate_path)?, false)?;
            mark(&mut job.phases, Phase::ExtractStart);
            let patch = fast.extractor.extract(&fast.manifest, &candidate)?;
            mark(&mut job.phases, Phase::ExtractEnd);
            Ok(patch)
        })();
        let patch = match result {
            Ok(p) => p,
            Err(error) => return self.stock(&mut job, format!("native relink required: {error}")),
        };
        if let Some(reason) = restart_reason {
            if !promote {
                if fs::metadata(&build.candidate_path)?.len() >= 256 * 1024 * 1024 {
                    return self.stock(
                        &mut job,
                        format!("{reason}; executable needs a native rebase"),
                    );
                }
                return self.restart_candidate(
                    &mut job,
                    &build,
                    patch,
                    format!("{reason}; retaining the current compiler epoch"),
                );
            }
            if !self.fence.current() {
                return Ok(());
            }
            self.runtime.start(&build.candidate_path)?;
            mark(&mut job.phases, Phase::SpawnEnd);
            let fast = self.fast.as_mut().unwrap();
            fast.extractor = ManifestExtractor::new(
                &self.runtime.executable,
                self.config.allow_tracing_metadata,
            )?;
            let catalog = build.candidate_path.with_extension("promoted126.json");
            if catalog.exists() {
                let symbols: Vec<serde_json::Value> = serde_json::from_slice(&fs::read(catalog)?)?;
                for symbol in symbols {
                    if symbol["text"] == true && symbol["size"].as_u64().unwrap_or(0) >= 5 {
                        fast.extractor.add_function(
                            symbol["address"]
                                .as_u64()
                                .context("invalid promotion catalog")?,
                        );
                    }
                }
            }
            fast.manifest = State::default();
            fast.promotions += 1;
            self.recovery_restart = false;
            self.publish(&job, SavePath::Restart, reason, true)?;
            return Ok(());
        }
        if !self.fence.current() {
            return Ok(());
        }
        mark(&mut job.phases, Phase::PatchSent);
        let reply = (|| -> Result<_> {
            let mut agent = agent::Client::new(&self.runtime.socket);
            agent.ping()?;
            agent.apply(&patch.records)
        })();
        mark(&mut job.phases, Phase::PatchAck);
        match reply {
            Ok(reply) => {
                self.sender.send(Update::AgentReply {
                    reply: reply.clone(),
                    revision: self.fence.revision,
                })?;
                match agent::action(&reply) {
                    agent::Action::Applied => {
                        self.fast.as_mut().unwrap().extractor.commit(patch)?;
                        self.publish(&job, SavePath::Live, String::new(), true)?;
                    }
                    agent::Action::Intact => {
                        self.publish(
                            &job,
                            SavePath::Error,
                            format!("patch rolled back; app intact: {}", reply.detail()),
                            false,
                        )?;
                    }
                    _ => {
                        return self.restart_candidate(
                            &mut job,
                            &build,
                            patch,
                            format!("agent refused patch: {}", reply.detail()),
                        );
                    }
                }
            }
            Err(error) => {
                self.runtime.stop();
                return self.restart_candidate(
                    &mut job,
                    &build,
                    patch,
                    format!("agent acknowledgement failed: {error}"),
                );
            }
        }
        Ok(())
    }
    pub fn failure(&mut self, job: &Job, error: &anyhow::Error) -> Result<()> {
        if self.fence.current() {
            self.publish(job, SavePath::Error, format!("{error:#}"), false)
        } else {
            self.event("superseded", "operation cancelled for a newer save");
            Ok(())
        }
    }
    pub fn poll(&mut self) -> Result<()> {
        if let Some(status) = self.runtime.poll_exit()? {
            self.event("application_exit", format!("application exited: {status}"));
        }
        if let Some(fast) = self.fast.as_mut()
            && let Err(error) = fast.client.guard_memory()
        {
            self.fast = None;
            self.event("fast_unavailable", error.to_string());
        }
        if let Some(reset) = &self.reset
            && reset.task.is_finished()
        {
            let reset = self.reset.take().unwrap();
            let result = reset
                .task
                .join()
                .map_err(|_| anyhow::anyhow!("compiler reset panicked"))?;
            if reset.revision != self.fence.latest.load(std::sync::atomic::Ordering::SeqCst) {
                drop(result);
                self.event(
                    "worker_reset_superseded",
                    "save arrived during compiler reset",
                );
                self.begin_reset();
            } else {
                match result {
                    Ok(fast) => {
                        self.fast = Some(fast);
                        self.event("worker_reset", "fast path ready");
                    }
                    Err(error) => self.event(
                        "worker_reset_failed",
                        format!("fast path unavailable: {error:#}"),
                    ),
                }
            }
        }
        if self
            .checker
            .as_ref()
            .is_some_and(|(check, _)| check.finished())
        {
            let (mut check, changes) = self.checker.take().unwrap();
            let result = check.finish();
            if check.revision != self.fence.latest.load(std::sync::atomic::Ordering::SeqCst) {
                return Ok(());
            }
            match result {
                Ok(result) if result.success => {
                    self.advance_checked(&changes);
                    self.event("check_ok", "borrow check passed");
                }
                Ok(result) => {
                    self.event("tainted", format!("⚠ borrowck: {}", result.diagnostic));
                    if self.checked_available {
                        // A borrow-check failure does not invalidate the warm
                        // compiler. Keep it quarantined from the reverted stock
                        // runtime by requiring a candidate restart on the next
                        // valid save, instead of discarding the whole worker.
                        if let Some(reset) = &self.reset {
                            reset
                                .cancel
                                .store(true, std::sync::atomic::Ordering::SeqCst);
                        }
                        self.recovery_restart = true;
                        let workspace = self
                            .workspace
                            .as_ref()
                            .context("checked snapshot unavailable")?;
                        // Restore only the private checker tree, never the user's working tree.
                        let paths: BTreeSet<_> = self
                            .unchecked
                            .iter()
                            .cloned()
                            .chain(changes.changes.iter().map(|c| c.path.clone()))
                            .collect();
                        for path in &paths {
                            workspace.write(path, self.checked.get(path).map(|v| v.as_ref()))?;
                        }
                        let result = fallback::build(
                            &self.config,
                            &workspace.root,
                            &self.fence,
                            self.floor,
                        )?;
                        if self.fence.current() && result.success {
                            self.runtime
                                .start(&result.executable.context("revert binary missing")?)?;
                            let restored = paths
                                .iter()
                                .map(|p| (p.clone(), self.checked.get(p).cloned()))
                                .collect();
                            for path in paths {
                                if let Some(bytes) = self.checked.get(&path) {
                                    self.sources.insert(path, bytes.clone());
                                } else {
                                    self.sources.remove(&path);
                                }
                            }
                            self.sender.send(Update::Reverted {
                                paths: restored,
                                revision: self.fence.revision,
                            })?;
                            self.unchecked.clear();
                            self.event("revert","last checked snapshot rebuilt and restarted; editor files preserved");
                        } else if !result.success {
                            self.event("revert_failed", result.diagnostic);
                        }
                    }
                }
                Err(error) => self.event("check_failed", error.to_string()),
            }
        }
        Ok(())
    }
}
impl Drop for Pipeline {
    fn drop(&mut self) {
        if let Some((mut check, _)) = self.checker.take() {
            check.cancel();
        }
        self.runtime.stop();
        self.fast = None;
        if let Some(reset) = self.reset.take() {
            reset
                .cancel
                .store(true, std::sync::atomic::Ordering::SeqCst);
            let _ = reset.task.join();
        }
    }
}
