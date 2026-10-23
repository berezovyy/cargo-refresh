//! Manual integration check. Hold the rustd lane for this whole process.
use anyhow::{Context, Result, ensure};
use cargo_refresh::{
    manifest::State,
    rustd::{Client, Config},
};
use std::{collections::BTreeMap, ffi::OsString, path::PathBuf};
struct Restore {
    path: PathBuf,
    bytes: Vec<u8>,
}
impl Drop for Restore {
    fn drop(&mut self) {
        let _ = std::fs::write(&self.path, &self.bytes);
    }
}
fn main() -> Result<()> {
    let a: Vec<_> = std::env::args().skip(1).collect();
    ensure!(
        a.len() == 6,
        "usage: rustd_client PROJECT BINARY RUSTD SOURCE_RELATIVE BEFORE AFTER"
    );
    let mut config = Config::new(&a[0], &a[2], &a[1])?;
    config.memory_min_bytes = 6 * 1024 * 1024 * 1024;
    // A fresh home avoids stale sockets in a previous smoke run.
    config.home = config.project.join(format!(
        "target/refresh/client-smoke-{}",
        std::process::id()
    ));
    if let Some(value) = std::env::var_os("REFRESH_RUSTD_ENV") {
        let env: BTreeMap<String, String> = serde_json::from_str(&value.to_string_lossy())?;
        config.extra_env = env
            .into_iter()
            .map(|(k, v)| (OsString::from(k), OsString::from(v)))
            .collect();
    }
    let path = config.project.join(&a[3]);
    let bytes = std::fs::read(&path)?;
    let text = std::str::from_utf8(&bytes)?;
    ensure!(
        text.matches(&a[4]).count() == 1,
        "BEFORE must occur exactly once"
    );
    let edited = text.replacen(&a[4], &a[5], 1);
    ensure!(edited.as_bytes() != bytes, "edit must change the source");
    let mut client = Client::new(config);
    client
        .start()
        .context("real daemon startup/capture/readiness")?;
    // Explicit stop before source restoration avoids leaving a worker observing
    // a restoration after this integration run has ended.
    let restore = Restore { path, bytes };
    let result = (|| -> Result<()> {
        std::fs::write(&restore.path, edited)?;
        let build = client.build().context("direct rustd sync build")?;
        let mut state = State::default();
        state.update(&build.manifest)?;
        ensure!(
            state.generation == 1,
            "smoke expects a new native capture epoch"
        );
        let candidate =
            cargo_refresh::elf::Image::parse(std::fs::read(&build.candidate_path)?, false)?;
        ensure!(
            !candidate.headers.is_empty(),
            "candidate has no program headers"
        );
        println!(
            "REAL_CLIENT_OK generation={} manifest_bytes={} pages={} writes={} candidate={}",
            state.generation,
            build.manifest.len(),
            state.pages.len(),
            state.writes.len(),
            build.candidate_path.display()
        );
        Ok(())
    })();
    let stopped = client.stop();
    drop(restore);
    result?;
    stopped?;
    Ok(())
}
