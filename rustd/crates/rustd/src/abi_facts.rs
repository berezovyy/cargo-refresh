//! Exact-compiler qualification for the EXP-104 x86_64 default-feature adapter.
use std::{collections::BTreeMap, path::PathBuf, process::Command};
use anyhow::{Context, bail};
use serde::Deserialize;
use sha2::{Digest, Sha256};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Bundle {
    schema: u32,
    commit_hash: String,
    rustc_vv: String,
    target: String,
    simd128: String,
    personality: String,
    type_id_source: String,
    source_sha256: String,
    fixture: String,
}

pub(crate) fn load() -> anyhow::Result<BTreeMap<String, String>> {
    let compiler = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
    let out = Command::new(compiler).arg("-Vv").output().context("querying rustc -Vv")?;
    if !out.status.success() { bail!("rustc -Vv failed"); }
    let identity = String::from_utf8(out.stdout)?;
    let commit = identity.lines().find_map(|l| l.strip_prefix("commit-hash: "))
        .context("compiler has no commit hash")?;
    let directory = PathBuf::from(std::env::var_os("RUSTD_ABI_FACTS_DIR")
        .context("set RUSTD_ABI_FACTS_DIR to qualified helper bundles")?);
    let path = directory.join(format!("{commit}.json"));
    let data = std::fs::read(&path).with_context(|| format!("reading {}", path.display()))?;
    let bundle: Bundle = serde_json::from_slice(&data)?;
    let target = std::env::var("CARGO_BUILD_TARGET").unwrap_or_else(|_| "x86_64-unknown-linux-gnu".into());
    if bundle.schema != 1 || bundle.commit_hash != commit || bundle.rustc_vv.trim() != identity.trim()
        || bundle.target != target || target != "x86_64-unknown-linux-gnu" {
        bail!("ABI bundle compiler/schema/target mismatch");
    }
    // This adapter is qualified only with default features. Broader settings need new facts.
    let flags = std::env::var("CARGO_ENCODED_RUSTFLAGS").unwrap_or_default()
        + &std::env::var("RUSTFLAGS").unwrap_or_default();
    if flags.contains("target-feature") || flags.contains("target-cpu") {
        bail!("ABI bundle does not qualify custom CPU/features");
    }
    if !matches!(bundle.type_id_source.as_str(), "mirrored" | "rustc-symbol")
        || !matches!(bundle.simd128.as_str(), "direct" | "indirect")
        || bundle.personality.is_empty()
        || !bundle.personality.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
        || bundle.source_sha256.len() != 64 || bundle.fixture.is_empty() {
        bail!("incomplete ABI bundle");
    }
    Ok(BTreeMap::from([
        ("RUSTD_ABI_SIMD128".into(), bundle.simd128),
        ("RUSTD_ABI_TYPEID_SOURCE".into(), bundle.type_id_source),
        ("RUSTD_ABI_PERSONALITY".into(), bundle.personality),
        ("RUSTD_ABI_FACTS_SHA256".into(), format!("{:x}", Sha256::digest(&data))),
        ("RUSTD_CACHE".into(), "off".into()),
    ]))
}
