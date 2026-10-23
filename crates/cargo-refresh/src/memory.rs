//! Linux memory budget shared by compiler admission and runtime protection.
//! Cgroup file cache is charged by memory.current, just like anonymous memory.
use anyhow::{Context, Result, ensure};
use std::{
    fs,
    io::ErrorKind,
    path::{Component, Path, PathBuf},
};

pub const GIB: u64 = 1024 * 1024 * 1024;
pub const RESERVE: u64 = GIB;
pub const INTERVAL: std::time::Duration = std::time::Duration::from_millis(500);

#[derive(Debug, Clone, Copy)]
pub struct Sample {
    pub available: u64,
    /// Largest full-stall avg10 across host and cgroup ancestors (percent).
    pub full_pressure: f64,
}
impl Sample {
    pub fn pressured(self, reserve: u64) -> bool {
        self.available < reserve
            || (self.available < reserve.saturating_mul(2) && self.full_pressure >= 10.0)
    }
}
pub fn host_available() -> Result<u64> {
    parse_host(&fs::read_to_string("/proc/meminfo")?)
}
fn parse_host(text: &str) -> Result<u64> {
    text.lines()
        .find_map(|l| {
            l.strip_prefix("MemAvailable:")
                .and_then(|s| s.split_whitespace().next())
        })
        .context("MemAvailable missing")?
        .parse::<u64>()?
        .checked_mul(1024)
        .context("MemAvailable overflow")
}
fn pressure(path: &Path) -> f64 {
    fs::read_to_string(path)
        .ok()
        .and_then(|text| {
            text.lines()
                .find(|l| l.starts_with("full "))
                .and_then(|line| {
                    line.split_whitespace()
                        .find_map(|field| field.strip_prefix("avg10="))
                        .and_then(|value| value.parse::<f64>().ok())
                })
        })
        .filter(|v| v.is_finite() && *v >= 0.0)
        .unwrap_or(0.0)
}
fn unescape_mount(text: &str) -> String {
    text.replace("\\040", " ")
        .replace("\\011", "\t")
        .replace("\\012", "\n")
        .replace("\\134", "\\")
}
/// Resolve unified membership against mountinfo, including namespaced/subtree mounts.
fn location(membership: &str, mounts: &str) -> Result<Option<(PathBuf, PathBuf)>> {
    let Some(group) = membership.lines().find_map(|l| l.strip_prefix("0::")) else {
        return Ok(None); // cgroup v1: host-only fallback.
    };
    let group = Path::new(group);
    ensure!(
        group.is_absolute()
            && !group
                .components()
                .any(|c| matches!(c, Component::ParentDir)),
        "invalid cgroup membership"
    );
    let mut selected: Option<(usize, PathBuf, PathBuf)> = None;
    for line in mounts.lines() {
        let Some((left, right)) = line.split_once(" - ") else {
            continue;
        };
        if right.split_whitespace().next() != Some("cgroup2") {
            continue;
        }
        let fields: Vec<_> = left.split_whitespace().collect();
        ensure!(fields.len() >= 5, "invalid cgroup2 mountinfo");
        let root = PathBuf::from(unescape_mount(fields[3]));
        let mount = PathBuf::from(unescape_mount(fields[4]));
        if let Ok(relative) = group.strip_prefix(&root) {
            let depth = root.components().count();
            // A bind mount of a subtree must not hide a limiting ancestor
            // when a broader view of the same hierarchy is also available.
            if selected
                .as_ref()
                .is_none_or(|(old_depth, _, _)| depth < *old_depth)
            {
                selected = Some((depth, mount.clone(), mount.join(relative)));
            }
        }
    }
    selected
        .map(|(_, mount, leaf)| Some((mount, leaf)))
        .context("unified cgroup mount not found")
}
fn in_tree(mut sample: Sample, mount: &Path, leaf: &Path) -> Result<Sample> {
    ensure!(leaf.starts_with(mount), "cgroup outside mount");
    for path in leaf.ancestors() {
        if !path.starts_with(mount) {
            break;
        }
        match fs::read_to_string(path.join("memory.max")) {
            Ok(max) => {
                if max.trim() != "max" {
                    let limit = max.trim().parse::<u64>().context("invalid memory.max")?;
                    let current = fs::read_to_string(path.join("memory.current"))?
                        .trim()
                        .parse::<u64>()
                        .context("invalid memory.current")?;
                    sample.available = sample.available.min(limit.saturating_sub(current));
                }
            }
            Err(e) if e.kind() == ErrorKind::NotFound => {} // controller not enabled here
            Err(e) => return Err(e.into()),
        }
        sample.full_pressure = sample
            .full_pressure
            .max(pressure(&path.join("memory.pressure")));
        if path == mount {
            break;
        }
    }
    Ok(sample)
}
pub fn sample() -> Result<Sample> {
    let sample = Sample {
        available: host_available()?,
        full_pressure: pressure(Path::new("/proc/pressure/memory")),
    };
    let membership = fs::read_to_string("/proc/self/cgroup")?;
    let mounts = fs::read_to_string("/proc/self/mountinfo")?;
    match location(&membership, &mounts)? {
        Some((mount, leaf)) => in_tree(sample, &mount, &leaf),
        None => Ok(sample),
    }
}
pub fn allows(available: u64, peak: u64) -> bool {
    peak.checked_add(RESERVE)
        .is_some_and(|needed| available >= needed)
}

/// Recovery policy is independent of wall-clock sampling, so pressure flapping
/// and retry timing can be checked without sleeping or allocating filler memory.
#[derive(Debug)]
pub struct Recovery {
    retry_after: std::time::Instant,
    healthy_since: Option<std::time::Instant>,
}
impl Recovery {
    pub fn new(now: std::time::Instant) -> Self {
        Self {
            retry_after: now,
            healthy_since: None,
        }
    }
    pub fn cooldown(&mut self, now: std::time::Instant) {
        self.retry_after = now + std::time::Duration::from_secs(30);
        self.healthy_since = None;
    }
    pub fn reset_stability(&mut self) {
        self.healthy_since = None;
    }
    pub fn can_start(&self, now: std::time::Instant) -> bool {
        now >= self.retry_after
    }
    pub fn observe(
        &mut self,
        now: std::time::Instant,
        sample: Sample,
        peak: u64,
        reserve: u64,
    ) -> bool {
        if sample.pressured(reserve)
            || !allows(sample.available.saturating_sub(512 * 1024 * 1024), peak)
        {
            self.healthy_since = None;
            return false;
        }
        let since = *self.healthy_since.get_or_insert(now);
        self.can_start(now)
            && now.saturating_duration_since(since) >= std::time::Duration::from_secs(5)
    }
}

/// One-time project model: fixed runtime/stdlib overhead + owned physical Rust
/// lines + linked dependency packages. Coefficients are documented in docs/memory.md.
#[derive(Debug, Clone, Copy, serde::Serialize)]
pub struct Estimate {
    pub rust_lines: u64,
    pub dependencies: usize,
    pub peak_bytes: u64,
}
impl Estimate {
    pub fn from_counts(rust_lines: u64, dependencies: usize) -> Self {
        // First 300 packages approximate graph/bootstrap cost; marginal cost
        // falls for the larger observed dependency closures. LOC remains linear.
        let dependencies_u64 = dependencies as u64;
        let dependency_bytes = dependencies_u64
            .min(300)
            .saturating_mul(16 * 1024 * 1024)
            .saturating_add(
                dependencies_u64
                    .saturating_sub(300)
                    .saturating_mul(4 * 1024 * 1024),
            );
        let peak_bytes = (5_u64 * GIB / 2)
            .saturating_add(rust_lines.saturating_mul(192 * 1024))
            .saturating_add(dependency_bytes);
        Self {
            rust_lines,
            dependencies,
            peak_bytes,
        }
    }
    pub fn project(roots: &[PathBuf], dependencies: usize) -> Result<Self> {
        let mut seen = std::collections::BTreeSet::new();
        let mut lines = 0_u64;
        for root in roots {
            if !root.exists() {
                continue;
            }
            for entry in walkdir::WalkDir::new(root)
                .into_iter()
                .filter_entry(|e| !matches!(e.file_name().to_str(), Some("target" | ".git")))
            {
                let entry = entry?;
                if entry.file_type().is_file()
                    && entry.path().extension().is_some_and(|ext| ext == "rs")
                {
                    let path = entry.path().canonicalize()?;
                    if seen.insert(path.clone()) {
                        let bytes = fs::read(path)?;
                        lines = lines
                            .saturating_add(bytes.split_inclusive(|b| *b == b'\n').count() as u64);
                    }
                }
            }
        }
        Ok(Self::from_counts(lines, dependencies))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn write(path: &Path, name: &str, text: &str) {
        fs::create_dir_all(path).unwrap();
        fs::write(path.join(name), text).unwrap();
    }
    #[test]
    fn nested_limits_unlimited_saturation_and_host_minimum() {
        let tree = tempfile::tempdir().unwrap();
        let root = tree.path();
        let leaf = root.join("parent/child");
        write(root, "memory.max", "max\n");
        write(&root.join("parent"), "memory.max", "1000\n");
        write(&root.join("parent"), "memory.current", "650\n");
        write(&leaf, "memory.max", "800\n");
        write(&leaf, "memory.current", "100\n");
        let sample = Sample {
            available: 900,
            full_pressure: 0.0,
        };
        assert_eq!(in_tree(sample, root, &leaf).unwrap().available, 350);
        assert_eq!(
            in_tree(
                Sample {
                    available: 200,
                    ..sample
                },
                root,
                &leaf
            )
            .unwrap()
            .available,
            200
        );
        write(&leaf, "memory.current", "900");
        assert_eq!(in_tree(sample, root, &leaf).unwrap().available, 0);
        write(&leaf, "memory.max", "broken");
        assert!(in_tree(sample, root, &leaf).is_err());
        write(&leaf, "memory.max", "800");
        fs::remove_file(leaf.join("memory.current")).unwrap();
        assert!(in_tree(sample, root, &leaf).is_err());
    }
    #[test]
    fn unified_namespaced_mount_and_v1_fallback() {
        let mount = "36 28 0:30 /tenant /sys/fs/cgroup rw - cgroup2 cgroup2 rw\n";
        assert_eq!(
            location("0::/tenant/a/b\n", mount).unwrap(),
            Some(("/sys/fs/cgroup".into(), "/sys/fs/cgroup/a/b".into()))
        );
        assert!(location("5:memory:/a\n", "").unwrap().is_none());
        assert!(location("0::/a/../b\n", mount).is_err());
        assert!(location("0::/a\n", "").is_err());
    }
    #[test]
    fn broader_mount_preserves_visible_ancestor_limit() {
        let tree = tempfile::tempdir().unwrap();
        let full = tree.path().join("full");
        let short = tree.path().join("short");
        let leaf = full.join("tenant/leaf");
        write(&full, "memory.max", "1000");
        write(&full, "memory.current", "900");
        write(&full.join("tenant"), "memory.max", "max");
        write(&leaf, "memory.max", "1000");
        write(&leaf, "memory.current", "100");
        std::os::unix::fs::symlink(full.join("tenant"), &short).unwrap();
        let mounts = format!(
            "1 0 0:1 /tenant {} rw - cgroup2 cgroup2 rw\n2 0 0:1 / {} rw - cgroup2 cgroup2 rw\n",
            short.display(),
            full.display()
        );
        let (mount, member) = location("0::/tenant/leaf\n", &mounts).unwrap().unwrap();
        let budget = in_tree(
            Sample {
                available: 2000,
                full_pressure: 0.0,
            },
            &mount,
            &member,
        )
        .unwrap();
        assert_eq!(budget.available, 100);
    }

    #[test]
    fn psi_is_optional_and_only_accelerates_low_headroom_degrade() {
        let tree = tempfile::tempdir().unwrap();
        let root = tree.path();
        write(
            root,
            "memory.pressure",
            "some avg10=99.0 avg60=0\nfull avg10=12.5 avg60=0\n",
        );
        let sample = in_tree(
            Sample {
                available: RESERVE + 1,
                full_pressure: 0.0,
            },
            root,
            root,
        )
        .unwrap();
        assert!(sample.pressured(RESERVE));
        assert!(
            !Sample {
                available: 3 * RESERVE,
                ..sample
            }
            .pressured(RESERVE)
        );
        fs::remove_file(root.join("memory.pressure")).unwrap();
        assert_eq!(pressure(&root.join("memory.pressure")), 0.0);
        assert!(!allows(u64::MAX, u64::MAX));
        assert!(allows(3 * GIB, 2 * GIB));
    }
    #[test]
    fn recovery_requires_stability_hysteresis_and_failure_cooldown() {
        use std::time::{Duration, Instant};
        let at = Instant::now();
        let mut recovery = Recovery::new(at);
        let healthy = Sample {
            available: 4 * GIB,
            full_pressure: 0.0,
        };
        let marginal = Sample {
            available: 3 * GIB,
            ..healthy
        };
        assert!(!recovery.observe(at, healthy, 2 * GIB, RESERVE));
        assert!(!recovery.observe(at + Duration::from_secs(4), healthy, 2 * GIB, RESERVE));
        assert!(!recovery.observe(at + Duration::from_secs(5), marginal, 2 * GIB, RESERVE));
        assert!(!recovery.observe(at + Duration::from_secs(6), healthy, 2 * GIB, RESERVE));
        assert!(recovery.observe(at + Duration::from_secs(11), healthy, 2 * GIB, RESERVE));
        recovery.cooldown(at + Duration::from_secs(11));
        assert!(!recovery.observe(at + Duration::from_secs(12), healthy, 2 * GIB, RESERVE));
        assert!(!recovery.observe(at + Duration::from_secs(40), healthy, 2 * GIB, RESERVE));
        assert!(recovery.observe(at + Duration::from_secs(41), healthy, 2 * GIB, RESERVE));
        assert!(!recovery.observe(
            at + Duration::from_secs(42),
            Sample {
                available: GIB - 1,
                ..healthy
            },
            2 * GIB,
            RESERVE
        ));
        assert!(!recovery.observe(at + Duration::from_secs(43), healthy, 2 * GIB, RESERVE));
    }

    #[test]
    fn loc_deduplicates_overlapping_roots_and_ignores_targets() {
        let tree = tempfile::tempdir().unwrap();
        let root = tree.path();
        write(root, "lib.rs", "// comment\nfn a() {}\n");
        write(&root.join("sub"), "a.rs", "fn b() {}");
        write(&root.join("target"), "generated.rs", "skip\n");
        let e = Estimate::project(&[root.to_owned(), root.join("sub")], 7).unwrap();
        assert_eq!(e.rust_lines, 3);
        assert_eq!(e.dependencies, 7);
        assert!(Estimate::from_counts(10000, 100).peak_bytes > e.peak_bytes);
    }
}
