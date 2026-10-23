//! Watch on the main thread; runtime/compiler ownership stays on one worker.
use crate::{
    changes::ChangeTracker,
    checker::Sources,
    config::Config,
    events::{EventLog, Phase, monotonic_seconds},
    fallback::Fence,
    ladder::{Job, Pipeline, Update},
    watcher::{SourceWatcher, WatchSpec},
};
use anyhow::{Context, Result};
use std::{
    collections::BTreeMap,
    fs,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Duration,
};
static STOP: AtomicBool = AtomicBool::new(false);
extern "C" fn stop(_: libc::c_int) {
    STOP.store(true, Ordering::SeqCst);
}
pub fn run(mut config: Config) -> Result<()> {
    STOP.store(false, Ordering::SeqCst);
    unsafe {
        libc::signal(libc::SIGINT, stop as *const () as libc::sighandler_t);
        libc::signal(libc::SIGTERM, stop as *const () as libc::sighandler_t);
    }
    let storage = config.target_directory.join("refresh");
    fs::create_dir_all(&storage)?;
    let session_lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(storage.join("session.lock"))?;
    use std::os::fd::AsRawFd;
    anyhow::ensure!(
        unsafe { libc::flock(session_lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0,
        "another cargo refresh session owns this workspace"
    );
    crate::dep_info::extend(&mut config)?;
    let mut events = EventLog::open(&config.events)?;
    let mut spec = WatchSpec::new(
        config.source_roots.clone(),
        config.input_files.clone(),
        vec![config.target_directory.clone()],
    );
    spec.exclude_file(config.events.clone());
    let mut watcher = SourceWatcher::new(spec)?;
    let files = watcher.files()?;
    let sources: Sources = files
        .iter()
        .map(|path| Ok((path.clone(), Arc::from(fs::read(path)?))))
        .collect::<std::io::Result<_>>()?;
    let mut tracker = ChangeTracker::new(files, watcher.build_inputs())?;
    let latest = Arc::new(AtomicU64::new(0));
    let stopping = Arc::new(AtomicBool::new(false));
    let fence = Fence {
        revision: 0,
        latest: latest.clone(),
        stopping: stopping.clone(),
    };
    let (jobs_tx, jobs_rx) = crossbeam_channel::unbounded::<Job>();
    let (updates_tx, updates_rx) = crossbeam_channel::unbounded();
    let worker_config = config.clone();
    let worker_latest = latest.clone();
    let worker_stopping = stopping.clone();
    events.event("start", 0)?;
    let worker = std::thread::spawn(move || {
        let latest = worker_latest;
        let stopping = worker_stopping;
        let result = (|| -> Result<()> {
            let mut pipeline = Pipeline::new(worker_config, sources, fence, updates_tx.clone())?;
            pipeline.setup()?;
            while !pipeline_stopping(&stopping) {
                match jobs_rx.recv_timeout(Duration::from_millis(10)) {
                    Ok(mut job) => {
                        while let Ok(newer) = jobs_rx.try_recv() {
                            updates_tx.send(Update::Event {
                                name: "superseded",
                                message: "save replaced by a newer queued revision".into(),
                                revision: job.changes.revision,
                            })?;
                            job = newer;
                        }
                        let failed_job = job.clone();
                        if let Err(error) = pipeline.process(job) {
                            pipeline.failure(&failed_job, &error)?;
                            updates_tx.send(Update::Event {
                                name: "operation_failed",
                                message: format!("{error:#}"),
                                revision: latest.load(Ordering::SeqCst),
                            })?;
                        }
                    }
                    Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
                    Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
                }
                if let Err(error) = pipeline.poll() {
                    updates_tx.send(Update::Event {
                        name: "operation_failed",
                        message: format!("{error:#}"),
                        revision: latest.load(Ordering::SeqCst),
                    })?;
                }
            }
            Ok(())
        })();
        if let Err(error) = result {
            let _ = updates_tx.send(Update::Fatal(format!("{error:#}")));
        }
    });
    let mut generation = 0;
    let mut ready = false;
    let mut fatal = None;
    // Initial capture reads disk; queue watcher input until that baseline is established.
    let result = (|| -> Result<()> {
        while !STOP.load(Ordering::SeqCst) {
            for update in updates_rx.try_iter() {
                match update {
                    Update::AgentReply { reply, revision } => {
                        let mut fields = serde_json::to_value(&reply)?;
                        fields
                            .as_object_mut()
                            .unwrap()
                            .insert("revision".into(), revision.into());
                        events.detail("agent_reply", generation, fields)?;
                    }
                    Update::Ready => {
                        ready = true;
                        eprintln!("cargo refresh ready");
                    }
                    Update::Save {
                        event,
                        applied,
                        generation: applied_generation,
                    } => {
                        generation = applied_generation;
                        if let Some(changes) = applied
                            && !tracker.mark_applied(&changes)
                        {
                            let paths: Vec<_> = changes
                                .changes
                                .iter()
                                .map(|c| (c.path.clone(), c.content.clone()))
                                .collect();
                            tracker.restore_applied(&paths)?;
                            let reconciled = tracker.observe(paths.into_iter().map(|(p, _)| p))?;
                            latest.store(reconciled.revision, Ordering::SeqCst);
                            let now = monotonic_seconds()? * 1000.0;
                            jobs_tx.send(Job {
                                changes: reconciled,
                                phases: BTreeMap::from([
                                    (Phase::Notice, now),
                                    (Phase::Sources, now),
                                ]),
                            })?;
                            events.detail("superseded",generation,serde_json::json!({"revision":event.revision,"message":"runtime commit overlapped a new save; reconciling its current bytes"}))?;
                            continue;
                        }
                        crate::ui::save(&event, config.verbose);
                        events.save(generation, &event)?;
                    }
                    Update::Reverted { paths, revision } => {
                        let current = tracker.is_current(revision);
                        tracker.restore_applied(&paths)?;
                        if !current {
                            let reconciled = tracker.observe(paths.into_iter().map(|(p, _)| p))?;
                            latest.store(reconciled.revision, Ordering::SeqCst);
                            let now = monotonic_seconds()? * 1000.0;
                            jobs_tx.send(Job {
                                changes: reconciled,
                                phases: BTreeMap::from([
                                    (Phase::Notice, now),
                                    (Phase::Sources, now),
                                ]),
                            })?;
                        }
                    }
                    Update::Reconfigure(new_config, _revision) => {
                        let mut spec = WatchSpec::new(
                            new_config.source_roots,
                            new_config.input_files,
                            vec![new_config.target_directory],
                        );
                        spec.exclude_file(config.events.clone());
                        watcher.reconfigure(spec)?;
                        tracker.set_build_inputs(watcher.build_inputs());
                        // Newly captured inputs have no acknowledged hash yet.
                        // Their first queued save must not be mistaken for the
                        // compiled baseline merely because discovery reads it later.
                    }
                    Update::Event {
                        name,
                        message,
                        revision,
                    } => {
                        if config.verbose > 0
                            || matches!(
                                name,
                                "tainted"
                                    | "revert"
                                    | "revert_failed"
                                    | "operation_failed"
                                    | "fast_unavailable"
                                    | "worker_reset_failed"
                                    | "application_exit"
                            )
                        {
                            eprintln!(
                                "{}",
                                if config.verbose > 0 {
                                    &message
                                } else {
                                    message.lines().next().unwrap_or_default()
                                }
                            );
                        }
                        events.detail(
                            name,
                            generation,
                            serde_json::json!({"message":message,"revision":revision}),
                        )?;
                    }
                    Update::Fatal(error) => {
                        fatal = Some(error);
                        STOP.store(true, Ordering::SeqCst);
                    }
                }
            }
            if ready {
                if let Some(batch) = watcher.next_batch(Duration::from_millis(10))? {
                    let notice = monotonic_seconds()? * 1000.0
                        - batch.noticed_at.elapsed().as_secs_f64() * 1000.0;
                    let changes = if batch.full_rehash {
                        tracker.rehash_all(batch.paths)
                    } else {
                        tracker.observe(batch.paths)
                    }
                    .context("read changed sources")?;
                    let phases = BTreeMap::from([
                        (Phase::Notice, notice),
                        (Phase::Sources, monotonic_seconds()? * 1000.0),
                    ]);
                    // Publish the revision before submitting. Both compiler and checker fence against it.
                    latest.store(changes.revision, Ordering::SeqCst);
                    jobs_tx.send(Job { changes, phases })?;
                }
            } else {
                std::thread::sleep(Duration::from_millis(10));
            }
            if worker.is_finished() && updates_rx.is_empty() {
                break;
            }
        }
        Ok(())
    })();
    // Close the queue and request cancellation before waiting for owned children to stop.
    stopping.store(true, Ordering::SeqCst);
    drop(jobs_tx);
    let joined = worker.join();
    events.event("stop", generation)?;
    result?;
    joined.map_err(|_| anyhow::anyhow!("controller worker panicked"))?;
    if let Some(error) = fatal {
        anyhow::bail!("{error}");
    }
    Ok(())
}
fn pipeline_stopping(stopping: &AtomicBool) -> bool {
    if STOP.load(Ordering::SeqCst) {
        stopping.store(true, Ordering::SeqCst);
    }
    stopping.load(Ordering::SeqCst)
}
