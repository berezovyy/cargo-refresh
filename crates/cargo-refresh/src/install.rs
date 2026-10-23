//! Exact-toolchain support selection. Nothing executable is used before validation.
use anyhow::{Context, Result, bail, ensure};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File},
    io::{Read, Write},
    os::{fd::AsRawFd, unix::fs::PermissionsExt},
    path::{Component, Path, PathBuf},
    process::Command,
    sync::Arc,
    time::Instant,
};

use crate::config::Config;

pub const QUALIFIED_COMMIT: &str = "b940084d7eb6a299eb4bfeb8e34901bc051e7ac4";
pub const TARGET: &str = "x86_64-unknown-linux-gnu";
const GIB: u64 = 1024 * 1024 * 1024;
// M1 exceeded the proposed 6 GiB peak. Until per-project estimates exist, reserve
// 13 GiB for warming (research upper bound), plus the required 2 GiB headroom.
pub const DEFAULT_WARM_PEAK: u64 = 13 * GIB;
const LIMIT: u64 = 512 * 1024 * 1024;
const FILES: &[&str] = &[
    "rustd",
    "cargo-rustd",
    "rustd-rustc",
    "librefresh.so",
    "abi-facts.bundle",
    "VERSION",
    "RUNTIME.txt",
    "licenses/cargo-refresh-LICENSE-MIT",
    "licenses/cargo-refresh-LICENSE-APACHE",
    "licenses/rustd-LICENSE-MIT",
    "licenses/rustd-LICENSE-APACHE",
    "licenses/rust-analyzer-LICENSE-MIT",
    "licenses/rust-analyzer-LICENSE-APACHE",
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Toolchain {
    pub release: String,
    pub commit: String,
    pub host: String,
}
impl Toolchain {
    pub fn parse(text: &str) -> Result<Self> {
        let fields = key_values(text, ": ")?;
        let get = |name| {
            fields
                .get(name)
                .cloned()
                .with_context(|| format!("rustc -vV missing {name}"))
        };
        let result = Self {
            release: get("release")?,
            commit: get("commit-hash")?,
            host: get("host")?,
        };
        ensure!(
            result.commit.len() == 40 && result.commit.bytes().all(|b| b.is_ascii_hexdigit()),
            "invalid rustc commit"
        );
        Ok(result)
    }
    pub fn detect(project: &Path) -> Result<Self> {
        let mut cmd = Command::new(std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into()));
        cmd.arg("-vV").current_dir(project);
        let output = crate::process::spawn(
            cmd.stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped()),
        )?
        .wait_with_output()?;
        ensure!(
            output.status.success(),
            "rustc -vV failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        Self::parse(std::str::from_utf8(&output.stdout)?)
    }
    pub fn qualified(&self) -> bool {
        self.commit == QUALIFIED_COMMIT && self.release == "1.99.0" && self.host == TARGET
    }
    pub fn fallback_message(&self) -> String {
        format!(
            "[refresh] cargo-refresh has no qualified support for rustc {} ({}).\n[refresh] Running normal cargo builds + restart (no live patching).\n[refresh] Supported toolchain: 1.99.0, x86_64-unknown-linux-gnu, default CPU/features.\n[refresh] See docs/install-layout.md for toolchain qualification.",
            self.release, self.commit
        )
    }
}

#[derive(Debug)]
pub struct Support {
    pub root: PathBuf,
    pub warm_peak_bytes: u64,
    // Private immutable bundle snapshot lives as long as all Pipeline/Reset clones.
    bundle: tempfile::TempDir,
}
impl Support {
    pub fn bundle_dir(&self) -> &Path {
        self.bundle.path()
    }
}

fn key_values(text: &str, separator: &str) -> Result<BTreeMap<String, String>> {
    let mut fields = BTreeMap::new();
    for line in text.lines() {
        if let Some((key, value)) = line.split_once(separator) {
            ensure!(
                fields.insert(key.into(), value.into()).is_none(),
                "duplicate metadata key {key}"
            );
        }
    }
    Ok(fields)
}
fn hash_reader(mut reader: impl Read) -> Result<String> {
    let mut hasher = Sha256::new();
    let mut buffer = [0; 64 * 1024];
    loop {
        let n = reader.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        hasher.update(&buffer[..n]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}
fn hash_file(path: &Path) -> Result<String> {
    hash_reader(File::open(path)?)
}
fn digest(value: &str) -> Result<()> {
    ensure!(
        value.len() == 64 && value.bytes().all(|b| b.is_ascii_hexdigit()),
        "invalid SHA-256"
    );
    Ok(())
}
fn member(path: &Path) -> Result<String> {
    ensure!(!path.is_absolute(), "absolute archive path");
    let mut parts = Vec::new();
    for part in path.components() {
        match part {
            Component::Normal(p) => parts.push(p.to_str().context("non-UTF8 member")?),
            Component::CurDir => {}
            _ => bail!("unsafe archive path {}", path.display()),
        }
    }
    let name = parts.join("/");
    ensure!(
        FILES.contains(&name.as_str()) || name == "SHA256SUMS",
        "unexpected member {name}"
    );
    Ok(name)
}
fn sums(text: &str) -> Result<BTreeMap<String, String>> {
    let mut result = BTreeMap::new();
    for line in text.lines() {
        let (sum, path) = line.split_once("  ").context("invalid SHA256SUMS line")?;
        digest(sum)?;
        let name = member(Path::new(path))?;
        ensure!(
            name != "SHA256SUMS"
                && result
                    .insert(name.clone(), sum.to_ascii_lowercase())
                    .is_none(),
            "duplicate checksum {name}"
        );
    }
    ensure!(
        result.len() == FILES.len() && FILES.iter().all(|f| result.contains_key(*f)),
        "incomplete SHA256SUMS"
    );
    Ok(result)
}

/// Validate the cache on every start, including all payload digests and ABI identity.
pub fn validate(root: &Path, toolchain: &Toolchain) -> Result<()> {
    ensure!(toolchain.qualified(), "unqualified rustc identity");
    let root_metadata = fs::symlink_metadata(root)?;
    ensure!(
        root_metadata.file_type().is_dir(),
        "support root is not a real directory"
    );
    ensure!(
        root_metadata.permissions().mode() & 0o022 == 0,
        "support directory is group/world writable"
    );
    let mut total = 0_u64;
    let mut found = BTreeSet::new();
    for entry in walkdir::WalkDir::new(root).min_depth(1).follow_links(false) {
        let entry = entry?;
        if entry.file_type().is_dir() {
            ensure!(
                entry.path() == root.join("licenses"),
                "unexpected support directory"
            );
            ensure!(
                entry.metadata()?.permissions().mode() & 0o022 == 0,
                "support subdirectory is group/world writable"
            );
            continue;
        }
        ensure!(
            entry.file_type().is_file(),
            "support member is not a regular file"
        );
        ensure!(
            entry.metadata()?.permissions().mode() & 0o6022 == 0,
            "unsafe support permissions: {}",
            entry.path().display()
        );
        let name = member(entry.path().strip_prefix(root)?)?;
        total = total
            .checked_add(entry.metadata()?.len())
            .context("size overflow")?;
        ensure!(total <= LIMIT, "support exceeds size limit");
        found.insert(name);
    }
    ensure!(found.len() == FILES.len() + 1, "support members missing");
    let expected = sums(&fs::read_to_string(root.join("SHA256SUMS"))?)?;
    for (name, sum) in expected {
        ensure!(
            hash_file(&root.join(&name))? == sum,
            "SHA-256 mismatch: {name}"
        );
    }
    for name in ["rustd", "cargo-rustd", "rustd-rustc", "librefresh.so"] {
        ensure!(
            fs::metadata(root.join(name))?.permissions().mode() & 0o100 != 0,
            "{name} is not executable"
        );
    }
    let version = key_values(&fs::read_to_string(root.join("VERSION"))?, "=")?;
    ensure!(
        version.get("rustc_commit") == Some(&toolchain.commit),
        "VERSION rustc commit mismatch"
    );
    ensure!(
        version.get("target").map(String::as_str) == Some(TARGET),
        "VERSION target mismatch"
    );
    for key in ["cargo_refresh_commit", "rustd_commit", "ra_commit"] {
        ensure!(
            version
                .get(key)
                .is_some_and(|v| v.len() == 40 && v.bytes().all(|b| b.is_ascii_hexdigit())),
            "VERSION missing/invalid {key}"
        );
    }
    ensure!(
        version
            .get("agent_static_libunwind")
            .is_some_and(|v| v == "0" || v == "1"),
        "VERSION missing/invalid agent linkage"
    );
    // The local development candidate uses -dev while the controller crate is 0.1.0.
    ensure!(
        version
            .get("cargo_refresh")
            .is_some_and(|v| v.strip_suffix("-dev").unwrap_or(v) == env!("CARGO_PKG_VERSION")),
        "controller version mismatch"
    );
    let facts: serde_json::Value =
        serde_json::from_slice(&fs::read(root.join("abi-facts.bundle"))?)?;
    ensure!(
        facts["schema"] == 1
            && facts["commit_hash"] == toolchain.commit
            && facts["target"] == TARGET,
        "ABI bundle identity mismatch"
    );
    ensure!(
        facts["simd128"] == "indirect" && facts["personality"] == "rust_eh_personality",
        "unqualified ABI facts"
    );
    let bundled = Toolchain::parse(
        facts["rustc_vv"]
            .as_str()
            .context("bundle missing rustc_vv")?,
    )?;
    ensure!(&bundled == toolchain, "bundle compiler identity mismatch");
    Ok(())
}

/// Local archives and their adjacent .sha256 files are explicitly trusted inputs.
/// Public fetching requires a trusted release index; no URL is invented here.
pub fn install_archive(archive: &Path, cache: &Path, toolchain: &Toolchain) -> Result<PathBuf> {
    ensure!(
        toolchain.qualified(),
        "refusing archive for unknown toolchain"
    );
    let metadata = fs::metadata(archive)?;
    ensure!(
        metadata.is_file() && metadata.len() <= LIMIT / 2,
        "archive exceeds size limit"
    );
    let mut checksum_path = archive.as_os_str().to_owned();
    checksum_path.push(".sha256");
    let checksum = fs::read_to_string(PathBuf::from(checksum_path))?;
    let mut lines = checksum.lines();
    let line = lines.next().context("empty archive checksum")?;
    ensure!(lines.next().is_none(), "expected one archive checksum");
    let (sum, name) = line.split_once("  ").context("invalid archive checksum")?;
    digest(sum)?;
    ensure!(
        Some(std::ffi::OsStr::new(name)) == archive.file_name(),
        "checksum filename mismatch"
    );
    // Keep the same opened descriptor for verification and extraction (no path race).
    let mut source = File::open(archive)?;
    ensure!(
        hash_reader(&mut source)? == sum.to_ascii_lowercase(),
        "archive SHA-256 mismatch"
    );
    use std::io::Seek;
    source.rewind()?;
    fs::create_dir_all(cache)?;
    fs::set_permissions(cache, fs::Permissions::from_mode(0o700))?;
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(cache.join(format!(".{}.lock", toolchain.commit)))?;
    ensure!(
        unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX) } == 0,
        "install lock failed"
    );
    let destination = cache.join(&toolchain.commit);
    if destination.exists() {
        validate(&destination, toolchain)
            .context("existing cache is corrupt; remove it explicitly before reinstalling")?;
        return Ok(destination);
    }
    let staging = tempfile::Builder::new()
        .prefix(".install-")
        .tempdir_in(cache)?;
    fs::set_permissions(staging.path(), fs::Permissions::from_mode(0o700))?;
    let decoder = zstd::Decoder::new(source)?;
    let mut tar = tar::Archive::new(decoder.take(LIMIT + 1024 * 1024));
    let mut expanded = 0_u64;
    let mut seen = BTreeSet::new();
    for entry in tar.entries()? {
        let mut entry = entry?;
        let path = entry.path()?.into_owned();
        if entry.header().entry_type().is_dir() {
            ensure!(
                path == Path::new("licenses"),
                "unexpected archive directory"
            );
            continue;
        }
        ensure!(
            entry.header().entry_type().is_file(),
            "archive links and special files are forbidden"
        );
        let name = member(&path)?;
        ensure!(seen.insert(name.clone()), "duplicate archive member {name}");
        expanded = expanded
            .checked_add(entry.size())
            .context("expanded size overflow")?;
        ensure!(expanded <= LIMIT, "archive expansion exceeds size limit");
        let target = staging.path().join(&name);
        fs::create_dir_all(target.parent().unwrap())?;
        if target.parent() != Some(staging.path()) {
            fs::set_permissions(target.parent().unwrap(), fs::Permissions::from_mode(0o755))?;
        }
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&target)?;
        let size = std::io::copy(&mut entry, &mut file)?;
        ensure!(size == entry.size(), "truncated archive member");
        file.flush()?;
        let mode =
            if ["rustd", "cargo-rustd", "rustd-rustc", "librefresh.so"].contains(&name.as_str()) {
                0o755
            } else {
                0o644
            };
        fs::set_permissions(&target, fs::Permissions::from_mode(mode))?;
    }
    validate(staging.path(), toolchain)?;
    fs::rename(staging.path(), &destination).context("atomic support installation")?;
    Ok(destination)
}

pub fn memory_allows(available: u64, peak: u64) -> bool {
    peak.checked_add(2 * GIB)
        .is_some_and(|required| available >= required)
}
fn default_environment(config: &Config) -> Result<()> {
    // No compiler/target/CPU overrides have been qualified. Cargo config may
    // hide these too; conservatively choose stock for any nonempty config.
    for key in [
        "RUSTFLAGS",
        "CARGO_ENCODED_RUSTFLAGS",
        "CARGO_BUILD_TARGET",
        "RUSTC_WRAPPER",
        "RUSTC_WORKSPACE_WRAPPER",
        "RUSTC_BOOTSTRAP",
        "LD_PRELOAD",
    ] {
        ensure!(
            std::env::var_os(key).is_none_or(|v| v.is_empty()),
            "unqualified environment override {key}"
        );
    }
    for (key, _) in std::env::vars_os() {
        let key = key.to_string_lossy();
        ensure!(
            !(key.starts_with("CARGO_TARGET_")
                || key.starts_with("CARGO_PROFILE_")
                || (key.starts_with("CARGO_BUILD_") && key != "CARGO_BUILD_JOBS")),
            "unqualified Cargo override {key}"
        );
    }
    let mut configs: Vec<_> = config
        .input_files
        .iter()
        .filter(|p| {
            p.file_name()
                .is_some_and(|n| n == "config" || n == "config.toml")
        })
        .cloned()
        .collect();
    let cargo_home = std::env::var_os("CARGO_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|p| PathBuf::from(p).join(".cargo")));
    if let Some(home) = cargo_home {
        configs.extend([home.join("config"), home.join("config.toml")]);
    }
    for path in configs {
        if path.exists() {
            let content = fs::read_to_string(&path)?;
            ensure!(
                content
                    .lines()
                    .all(|line| line.trim().is_empty() || line.trim().starts_with('#')),
                "Cargo config is not qualified: {}",
                path.display()
            );
        }
    }
    Ok(())
}
fn runtime_dependencies(root: &Path) -> Result<()> {
    // --list resolves DT_NEEDED and symbol versions without running constructors.
    for name in ["rustd", "cargo-rustd", "rustd-rustc", "librefresh.so"] {
        let output = Command::new("/lib64/ld-linux-x86-64.so.2")
            .arg("--list")
            .arg(root.join(name))
            .output()?;
        ensure!(
            output.status.success(),
            "runtime requirements for {name}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    unsafe {
        let handle = libc::dlopen(
            c"libgcc_s.so.1".as_ptr(),
            libc::RTLD_LAZY | libc::RTLD_LOCAL,
        );
        ensure!(
            !handle.is_null(),
            "Live patching needs libgcc_s.so.1; install your distro's GCC runtime package"
        );
        libc::dlclose(handle);
    }
    Ok(())
}
fn cache_root() -> Result<PathBuf> {
    if let Some(path) = std::env::var_os("XDG_CACHE_HOME").filter(|v| !v.is_empty()) {
        let path = PathBuf::from(path);
        ensure!(path.is_absolute(), "XDG_CACHE_HOME must be absolute");
        return Ok(path.join("cargo-refresh"));
    }
    Ok(
        PathBuf::from(std::env::var_os("HOME").context("HOME is not set")?)
            .join(".cache/cargo-refresh"),
    )
}

/// Called on startup and after Cargo inputs change. Failure is a stock decision.
pub fn prepare(config: &mut Config) {
    config.support = None;
    let toolchain = match Toolchain::detect(&config.workspace_root) {
        Ok(value) => value,
        Err(error) => {
            eprintln!(
                "[refresh] Cannot detect rustc: {error:#}.\n[refresh] Running normal cargo builds + restart (no live patching)."
            );
            return;
        }
    };
    let selected = (|| -> Result<Support> {
        ensure!(toolchain.qualified(), "unsupported toolchain");
        default_environment(config)?;
        let available = crate::rustd::memory_available()?;
        // Optional measured project estimate, in bytes. Never accept an invalid estimate.
        let peak = std::env::var("CARGO_REFRESH_WARM_PEAK_BYTES")
            .ok()
            .map(|v| v.parse::<u64>())
            .transpose()?
            .unwrap_or(DEFAULT_WARM_PEAK);
        if !memory_allows(available, peak) {
            eprintln!(
                "[refresh] Live patching needs an estimated {:.1} GiB plus 2 GiB reserve; {:.1} GiB is available.\n[refresh] Continuing in cargo mode. Close other large processes and retry to enable live patching.",
                peak as f64 / GIB as f64,
                available as f64 / GIB as f64
            );
            bail!("insufficient memory for compiler warm-up");
        }
        let cache = cache_root()?;
        let local = std::env::var_os("CARGO_REFRESH_SUPPORT").map(PathBuf::from);
        let mut root = local
            .clone()
            .unwrap_or_else(|| cache.join(&toolchain.commit));
        if !root.exists() && local.is_none() {
            if let Some(archive) = config
                .invocation
                .from_archive
                .clone()
                .or_else(|| std::env::var_os("CARGO_REFRESH_DIST").map(PathBuf::from))
            {
                ensure!(
                    !archive.to_string_lossy().contains("://"),
                    "URL support download is not available; supply a local release archive"
                );
                let started = Instant::now();
                eprintln!(
                    "[refresh] installing local toolchain support from {}…",
                    archive.display()
                );
                root = install_archive(&archive, &cache, &toolchain)?;
                eprintln!(
                    "[refresh] toolchain support installed in {:.2} s",
                    started.elapsed().as_secs_f64()
                );
            } else {
                bail!(
                    "toolchain support missing at {}; downloading is not available yet (use --from-archive or CARGO_REFRESH_DIST)",
                    root.display()
                );
            }
        }
        validate(&root, &toolchain)
            .with_context(|| format!("invalid support at {}", root.display()))?;
        runtime_dependencies(&root)?;
        let bundle = tempfile::Builder::new().prefix("refresh-abi-").tempdir()?;
        fs::set_permissions(bundle.path(), fs::Permissions::from_mode(0o700))?;
        fs::copy(
            root.join("abi-facts.bundle"),
            bundle.path().join(format!("{}.json", toolchain.commit)),
        )?;
        Ok(Support {
            root: root.canonicalize()?,
            warm_peak_bytes: peak,
            bundle,
        })
    })();
    match selected {
        Ok(support) => {
            eprintln!(
                "[refresh] rustc {} ({:.9}…) — supported",
                toolchain.release, toolchain.commit
            );
            eprintln!("[refresh] toolchain support: {}", support.root.display());
            config.support = Some(Arc::new(support));
        }
        Err(error) => {
            eprintln!("[refresh] {error:#}");
            eprintln!("{}", toolchain.fallback_message());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn qualified() -> Toolchain {
        Toolchain::parse(&format!(
            "rustc 1.99.0\ncommit-hash: {QUALIFIED_COMMIT}\nrelease: 1.99.0\nhost: {TARGET}\n"
        ))
        .unwrap()
    }
    fn checksums(root: &Path) {
        let text = FILES
            .iter()
            .map(|name| format!("{}  ./{name}\n", hash_file(&root.join(name)).unwrap()))
            .collect::<String>();
        fs::write(root.join("SHA256SUMS"), text).unwrap();
        fs::set_permissions(root.join("SHA256SUMS"), fs::Permissions::from_mode(0o644)).unwrap();
    }
    fn package(root: &Path) {
        fs::create_dir_all(root.join("licenses")).unwrap();
        fs::set_permissions(root, fs::Permissions::from_mode(0o700)).unwrap();
        fs::set_permissions(root.join("licenses"), fs::Permissions::from_mode(0o755)).unwrap();
        for name in FILES {
            fs::write(root.join(name), b"test payload\n").unwrap();
            fs::set_permissions(root.join(name), fs::Permissions::from_mode(0o755)).unwrap();
        }
        fs::write(
            root.join("VERSION"),
            format!(
                "cargo_refresh={}\nrustc_commit={QUALIFIED_COMMIT}\ntarget={TARGET}\ncargo_refresh_commit={QUALIFIED_COMMIT}\nrustd_commit={QUALIFIED_COMMIT}\nra_commit={QUALIFIED_COMMIT}\nagent_static_libunwind=0\n",
                env!("CARGO_PKG_VERSION")
            ),
        )
        .unwrap();
        let facts = serde_json::json!({"schema":1, "commit_hash":QUALIFIED_COMMIT, "target":TARGET,
            "simd128":"indirect", "personality":"rust_eh_personality",
            "rustc_vv":format!("release: 1.99.0\ncommit-hash: {QUALIFIED_COMMIT}\nhost: {TARGET}")});
        fs::write(
            root.join("abi-facts.bundle"),
            serde_json::to_vec(&facts).unwrap(),
        )
        .unwrap();
        checksums(root);
    }
    fn archive(root: &Path, output: &Path, extra: Option<(&str, &[u8], tar::EntryType)>) {
        let encoder = zstd::Encoder::new(File::create(output).unwrap(), 1).unwrap();
        let mut tar = tar::Builder::new(encoder);
        for name in FILES.iter().copied().chain(["SHA256SUMS"]) {
            tar.append_path_with_name(root.join(name), name).unwrap();
        }
        if let Some((name, data, kind)) = extra {
            let mut header = tar::Header::new_gnu();
            header.set_size(data.len() as u64);
            header.set_mode(0o755);
            header.set_entry_type(kind);
            if kind.is_symlink() {
                header.set_link_name("/tmp/forbidden").unwrap();
            }
            header.set_cksum();
            tar.append_data(&mut header, name, data).unwrap();
        }
        tar.into_inner().unwrap().finish().unwrap();
        let checksum = format!(
            "{}  {}\n",
            hash_file(output).unwrap(),
            output.file_name().unwrap().to_str().unwrap()
        );
        fs::write(output.with_extension("zst.sha256"), checksum).unwrap();
    }

    #[test]
    fn detects_full_identity_and_rejects_unknown_or_malformed_output() {
        assert!(qualified().qualified());
        for text in [
            "release: 1.99.0",
            "commit-hash: unknown\nrelease: 1.99.0\nhost: x",
            "release: a\nrelease: b",
        ] {
            assert!(Toolchain::parse(text).is_err(), "{text}");
        }
        let mut unknown = qualified();
        unknown.commit.replace_range(..1, "0");
        assert!(!unknown.qualified());
        assert_eq!(
            unknown.fallback_message(),
            format!(
                "[refresh] cargo-refresh has no qualified support for rustc 1.99.0 ({}).\n[refresh] Running normal cargo builds + restart (no live patching).\n[refresh] Supported toolchain: 1.99.0, x86_64-unknown-linux-gnu, default CPU/features.\n[refresh] See docs/install-layout.md for toolchain qualification.",
                unknown.commit
            )
        );
        unknown = qualified();
        unknown.host = "aarch64-unknown-linux-gnu".into();
        assert!(!unknown.qualified());
        unknown = qualified();
        unknown.release = "1.100.0".into();
        assert!(!unknown.qualified());
    }
    #[test]
    fn memory_boundary_includes_reserve_and_cannot_overflow() {
        assert!(memory_allows(15 * GIB, DEFAULT_WARM_PEAK));
        assert!(!memory_allows(15 * GIB - 1, DEFAULT_WARM_PEAK));
        assert!(!memory_allows(u64::MAX, u64::MAX));
    }
    #[test]
    fn verifies_every_payload_identity_and_permission() {
        let temp = tempfile::tempdir().unwrap();
        package(temp.path());
        validate(temp.path(), &qualified()).unwrap();
        fs::write(temp.path().join("rustd"), "corrupt").unwrap();
        assert!(
            validate(temp.path(), &qualified())
                .unwrap_err()
                .to_string()
                .contains("SHA-256")
        );
        package(temp.path());
        fs::write(
            temp.path().join("VERSION"),
            "cargo_refresh=0.1.0\nrustc_commit=wrong\ntarget=x86_64-unknown-linux-gnu\n",
        )
        .unwrap();
        checksums(temp.path());
        assert!(
            validate(temp.path(), &qualified())
                .unwrap_err()
                .to_string()
                .contains("commit mismatch")
        );
        package(temp.path());
        fs::set_permissions(
            temp.path().join("rustd-rustc"),
            fs::Permissions::from_mode(0o644),
        )
        .unwrap();
        assert!(validate(temp.path(), &qualified()).is_err());
        package(temp.path());
        let mut facts: serde_json::Value =
            serde_json::from_slice(&fs::read(temp.path().join("abi-facts.bundle")).unwrap())
                .unwrap();
        facts["commit_hash"] = "0000000000000000000000000000000000000000".into();
        fs::write(
            temp.path().join("abi-facts.bundle"),
            serde_json::to_vec(&facts).unwrap(),
        )
        .unwrap();
        checksums(temp.path());
        assert!(
            validate(temp.path(), &qualified())
                .unwrap_err()
                .to_string()
                .contains("ABI bundle identity")
        );
    }
    #[test]
    fn installs_atomically_and_reuses_verified_cache() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("payload");
        package(&root);
        let file = temp.path().join("support.tar.zst");
        archive(&root, &file, None);
        let cache = temp.path().join("cache");
        let installed = install_archive(&file, &cache, &qualified()).unwrap();
        assert_eq!(installed, cache.join(QUALIFIED_COMMIT));
        validate(&installed, &qualified()).unwrap();
        assert_eq!(
            install_archive(&file, &cache, &qualified()).unwrap(),
            installed
        );
        assert!(!cache.read_dir().unwrap().any(|e| {
            e.unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".install-")
        }));
        fs::write(installed.join("librefresh.so"), "corrupt").unwrap();
        assert!(install_archive(&file, &cache, &qualified()).is_err());
    }
    #[test]
    fn rejects_corrupt_archive_without_exposing_partial_cache() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("payload");
        package(&root);
        let file = temp.path().join("support.tar.zst");
        archive(&root, &file, None);
        let cache = temp.path().join("cache");
        fs::write(&file, b"corrupted archive").unwrap();
        assert!(
            install_archive(&file, &cache, &qualified())
                .unwrap_err()
                .to_string()
                .contains("archive SHA-256")
        );
        assert!(!cache.join(QUALIFIED_COMMIT).exists());
    }
    #[test]
    fn rejects_duplicates_links_unexpected_members_and_traversal() {
        for extra in [
            ("rustd", &b"duplicate"[..], tar::EntryType::Regular),
            ("extra", &b"unexpected"[..], tar::EntryType::Regular),
            ("rustd", &b""[..], tar::EntryType::Symlink),
        ] {
            let temp = tempfile::tempdir().unwrap();
            let root = temp.path().join("payload");
            package(&root);
            let file = temp.path().join("support.tar.zst");
            archive(&root, &file, Some(extra));
            let cache = temp.path().join("cache");
            assert!(install_archive(&file, &cache, &qualified()).is_err());
            assert!(!cache.join(QUALIFIED_COMMIT).exists());
            assert!(!cache.read_dir().unwrap().any(|e| {
                e.unwrap()
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".install-")
            }));
        }
        for path in ["../rustd", "/rustd", "licenses/../../rustd"] {
            assert!(member(Path::new(path)).is_err());
        }
    }
    #[test]
    fn rejects_excessive_expansion_before_reading_payload() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("oversized.tar.zst");
        let mut header = tar::Header::new_gnu();
        header.set_path("rustd").unwrap();
        header.set_entry_type(tar::EntryType::Regular);
        header.set_mode(0o755);
        header.set_size(LIMIT + 1);
        header.set_cksum();
        let mut bytes = header.as_bytes().to_vec();
        bytes.extend_from_slice(&[0; 1024]);
        fs::write(&file, zstd::encode_all(bytes.as_slice(), 1).unwrap()).unwrap();
        fs::write(
            file.with_extension("zst.sha256"),
            format!("{}  oversized.tar.zst\n", hash_file(&file).unwrap()),
        )
        .unwrap();
        let cache = temp.path().join("cache");
        assert!(
            install_archive(&file, &cache, &qualified())
                .unwrap_err()
                .to_string()
                .contains("expansion exceeds")
        );
        assert!(!cache.join(QUALIFIED_COMMIT).exists());
    }

    #[test]
    fn rejects_cached_symlink_and_checksum_omissions() {
        let temp = tempfile::tempdir().unwrap();
        package(temp.path());
        fs::remove_file(temp.path().join("rustd")).unwrap();
        std::os::unix::fs::symlink("/bin/true", temp.path().join("rustd")).unwrap();
        assert!(validate(temp.path(), &qualified()).is_err());
        assert!(sums(&format!("{}  rustd\n", "0".repeat(64))).is_err());
    }
}
