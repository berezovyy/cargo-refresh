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
    manifest::{AsyncRuntime, State},
    rustd,
    supervisor::Supervisor,
};
use anyhow::{Context, Result, ensure};
use refresh_guard::{Admission, AdmissionRule, Class, FileSnapshot, Guard, Reason};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    os::unix::fs::MetadataExt,
    path::PathBuf,
    sync::Arc,
    thread::{self, JoinHandle},
    time::{Duration, Instant},
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
    async_runtime: AsyncRuntime,
    async_owners: Vec<refresh_guard::ChangedFn>,
    control: PathBuf,
    promotions: u32,
    pressure: bool,
    /// Compiler image, before the runtime snapshot copy expands sparse holes.
    binary: PathBuf,
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
    triggers: crate::triggers::Triggers,
    last_save: Instant,
    epoch_idle_safe: bool,
    memory_poll: Instant,
    memory_recovery: crate::memory::Recovery,
}
fn mark(phases: &mut BTreeMap<Phase, f64>, phase: Phase) {
    phases.insert(phase, monotonic_seconds().unwrap_or(0.0) * 1000.0);
}
fn executable_bytes(path: &std::path::Path) -> Result<u64> {
    // Promoted ELFs contain a sparse gap for the reserved code region. Its
    // virtual/file offsets do not consume the retained executable budget.
    fs::metadata(path)?
        .blocks()
        .checked_mul(512)
        .context("executable allocation overflow")
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
fn async_certificates_setting(value: Option<&std::ffi::OsStr>) -> bool {
    value.is_none_or(|value| value == "1")
}
fn async_certificates_enabled() -> bool {
    async_certificates_setting(std::env::var_os("REFRESH_ASYNC_CERTS").as_deref())
}
fn start_fast(
    config: &Config,
    session_home: &std::path::Path,
    seq: u64,
    floor: u64,
    cancel: Arc<std::sync::atomic::AtomicBool>,
) -> Result<Fast> {
    let support = config
        .support
        .as_ref()
        .context("live patching disabled; using stock Cargo")?;
    ensure!(
        crate::install::memory_allows(rustd::memory_available()?, support.warm_peak_bytes),
        "insufficient memory for compiler warm-up"
    );
    let control = session_home.join(format!("promote-{seq}"));
    fs::create_dir_all(control.parent().unwrap())?;
    fs::write(&control, "0")?;
    let mut cfg = rustd::Config::new(
        &config.workspace_root,
        support.root.join("rustd"),
        &config.bin,
    )?;
    cfg.home = session_home.join(format!("d-{seq}"));
    cfg.memory_min_bytes = floor;
    cfg.cancel = Some(cancel);
    if let Some(value) = std::env::var_os("REFRESH_RUSTD_ENV") {
        let env: BTreeMap<String, String> = serde_json::from_str(&value.to_string_lossy())?;
        cfg.extra_env = env.into_iter().map(|(k, v)| (k.into(), v.into())).collect();
    }
    cfg.extra_env.insert(
        "RUSTD_ABI_FACTS_DIR".into(),
        support.bundle_dir().as_os_str().to_owned(),
    );
    cfg.extra_env.insert(
        "RUSTD_PROMOTE_CONTROL".into(),
        control.as_os_str().to_owned(),
    );
    cfg.extra_env
        .insert("RUSTD_PROMOTION_CATALOG".into(), "delta-v1".into());
    cfg.extra_env.insert(
        "RUSTD_ASYNC_CERTS".into(),
        if async_certificates_enabled() {
            "1"
        } else {
            "0"
        }
        .into(),
    );
    cfg.package = Some(config.package.clone());
    cfg.edit = config.edit.clone();
    cfg.features = config.features.clone();
    let mut client = rustd::Client::new(cfg);
    client.start()?;
    eprintln!(
        "[refresh] compiler ready (editing: {}{})",
        config.package,
        if config.edit.is_empty() {
            String::new()
        } else {
            format!(", {}", config.edit.join(", "))
        }
    );
    eprintln!(
        "[refresh] tip: live patches apply to {}; edits to other crates use cargo. Add more with --edit <crate>.",
        config.package
    );
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
        async_runtime: AsyncRuntime::default(),
        async_owners: Vec::new(),
        control,
        promotions: 0,
        pressure: false,
        binary,
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
            .unwrap_or(crate::memory::RESERVE);
        let mut runtime = Supervisor::new(
            &config.workspace_root,
            &config.target_directory.join("refresh"),
            config.args.clone(),
            config.after_patch_hook,
            config.bin.clone().into(),
        )?;
        runtime.configure_agent(
            config
                .support
                .as_ref()
                .map(|s| s.root.join("librefresh.so"))
                .as_deref(),
        );
        let session_home = tempfile::Builder::new()
            .prefix("session-")
            .tempdir_in(config.target_directory.join("refresh"))?
            .keep();
        let workspace = if config.support.is_some() && (config.check || config.strict) {
            Some(Workspace::copy(&config)?)
        } else {
            None
        };
        Ok(Self {
            triggers: crate::triggers::Triggers::new(
                config.after_patch_touch.clone(),
                config.after_patch_http.as_deref(),
            )?,
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
            last_save: Instant::now(),
            epoch_idle_safe: false,
            memory_poll: Instant::now(),
            memory_recovery: crate::memory::Recovery::new(Instant::now()),
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
                self.recovery_restart = true;
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
                self.checked_available = true;
            }
        }
        if self.config.support.is_some() {
            let reply = agent::Client::new(&self.runtime.socket).ping()?;
            self.hook_missing = self.config.after_patch_hook
                && reply
                    .counters
                    .get("callback_present")
                    .and_then(|v| v.as_bool())
                    != Some(true);
            if self.hook_missing && !self.triggers.configured() {
                self.event(
                    "hook_unavailable",
                    "after-patch callback is missing; edits require a restart",
                );
            }
        }
        if self.fast.is_some()
            && let Some(workspace) = &self.workspace
        {
            let mut check = Checker::start(&self.config, workspace, self.fence.clone(), self.floor);
            let result = check.finish()?;
            ensure!(result.success, "initial cargo check: {}", result.diagnostic);
            self.checked_available = true;
        }
        // The public ready event must follow dependency-watch installation.
        self.refresh_inputs()?;
        self.event(
            "ready",
            format!("application pid {}", self.runtime.pid().unwrap_or(0)),
        );
        eprintln!(
            "[refresh] {} running (pid {}). Save a file to see it update.",
            self.config.bin,
            self.runtime.pid().unwrap_or(0)
        );
        self.sender.send(Update::Ready)?;
        Ok(())
    }
    fn refresh_inputs(&mut self) -> Result<()> {
        crate::dep_info::extend(&mut self.config)?;
        self.sender.send(Update::Reconfigure(
            Box::new(self.config.clone()),
            self.fence.revision,
        ))?;
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
        if self.config.support.is_none()
            || self.reset.is_some()
            || self.fast.is_some()
            || !self.fence.current()
            || !self.memory_recovery.can_start(Instant::now())
        {
            return;
        }
        let peak = self.config.support.as_ref().unwrap().warm_peak_bytes;
        if !crate::memory::sample().is_ok_and(|sample| {
            crate::memory::allows(sample.available, peak) && !sample.pressured(self.floor)
        }) {
            return;
        }
        self.memory_recovery.cooldown(Instant::now());
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
        if applied && let Some(fast) = self.fast.as_mut() {
            fast.async_runtime.commit(&fast.async_owners);
            fast.async_owners.clear();
        }
        let notice = job.phases.get(&Phase::Notice).copied().unwrap_or(0.0);
        if applied {
            self.epoch_idle_safe = true;
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
            let watched_inputs = self.config.input_files.clone();
            self.config = Config::load(self.config.invocation.clone())?;
            crate::install::prepare(&mut self.config);
            // Keep already captured inputs observable during the stock build.
            // Installation policy above uses only the new Cargo configuration.
            self.config.input_files.extend(watched_inputs);
            self.config.input_files.sort();
            self.config.input_files.dedup();
            self.runtime.configure_agent(
                self.config
                    .support
                    .as_ref()
                    .map(|s| s.root.join("librefresh.so"))
                    .as_deref(),
            );
            self.triggers = crate::triggers::Triggers::new(
                self.config.after_patch_touch.clone(),
                self.config.after_patch_http.as_deref(),
            )?;
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
        if manifest_change && self.config.support.is_some() {
            let reply = agent::Client::new(&self.runtime.socket).ping()?;
            self.hook_missing = self.config.after_patch_hook
                && reply
                    .counters
                    .get("callback_present")
                    .and_then(|v| v.as_bool())
                    != Some(true);
        }
        self.recovery_restart = true;
        // A client may save a newly included file immediately after the save
        // event. Queue the new watch set first on the same ordered channel.
        self.refresh_inputs()?;
        self.publish(job, SavePath::Stock, reason, true)?;
        // Stock build ran borrow checking and compiled the current source snapshot.
        self.advance_checked(&job.changes);
        self.event("check_ok", "stock build passed borrow checking");
        if manifest_change {
            // A session can gain qualified support after its toolchain/config
            // changes. Recreate checks from the new policy before warming rustd.
            self.workspace =
                if self.config.support.is_some() && (self.config.check || self.config.strict) {
                    Some(Workspace::copy(&self.config)?)
                } else {
                    None
                };
            self.view_dirty.clear();
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
        self.fast.as_mut().unwrap().pressure = false;
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
        mark(&mut job.phases, Phase::WorkerStart);
        self.last_save = Instant::now();
        self.epoch_idle_safe = false;
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
        if (self.config.support.is_none() || self.fast.is_none())
            && job.changes.disposition != Disposition::Skip
        {
            let reason = if self.config.support.is_none() {
                "live patching disabled; using normal cargo builds + restart"
            } else {
                "compiler unavailable or recovering; using normal cargo builds + restart"
            };
            return self.stock(&mut job, reason.into());
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
        if job.changes.changes.iter().any(|c| {
            self.config
                .stock_frontier_roots
                .iter()
                .any(|root| c.path.starts_with(root))
        }) {
            return self.stock(&mut job,
                "edited crate has callers outside the compiler frontier; stock rebuild required for generic, inline and const dependencies".into());
        }
        if let Some(workspace) = &self.workspace {
            workspace.update(&job.changes)?;
        }
        mark(&mut job.phases, Phase::MirrorEnd);
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
        if job.changes.changes.iter().any(|change| {
            crate::dep_info::includes_changed(
                self.sources
                    .get(&change.path)
                    .map_or(&[], |bytes| bytes.as_ref()),
                change.content.as_deref().unwrap_or(&[]),
            )
        }) {
            return self.stock(
                &mut job,
                "include macro inputs changed; refreshing rustc dep-info and compiler capture"
                    .into(),
            );
        }
        let certificates = async_certificates_enabled();
        let (class, async_owners) = if certificates {
            self.guard.classify_with_async_certificates(&before, &after)
        } else {
            (self.guard.classify(&before, &after), Vec::new())
        };
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
        let mut restart_reason = match class {
            Class::Restart { reason } => Some(reason_text(&reason)),
            Class::Stock { reason } => return self.stock(&mut job, reason_text(&reason)),
            Class::LiveEligible { .. } => {
                if self.hook_missing && !self.triggers.configured() {
                    Some("after-patch callback is unavailable".into())
                } else if self.recovery_restart {
                    Some("compiler recovered; establishing the live baseline".into())
                } else {
                    None
                }
            }
        };
        let fast = self.fast.as_mut().unwrap();
        fast.async_owners = async_owners;
        let promote = restart_reason.is_some()
            && fast.promotions < 10
            && crate::elf::loaded_bytes(&self.runtime.executable)? < 256 * 1024 * 1024
            && executable_bytes(&fast.binary)? < 256 * 1024 * 1024;
        fs::write(&fast.control, if promote { "1" } else { "0" })?;
        mark(&mut job.phases, Phase::BuildStart);
        let build = fast.client.build_with_async_certificates(certificates);
        mark(&mut job.phases, Phase::BuildEnd);
        if !self.fence.current() {
            self.event("superseded", "compiler result superseded by a newer save");
            // Deltas and semantic promotions must be consumed before any later build.
            if let Ok(build) = build {
                if build.native_rebase || promote {
                    self.quarantine();
                    self.recovery_restart = true;
                } else {
                    let fast = self.fast.as_mut().unwrap();
                    let _ = fast.manifest.update(&build.manifest);
                    if certificates {
                        fast.async_runtime
                            .observe_certificates(fast.manifest.async_certificates.as_deref());
                    }
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
                if detail.contains("effective available")
                    || detail.contains("effective memory budget")
                {
                    self.memory_recovery.cooldown(Instant::now());

                    self.event(
                        "fast_unavailable",
                        format!("[refresh] {detail}. Retrying when memory recovers."),
                    );
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
        if build.native_rebase {
            // A native link changed every address. No old page, redirect, or
            // retained-state claim may cross this fresh process boundary.
            let extractor = match ManifestExtractor::new(
                &build.candidate_path,
                self.config.allow_tracing_metadata,
            ) {
                Ok(extractor) => extractor,
                Err(error) => {
                    return self.stock(
                        &mut job,
                        format!("native rebase validation failed: {error:#}"),
                    );
                }
            };
            if !self.fence.current() {
                self.quarantine();
                self.recovery_restart = true;
                return Ok(());
            }
            if let Err(error) = self.runtime.start(&build.candidate_path) {
                return self.stock(&mut job, format!("native rebase restart failed: {error:#}"));
            }
            let fast = self.fast.as_mut().unwrap();
            fast.extractor = extractor;
            fast.manifest = State::default();
            fast.async_runtime = AsyncRuntime::default();
            fast.async_owners.clear();
            fast.binary = build.candidate_path;
            fast.promotions = 0;
            fast.pressure = false;
            self.recovery_restart = false;
            mark(&mut job.phases, Phase::SpawnEnd);
            return self.publish(
                &job,
                SavePath::Restart,
                "compiler requested a full native rebase".into(),
                true,
            );
        }
        // Fresh manifest != delivered edit. An untracked callback can produce
        // a successful zero-function splice of old code.
        // No compiler no-op classification is currently supported here.
        if let Err(error) = build.ensure_compiled_change(&self.fast.as_ref().unwrap().manifest) {
            self.event("compiler_no_change", error.to_string());
            return self.stock(&mut job, error.to_string());
        }
        self.fast.as_mut().unwrap().binary = build.candidate_path.clone();
        let result = (|| -> Result<_> {
            let fast = self.fast.as_mut().unwrap();
            ensure!(
                fast.manifest.update(&build.manifest)?,
                "stale compiler manifest"
            );
            if certificates {
                fast.async_runtime
                    .observe_certificates(fast.manifest.async_certificates.as_deref());
            }
            mark(&mut job.phases, Phase::ManifestEnd);
            // SAFETY: worker builds are serialized here. The completed candidate
            // is dropped at the end of this closure, before any next build/reset.
            let candidate = unsafe { Image::map_candidate(&build.candidate_path)? };
            mark(&mut job.phases, Phase::CandidateEnd);
            mark(&mut job.phases, Phase::ExtractStart);
            let patch = fast.extractor.extract(&fast.manifest, &candidate)?;
            mark(&mut job.phases, Phase::ExtractEnd);
            Ok(patch)
        })();
        let patch = match result {
            Ok(p) => p,
            Err(error) => return self.stock(&mut job, format!("native relink required: {error}")),
        };
        if certificates && restart_reason.is_none() {
            let fast = self.fast.as_ref().unwrap();
            if (!fast.async_owners.is_empty() || fast.async_runtime.has_changes())
                && let Err(reason) = fast.async_runtime.decide(&fast.async_owners)
            {
                restart_reason = Some(reason);
            }
        }
        if let Some(reason) = restart_reason {
            if !promote {
                if crate::elf::loaded_bytes(&build.candidate_path)? >= 256 * 1024 * 1024
                    || executable_bytes(&build.candidate_path)? >= 256 * 1024 * 1024
                {
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
            let promoted = (|| -> Result<()> {
                let catalog = build.candidate_path.with_extension("promoted126.json");
                // SAFETY: this private run inode is never overwritten or truncated.
                // Unlinking it at the next restart leaves existing mappings valid.
                let baseline = unsafe { Image::map_baseline(&self.runtime.executable)? };
                self.fast
                    .as_mut()
                    .unwrap()
                    .extractor
                    .promote(baseline, &fs::read(catalog)?)
            })();
            if let Err(error) = promoted {
                return self.stock(&mut job, format!("invalid promotion baseline: {error:#}"));
            }
            let fast = self.fast.as_mut().unwrap();
            fast.manifest = State::default();
            fast.async_runtime = AsyncRuntime::default();
            fast.async_owners.clear();
            fast.promotions += 1;
            fast.pressure = false;
            self.recovery_restart = false;
            self.publish(&job, SavePath::Restart, reason, true)?;
            return Ok(());
        }
        if !self.fence.current() {
            return Ok(());
        }
        mark(&mut job.phases, Phase::PatchSent);
        let reply = self.runtime.apply_patch(&patch);
        mark(&mut job.phases, Phase::PatchAck);
        match reply {
            Ok(reply) => {
                if let Some(fast) = self.fast.as_mut() {
                    fast.pressure |=
                        reply.counters.get("pressure").and_then(|v| v.as_bool()) == Some(true);
                }
                self.sender.send(Update::AgentReply {
                    reply: reply.clone(),
                    revision: self.fence.revision,
                })?;
                match agent::action(&reply) {
                    agent::Action::Applied => {
                        let notification = self.triggers.fire();
                        mark(&mut job.phases, Phase::TriggerEnd);
                        // APPLY already mutated the runtime, even if a newer
                        // save arrived during notification. Commit and publish
                        // that fact so the watcher can reconcile its hashes.
                        self.fast.as_mut().unwrap().extractor.commit(patch)?;
                        mark(&mut job.phases, Phase::CommitEnd);
                        if let Err(error) = notification {
                            self.event("after_patch_failed", format!("{error:#}"));
                            self.runtime.start(&build.candidate_path)?;
                            mark(&mut job.phases, Phase::SpawnEnd);
                            self.recovery_restart = false;
                            self.publish(
                                &job,
                                SavePath::Restart,
                                format!("after-patch notification failed: {error:#}"),
                                true,
                            )?;
                        } else {
                            self.publish(&job, SavePath::Live, String::new(), true)?;
                        }
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
        self.poll_memory();
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
                    Err(_) if reset.cancel.load(std::sync::atomic::Ordering::SeqCst) => {}
                    Err(error) => {
                        self.memory_recovery.cooldown(Instant::now());
                        self.event(
                            "worker_reset_failed",
                            format!("fast path unavailable: {error:#}"),
                        );
                    }
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
        self.poll_epoch()?;
        Ok(())
    }

    /// Idle and reset checks share the same cadence as blocking compiler work.
    /// Recovery needs five stable seconds and 512 MiB additional headroom; a
    /// failed/degraded attempt has a 30-second cooldown to prevent thrashing.
    fn poll_memory(&mut self) {
        if self.memory_poll.elapsed() < crate::memory::INTERVAL {
            return;
        }
        self.memory_poll = Instant::now();
        let sample = match crate::memory::sample() {
            Ok(sample) => sample,
            Err(error) => {
                if self.fast.is_some()
                    || self
                        .reset
                        .as_ref()
                        .is_some_and(|r| !r.cancel.load(std::sync::atomic::Ordering::SeqCst))
                {
                    self.quarantine();
                    self.recovery_restart = true;
                    self.memory_recovery.cooldown(Instant::now());
                    self.event("fast_unavailable", format!("[refresh] Cannot read memory budget: {error}; continuing in cargo mode."));
                }
                return;
            }
        };
        if sample.pressured(self.floor) {
            self.memory_recovery.reset_stability();
            if self.fast.is_some()
                || self
                    .reset
                    .as_ref()
                    .is_some_and(|r| !r.cancel.load(std::sync::atomic::Ordering::SeqCst))
            {
                self.quarantine();
                self.recovery_restart = true;
                self.memory_recovery.cooldown(Instant::now());
                self.event("fast_unavailable", format!(
                    "[refresh] Memory pressure: {:.1} GiB effective available (reserve {:.1} GiB); stopped rustd, continuing in cargo mode. Retrying when memory recovers.",
                    sample.available as f64 / crate::memory::GIB as f64,
                    self.floor as f64 / crate::memory::GIB as f64));
            }
            return;
        }
        let Some(support) = &self.config.support else {
            return;
        };
        if self.fast.is_some() || self.reset.is_some() {
            return;
        }
        if self
            .memory_recovery
            .observe(Instant::now(), sample, support.warm_peak_bytes, self.floor)
        {
            self.begin_reset();
        }
    }

    /// Maintenance has no editor revision of its own. Only restart a fully
    /// delivered, checked snapshot after the watcher has been idle for two seconds.
    fn poll_epoch(&mut self) -> Result<()> {
        if !self.epoch_idle_safe
            || self.last_save.elapsed() < Duration::from_secs(2)
            || !self.fence.current()
            || self.checker.is_some()
            || self.reset.is_some()
            || self.recovery_restart
            || (self.workspace.is_some() && !self.unchecked.is_empty())
        {
            return Ok(());
        }
        let Some(fast) = self.fast.as_ref() else {
            return Ok(());
        };
        if !fast.manifest.epoch_due && !fast.pressure {
            return Ok(());
        }
        self.event(
            "epoch_due",
            format!(
                "epoch_due=true cumulative_roots={} retained_bytes={} pressure={}",
                fast.manifest.cumulative_roots, fast.manifest.retained_bytes, fast.pressure,
            ),
        );
        // Bound the loader's retained headers/tables too. A native recapture is
        // quarantined from the current runtime until the next save establishes it.
        if crate::elf::loaded_bytes(&self.runtime.executable)? >= 256 * 1024 * 1024 {
            self.quarantine();
            self.recovery_restart = true;
            self.epoch_idle_safe = false;
            self.begin_reset();
            self.event(
                "epoch_native_due",
                "epoch limit reached; next save establishes a native baseline",
            );
            return Ok(());
        }
        let result = self.promote_idle_epoch();
        if let Err(error) = result {
            // Promotion may have advanced compiler semantics even when a newer
            // revision arrived. Never continue patching the old live baseline.
            self.quarantine();
            self.recovery_restart = true;
            self.epoch_idle_safe = false;
            self.event(
                "epoch_failed",
                format!("epoch promotion deferred to next save: {error:#}"),
            );
            self.begin_reset();
        }
        Ok(())
    }

    fn promote_idle_epoch(&mut self) -> Result<()> {
        let fast = self.fast.as_mut().context("compiler unavailable")?;
        fs::write(&fast.control, "1")?;
        let build = fast
            .client
            .build_with_async_certificates(async_certificates_enabled());
        fs::write(&fast.control, "0")?;
        let build = build?;
        ensure!(
            crate::elf::loaded_bytes(&build.candidate_path)? < 256 * 1024 * 1024,
            "epoch candidate requires a native rebase"
        );
        ensure!(
            self.fence.current(),
            "new save arrived during epoch promotion"
        );
        ensure!(
            fast.manifest.update(&build.manifest)?,
            "stale epoch manifest"
        );
        // SAFETY: this candidate is dropped before any subsequent worker build.
        let candidate = unsafe { Image::map_candidate(&build.candidate_path)? };
        // Apply the same candidate validation as a save before replacing a process.
        let _ = fast.extractor.extract(&fast.manifest, &candidate)?;
        let catalog = build.candidate_path.with_extension("promoted126.json");
        ensure!(catalog.exists(), "epoch promotion catalog missing");
        ensure!(
            self.fence.current(),
            "new save arrived during epoch validation"
        );
        self.runtime.start(&build.candidate_path)?;
        // SAFETY: normalization touches only private pages of an immutable run inode.
        let baseline = unsafe { Image::map_baseline(&self.runtime.executable)? };
        fast.extractor.promote(baseline, &fs::read(catalog)?)?;
        fast.manifest = State::default();
        fast.async_runtime = AsyncRuntime::default();
        fast.async_owners.clear();
        // Keep the existing layout-save promotion budget independent of idle
        // maintenance. The absolute executable bound above covers both kinds.
        fast.pressure = false;
        self.epoch_idle_safe = false;
        self.event(
            "epoch_restart",
            "promoted epoch baseline; cumulative roots and retained pages reset",
        );
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn qualified_async_default_keeps_explicit_opt_out_and_unknown_values_closed() {
        use std::{ffi::OsStr, os::unix::ffi::OsStrExt};
        assert!(async_certificates_setting(None));
        assert!(async_certificates_setting(Some(OsStr::new("1"))));
        for value in [
            OsStr::new("0"),
            OsStr::new(""),
            OsStr::new("true"),
            OsStr::from_bytes(b"\xff"),
        ] {
            assert!(!async_certificates_setting(Some(value)), "{value:?}");
        }
    }
    #[test]
    fn promotion_budget_excludes_sparse_reserved_region() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        file.write_all(&[1; 4096]).unwrap();
        file.as_file().set_len(300 * 1024 * 1024).unwrap();
        assert!(fs::metadata(file.path()).unwrap().len() > 256 * 1024 * 1024);
        assert!(executable_bytes(file.path()).unwrap() < 256 * 1024 * 1024);
        let snapshot = tempfile::NamedTempFile::new().unwrap();
        fs::copy(file.path(), snapshot.path()).unwrap();
        // The policy must read the compiler file: copying may fill all holes.
        assert!(fs::metadata(snapshot.path()).unwrap().len() > 256 * 1024 * 1024);
        assert!(executable_bytes(file.path()).unwrap() < 256 * 1024 * 1024);
    }
}
