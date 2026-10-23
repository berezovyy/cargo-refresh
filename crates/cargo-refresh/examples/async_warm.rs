//! Primary-frontier warm RSS probe. Run each arm in its own quiet-lane session.
use anyhow::{Result, ensure};
use cargo_refresh::rustd::{Client, Config, memory_available};
use std::{fs, path::PathBuf};
fn rss(home: &std::path::Path) -> Result<u64> {
    let marker = format!("RUSTD_HOME={}", home.display());
    let mut total = 0;
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
        if let Ok(env) = fs::read(p.join("environ")) {
            if env
                .split(|b| *b == 0)
                .any(|field| field == marker.as_bytes())
            {
                if let Ok(status) = fs::read_to_string(p.join("status")) {
                    total += status
                        .lines()
                        .find_map(|l| {
                            l.strip_prefix("VmRSS:")
                                .and_then(|l| l.split_whitespace().next())
                                .and_then(|n| n.parse::<u64>().ok())
                        })
                        .unwrap_or(0)
                        * 1024;
                }
            }
        }
    }
    Ok(total)
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
        "target/refresh/async-warm-{}",
        if enabled { "on" } else { "off" }
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
    let mut total = rss(&home)?;
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
                total += rss(&candidate)?;
            }
        }
    }
    println!(
        "{}",
        serde_json::json!({"enabled":enabled,"rss_bytes":total,"before":before,"after":{"loadavg":fs::read_to_string("/proc/loadavg")?,"mem_available":memory_available()?}})
    );
    ensure!(total > 0, "no owned compiler RSS found");
    client.stop()?;
    Ok(())
}
