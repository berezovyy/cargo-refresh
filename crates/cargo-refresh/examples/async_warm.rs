//! Primary-frontier warm RSS probe. Run each arm in its own quiet-lane session.
use anyhow::{Result, ensure};
use cargo_refresh::rustd::{Client, Config, memory_available};
use std::{fs, path::PathBuf};
fn rss(home: &std::path::Path) -> Result<(u64, u64)> {
    let marker = format!("RUSTD_HOME={}", home.display());
    let (mut total, mut hwm) = (0, 0);
    for p in fs::read_dir("/proc")? {
        let p = p?.path();
        if !p
            .file_name()
            .unwrap()
            .as_encoded_bytes()
            .iter()
            .all(u8::is_ascii_digit)
        {
            continue;
        }
        if let Ok(env) = fs::read(p.join("environ"))
            && env
                .split(|b| *b == 0)
                .any(|field| field == marker.as_bytes())
            && let Ok(status) = fs::read_to_string(p.join("status"))
        {
            let kb = |prefix: &str| {
                status
                    .lines()
                    .find_map(|l| {
                        l.strip_prefix(prefix)
                            .and_then(|l| l.split_whitespace().next())
                            .and_then(|n| n.parse::<u64>().ok())
                    })
                    .unwrap_or(0)
                    * 1024
            };
            total += kb("VmRSS:");
            hwm += kb("VmHWM:");
        }
    }
    Ok((total, hwm))
}
fn main() -> Result<()> {
    let a: Vec<_> = std::env::args().skip(1).collect();
    ensure!(
        a.len() == 5,
        "usage: async_warm PROJECT RUSTD FACTS_DIR EDIT ENABLED"
    );
    let enabled = a[4].parse::<bool>()?;
    let mut cfg = Config::new(&a[0], &a[1], "rust-analyzer")?;
    cfg.home = cfg.project.join(format!(
        "target/refresh/async-warm-{}-{}",
        if enabled { "on" } else { "off" },
        std::process::id()
    ));
    cfg.edit = vec![a[3].clone()];
    cfg.memory_min_bytes = 6 * 1024 * 1024 * 1024;
    cfg.ready_timeout = std::time::Duration::from_secs(600);
    cfg.command_timeout = cfg.ready_timeout;
    cfg.extra_env.insert(
        "RUSTD_ASYNC_CERTS".into(),
        if enabled { "1" } else { "0" }.into(),
    );
    cfg.extra_env.insert("RUSTD_DEBUGINFO".into(), "0".into());
    cfg.extra_env.insert("RUSTD_STATS".into(), "1".into());
    cfg.extra_env
        .insert("RUSTD_ABI_FACTS_DIR".into(), a[2].clone().into());
    cfg.extra_env.insert(
        "CARGO_BUILD_TARGET".into(),
        "x86_64-unknown-linux-gnu".into(),
    );
    let home: PathBuf = cfg.home.clone();
    let mut client = Client::new(cfg);
    let before = serde_json::json!({"loadavg":fs::read_to_string("/proc/loadavg")?,"mem_available":memory_available()?});
    client.start()?;
    // Socket transport aliases are short private symlinks; workers inherit that
    // spelling. Match either the real home or its unique final component.
    std::thread::sleep(std::time::Duration::from_secs(1));
    client.guard_memory()?;
    let (mut total, mut hwm) = rss(&home)?;
    if total == 0 {
        for p in fs::read_dir("/tmp")? {
            let p = p?.path();
            if !p
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("crfd-")
            {
                continue;
            }
            let candidate = p.join("p").join(home.file_name().unwrap());
            if fs::canonicalize(&candidate).ok().as_ref() == Some(&home) {
                let sample = rss(&candidate)?;
                total += sample.0;
                hwm += sample.1;
            }
        }
    }
    let compiled_roots = fs::read_dir(home.join("workers"))?
        .filter_map(|p| p.ok())
        .filter_map(|p| fs::read_to_string(p.path().join("log")).ok())
        .flat_map(|text| {
            text.lines()
                .filter_map(|line| {
                    line.strip_prefix("compiled ")
                        .and_then(|s| s.split_whitespace().next())
                        .and_then(|s| s.parse::<usize>().ok())
                })
                .collect::<Vec<_>>()
        })
        .max()
        .unwrap_or(0);
    ensure!(
        compiled_roots > 0,
        "compiler warm-up did not compile owned roots"
    );
    println!(
        "{}",
        serde_json::json!({"compiled_roots":compiled_roots,"enabled":enabled,"rss_bytes":total,"hwm_bytes":hwm,"before":before,"after":{"loadavg":fs::read_to_string("/proc/loadavg")?,"mem_available":memory_available()?}})
    );
    ensure!(total > 0, "no owned compiler RSS found");
    client.stop()?;
    Ok(())
}
