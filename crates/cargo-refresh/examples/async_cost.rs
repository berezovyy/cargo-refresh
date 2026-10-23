//! Quiet-lane manual A/B probe: certificate-off/on builds, interleaved per pair.
use anyhow::{Result, ensure};
use cargo_refresh::rustd::{Client, Config, memory_available};
use std::{fs, path::PathBuf, time::Instant};
struct Restore(PathBuf, Vec<u8>);
impl Drop for Restore {
    fn drop(&mut self) {
        let _ = fs::write(&self.0, &self.1);
    }
}
fn resources() -> Result<serde_json::Value> {
    Ok(
        serde_json::json!({"loadavg":fs::read_to_string("/proc/loadavg")?.trim(),"mem_available":memory_available()?}),
    )
}
fn main() -> Result<()> {
    let a: Vec<_> = std::env::args().skip(1).collect();
    ensure!(a.len() == 3, "usage: async_cost PROJECT RUSTD FACTS_DIR");
    let mut cfg = Config::new(&a[0], &a[1], "web-server")?;
    cfg.memory_min_bytes = 6 * 1024 * 1024 * 1024;
    cfg.home = cfg.project.join("target/refresh/async-cost");
    cfg.edit = vec!["api".into(), "domain".into(), "store".into()];
    cfg.extra_env.insert("RUSTD_ASYNC_CERTS".into(), "1".into());
    cfg.extra_env.insert("RUSTD_DEBUGINFO".into(), "0".into());
    cfg.extra_env.insert("RUSTD_STATS".into(), "1".into());
    cfg.extra_env
        .insert("RUSTD_ABI_FACTS_DIR".into(), a[2].clone().into());
    cfg.extra_env.insert(
        "CARGO_BUILD_TARGET".into(),
        "x86_64-unknown-linux-gnu".into(),
    );
    let source = cfg.project.join("crates/api/src/handlers.rs");
    let original = fs::read(&source)?;
    let text = String::from_utf8(original.clone())?;
    let needle = "weighted_total: total.saturating_mul(priority_weight(0) as usize),";
    ensure!(text.contains(needle), "missing summary arithmetic marker");
    let mut client = Client::new(cfg);
    client.start()?;
    let restore = Restore(source, original);
    let result = (|| -> Result<()> {
        for pair in 0..=30 {
            if pair == 1 {
                loop {
                    client.guard_memory()?;
                    let load = fs::read_to_string("/proc/loadavg")?;
                    if load.split_whitespace().next().unwrap().parse::<f64>()? <= 2.0 {
                        break;
                    }
                    std::thread::sleep(std::time::Duration::from_secs(1));
                }
            }
            let mut times = Vec::new();
            let mut proof_rows = Vec::new();
            let before = resources()?;
            for enabled in [false, true] {
                let value = pair * 2 + usize::from(enabled);
                let replacement = format!(
                    "weighted_total: total.saturating_mul(priority_weight(0) as usize).saturating_add({value:06}),"
                );
                fs::write(&restore.0, text.replacen(needle, &replacement, 1))?;
                let start = Instant::now();
                let build = client.build_with_async_certificates(enabled)?;
                times.push(start.elapsed().as_secs_f64() * 1000.0);
                let m = refresh_proto::manifest::parse(&build.manifest)?;
                let count = m.async_certificates.map(|rows| rows.len());
                ensure!(
                    if enabled {
                        count.is_some_and(|n| n > 0)
                    } else {
                        count.is_none()
                    },
                    "A/B arm did not exercise the requested proof path: enabled={enabled}, rows={count:?}"
                );
                proof_rows.push(count);
            }
            println!(
                "{}",
                serde_json::json!({"pair":pair,"warmup":pair==0,"off_ms":times[0],"on_ms":times[1],"delta_ms":times[1]-times[0],"ratio":times[1]/times[0],"proof_rows":proof_rows,"before":before,"after":resources()?})
            );
        }
        Ok(())
    })();
    client.stop()?;
    drop(restore);
    result
}
