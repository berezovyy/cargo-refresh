//! Watch-only controller loop. Compiler, supervisor, and agent integration follow in later tasks.
use std::{
    collections::BTreeMap,
    io::{self, Write},
    time::Duration,
};

use anyhow::{Context, Result};

use crate::{
    changes::{ChangeTracker, Disposition},
    config::Config,
    events::{EventLog, Phase, SaveEvent, SavePath, monotonic_seconds},
    watcher::{SourceWatcher, WatchSpec},
};

pub fn run(config: Config) -> Result<()> {
    let mut events = EventLog::open(&config.events).context("open event log")?;
    let mut spec = WatchSpec::new(
        config.source_roots,
        config.input_files,
        vec![config.target_directory],
    );
    // An explicit log under a source root must not trigger a self-sustaining save loop.
    spec.exclude_file(config.events.clone());
    let mut watcher = SourceWatcher::new(spec)?;
    let mut tracker = ChangeTracker::new(watcher.files()?, watcher.build_inputs())
        .context("hash baseline sources")?;
    events.event("start", 0)?;
    println!("watching (compiler integration pending)");
    io::stdout().flush()?;
    loop {
        let Some(batch) = watcher.next_batch(Duration::from_millis(20))? else {
            continue;
        };
        let notice =
            monotonic_seconds()? * 1000.0 - batch.noticed_at.elapsed().as_secs_f64() * 1000.0;
        let mut phases = BTreeMap::from([(Phase::Notice, notice)]);
        let paths: Vec<_> = batch.paths.iter().cloned().collect();
        let changes = if batch.full_rehash {
            tracker.rehash_all(batch.paths)
        } else {
            tracker.observe(batch.paths)
        };
        phases.insert(Phase::Sources, monotonic_seconds()? * 1000.0);
        let (path, reason, files) = match changes {
            Ok(changes) => {
                let output = if changes.disposition == Disposition::Skip {
                    println!("· unchanged");
                    (SavePath::Skip, "identical-content".to_owned(), paths)
                } else {
                    let files: Vec<_> = changes
                        .changes
                        .iter()
                        .map(|change| change.path.clone())
                        .collect();
                    println!(
                        "changed: {}",
                        files
                            .iter()
                            .map(|path| path.display().to_string())
                            .collect::<Vec<_>>()
                            .join(", ")
                    );
                    (
                        SavePath::Waiting,
                        "watch-only: compiler integration pending".to_owned(),
                        files,
                    )
                };
                // For this skeleton, acknowledge observation only. Later rungs acknowledge after apply.
                assert!(tracker.mark_applied(&changes));
                output
            }
            Err(error) => {
                eprintln!("source error: {error}");
                (SavePath::Error, error.to_string(), paths)
            }
        };
        io::stdout().flush()?;
        events.save(
            0,
            &SaveEvent {
                path,
                reason,
                latency_ms: monotonic_seconds()? * 1000.0 - notice,
                phases,
                revision: tracker.revision(),
                files,
            },
        )?;
    }
}
