//! Session state for the compiler's cumulative, generation-fenced manifest.
use anyhow::{Result, ensure};
pub use refresh_proto::manifest::*;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

#[derive(Default, Debug)]
pub struct State {
    pub generation: u64,
    pub version: u16,
    pub unwind_regions: Option<Vec<UnwindRegion>>,
    pub symbols: Vec<Symbol>,
    pub cumulative_roots: u64,
    pub retained_bytes: u64,
    pub epoch_due: bool,
    pub segments: Vec<Segment>,
    pub pages: BTreeMap<u64, Arc<Page>>,
    pub writes: BTreeMap<u64, Write>,
    pub compiler_prepare_us: u64,
    pub compiler_scanned_bytes: u64,
    pub compiler_relative_records: u64,
    pub async_certificates: Option<Vec<refresh_proto::async_certs::Certificate>>,
    last: Vec<u8>,
}
impl State {
    /// A repeated manifest is stale. Parsing/validation failures leave state intact.
    pub fn update(&mut self, bytes: &[u8]) -> Result<bool> {
        if bytes == self.last {
            return Ok(false);
        }
        let m = parse(bytes)?;
        ensure!(
            m.generation == 1 || self.generation.checked_add(1) == Some(m.generation),
            "manifest generation gap: {} after {}",
            m.generation,
            self.generation
        );
        let mut ranges: Vec<_> = m
            .segments
            .iter()
            .map(|s| (s.addr, s.addr + s.len))
            .collect();
        ranges.sort_unstable();
        ensure!(
            ranges.windows(2).all(|w| w[0].1 <= w[1].0),
            "overlapping manifest segments"
        );
        let mut changed = BTreeSet::new();
        for page in &m.pages {
            ensure!(changed.insert(page.addr), "duplicate manifest page");
        }
        let mut writes = BTreeMap::new();
        for w in m.writes {
            ensure!(
                writes.insert(w.addr, w).is_none(),
                "duplicate manifest write"
            );
        }
        let mut prior_end = 0;
        for w in writes.values() {
            ensure!(w.addr >= prior_end, "overlapping manifest writes");
            prior_end = w.addr + w.payload.len() as u64;
        }
        let available_pages = changed.len().saturating_add(if m.generation == 1 {
            0
        } else {
            self.pages.len()
        });
        let required_pages = m.segments.iter().try_fold(0_u64, |sum, s| {
            sum.checked_add(s.len.div_ceil(4096))
                .ok_or_else(|| anyhow::anyhow!("page count overflow"))
        })?;
        ensure!(
            required_pages <= available_pages as u64,
            "manifest page count exceeds available pages"
        );
        // Every active page must exist, either in this delta or the prior generation.
        for segment in &m.segments {
            let mut at = segment.addr;
            while at < segment.addr + segment.len {
                ensure!(
                    changed.contains(&at) || (m.generation != 1 && self.pages.contains_key(&at)),
                    "manifest missing page {at:x}"
                );
                at = at
                    .checked_add(4096)
                    .ok_or_else(|| anyhow::anyhow!("page overflow"))?;
            }
        }
        if m.generation == 1 {
            self.pages.clear();
        }
        for p in m.pages {
            self.pages.insert(p.addr, Arc::new(p));
        }
        self.version = m.version;
        self.unwind_regions = m.unwind_regions;
        self.symbols = m.symbols;
        self.cumulative_roots = m.cumulative_roots;
        self.retained_bytes = m.retained_bytes;
        self.epoch_due = m.epoch_due;
        self.segments = m.segments;
        self.writes = writes;
        self.generation = m.generation;
        self.compiler_prepare_us = m.compiler_prepare_us;
        self.compiler_scanned_bytes = m.compiler_scanned_bytes;
        self.compiler_relative_records = m.compiler_relative_records;
        self.async_certificates = m.async_certificates;
        self.last = bytes.to_vec();
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn manifest(generation: u64, page: bool, segment: bool) -> Vec<u8> {
        let mut b = b"RDM09001".to_vec();
        b.extend(generation.to_le_bytes());
        b.extend((u32::from(segment)).to_le_bytes());
        if segment {
            b.extend(0x4000_u64.to_le_bytes());
            b.extend(4096_u64.to_le_bytes());
            b.extend(1_u32.to_le_bytes());
        }
        b.extend((u32::from(page)).to_le_bytes());
        if page {
            b.extend(0x4000_u64.to_le_bytes());
            b.extend(1_u32.to_le_bytes());
            b.extend(0_u32.to_le_bytes());
            b.extend(0_u32.to_le_bytes());
        }
        b.extend([0; 8]);
        b.extend([0; 24]);
        b
    }
    #[test]
    fn deltas_generation_and_atomic_rejection() {
        let mut s = State::default();
        let first = manifest(1, true, true);
        assert!(s.update(&first).unwrap());
        assert!(!s.update(&first).unwrap());
        assert_eq!(s.pages[&0x4000].payload, vec![0; 4096]);
        assert!(s.update(&manifest(3, false, true)).is_err());
        assert_eq!(s.generation, 1);
        assert!(s.update(&manifest(2, false, true)).unwrap());
        assert_eq!(s.pages.len(), 1);
        assert!(s.update(&manifest(1, false, true)).is_err());
        assert_eq!(s.generation, 2);
        assert_eq!(s.pages.len(), 1);
        assert!(s.update(&manifest(1, false, false)).unwrap());
        assert!(s.pages.is_empty());
    }
    #[test]
    fn huge_segment_is_bounded() {
        let mut b = manifest(1, true, true);
        b[28..36].copy_from_slice(&(u64::MAX - 0x8000).to_le_bytes());
        let mut s = State::default();
        assert!(s.update(&b).is_err());
        assert_eq!(s.generation, 0);
    }
}

/// Proofs for the process actually running, separate from compiler generations.
/// Superseded builds accumulate in `pending` until a candidate is delivered.
/// No whole-program certificate set is decoded or copied on a save.
#[derive(Debug, Default)]
pub struct AsyncRuntime {
    current: BTreeMap<String, refresh_proto::async_certs::Descriptor>,
    pending: BTreeMap<String, refresh_proto::async_certs::Certificate>,
    missing: bool,
    uncertified: BTreeSet<String>,
    unknown_owners: BTreeSet<(std::path::PathBuf, String)>,
}
fn owner_identity(name: &str) -> String {
    name.strip_prefix("@line:")
        .and_then(|s| s.split_once("::"))
        .map_or_else(|| name.to_owned(), |(_, method)| format!("::{method}"))
}
fn covers(d: &refresh_proto::async_certs::Descriptor, owner: &refresh_guard::ChangedFn) -> bool {
    let method = if let Some(s) = owner.name.strip_prefix("@line:") {
        let Some((line, method)) = s.split_once("::") else {
            return false;
        };
        let bounds = line.split_once('-').map_or_else(
            || line.parse::<u32>().ok().map(|n| (n, n)),
            |(first, last)| first.parse::<u32>().ok().zip(last.parse::<u32>().ok()),
        );
        if !bounds.is_some_and(|(first, last)| {
            first != 0 && first <= d.owner_line && d.owner_line <= last
        }) {
            return false;
        }
        method
    } else {
        owner.name.trim_start_matches(':')
    };
    d.kind == 1
        && !d.owner_file.is_empty()
        && (owner.path == std::path::Path::new(&d.owner_file)
            || owner.path.ends_with(std::path::Path::new(&d.owner_file)))
        && d.owner_path.ends_with(&format!("::{method}"))
}
impl AsyncRuntime {
    pub fn observe(&mut self, bytes: &[u8]) -> Result<()> {
        let m = parse(bytes)?;
        self.observe_certificates(m.async_certificates.as_deref());
        Ok(())
    }
    pub fn observe_certificates(
        &mut self,
        rows: Option<&[refresh_proto::async_certs::Certificate]>,
    ) {
        match rows {
            Some(rows) => {
                for row in rows {
                    self.pending.insert(row.symbol.clone(), row.clone());
                }
            }
            None => self.missing = true,
        }
    }
    pub fn decide(&self, owners: &[refresh_guard::ChangedFn]) -> std::result::Result<(), String> {
        if self.missing {
            return Err("missing compiler async certificates".into());
        }
        for owner in owners {
            if self
                .unknown_owners
                .contains(&(owner.path.clone(), owner_identity(&owner.name)))
            {
                return Err(format!(
                    "async owner needs a fresh compiler baseline: {}",
                    owner.name
                ));
            }
        }
        for row in self.pending.values() {
            if self.uncertified.contains(&row.symbol) {
                return Err(format!(
                    "missing running async frame/resume proof: {}",
                    row.symbol
                ));
            }
            let old = self.current.get(&row.symbol).or(row.old.as_ref());
            let (Some(old), Some(new)) = (old, row.new.as_ref()) else {
                return Err(format!(
                    "missing compiler frame/resume proof: {}",
                    row.symbol
                ));
            };
            if !old.protected_eq(new) {
                return Err(format!(
                    "compiler frame or protected resume region changed: {}",
                    row.symbol
                ));
            }
        }
        for owner in owners {
            if !self
                .pending
                .values()
                .any(|r| r.new.as_ref().is_some_and(|d| covers(d, owner)))
            {
                return Err(format!(
                    "missing coroutine coverage for {}:{}",
                    owner.path.display(),
                    owner.name
                ));
            }
        }
        Ok(())
    }
    pub fn has_changes(&self) -> bool {
        !self.pending.is_empty()
    }
    /// Call only after the candidate has been applied or restarted successfully.
    pub fn commit(&mut self, owners: &[refresh_guard::ChangedFn]) {
        for owner in owners {
            if self.missing
                || !self
                    .pending
                    .values()
                    .any(|r| r.new.as_ref().is_some_and(|d| covers(d, owner)))
            {
                self.unknown_owners
                    .insert((owner.path.clone(), owner_identity(&owner.name)));
            }
        }
        for (_, row) in std::mem::take(&mut self.pending) {
            if let Some(new) = row.new {
                self.uncertified.remove(&row.symbol);
                self.current.insert(row.symbol, new);
            } else {
                self.current.remove(&row.symbol);
                self.uncertified.insert(row.symbol);
            }
        }
        self.missing = false;
    }
}

#[cfg(test)]
mod async_tests {
    use super::*;
    use refresh_proto::async_certs::{Certificate, Descriptor};
    fn descriptor(resume: u8) -> Descriptor {
        Descriptor {
            kind: 1,
            owner_file: "src/main.rs".into(),
            owner_path: "app::handler".into(),
            owner_line: 42,
            frame: [1; 16],
            entry: [2; 16],
            resumes: vec![(3, [resume; 16])],
        }
    }
    fn owner() -> refresh_guard::ChangedFn {
        refresh_guard::ChangedFn {
            path: "src/main.rs".into(),
            name: "::handler".into(),
        }
    }
    fn row(old: u8, new: u8) -> Certificate {
        Certificate {
            symbol: "poll".into(),
            old: Some(descriptor(old)),
            new: Some(descriptor(new)),
        }
    }
    #[test]
    fn postawait_changes_fail_and_preawait_entry_changes_pass() {
        let mut s = AsyncRuntime::default();
        let mut a = row(1, 1);
        a.new.as_mut().unwrap().entry[0] = 9;
        s.pending.insert("poll".into(), a);
        assert!(s.decide(&[owner()]).is_ok());
        s.pending.insert("poll".into(), row(1, 2));
        assert!(s.decide(&[owner()]).is_err());
        s.commit(&[owner()]); // Candidate restarted with resume region 2.
        s.pending.insert("poll".into(), row(1, 1)); // Compiler's baseline is still region 1.
        assert!(
            s.decide(&[owner()]).is_err(),
            "reverting cannot use the compiler's stale runtime proof"
        );
        s.pending.insert("poll".into(), row(1, 2));
        assert!(s.decide(&[owner()]).is_ok());
    }
    #[test]
    fn missing_unknown_or_partial_owner_coverage_fails_closed() {
        let mut s = AsyncRuntime::default();
        assert!(s.decide(&[owner()]).is_err());
        s.pending.insert("poll".into(), row(1, 1));
        let mut other = owner();
        other.name = "::other".into();
        assert!(s.decide(&[owner(), other]).is_err());
        s.pending.get_mut("poll").unwrap().old = None;
        assert!(s.decide(&[owner()]).is_err());
        s.pending.insert("poll".into(), row(1, 1));
        s.missing = true;
        assert!(s.decide(&[owner()]).is_err());
    }
    #[test]
    fn method_coverage_requires_the_compiler_declaration_line() {
        let mut s = AsyncRuntime::default();
        s.pending.insert("poll".into(), row(1, 1));
        let mut method = owner();
        method.name = "@line:42::handler".into();
        assert!(s.decide(&[method.clone()]).is_ok());
        method.name = "@line:43::handler".into();
        assert!(s.decide(&[method]).is_err());
    }
    #[test]
    fn method_attribute_anchors_require_the_original_header_range() {
        let mut s = AsyncRuntime::default();
        s.pending.insert("poll".into(), row(1, 1));
        for (name, allowed) in [
            ("@line:40-42::handler", true),
            ("@line:42-44::handler", true),
            ("@line:43-44::handler", false),
            ("@line:40-41::handler", false),
            ("@line:44-40::handler", false),
            ("@line:0-42::handler", false),
            ("@line:garbage::handler", false),
        ] {
            let mut method = owner();
            method.name = name.into();
            assert_eq!(s.decide(&[method]).is_ok(), allowed, "{name}");
        }
    }
    #[test]
    fn uncertified_runtime_cannot_reuse_the_original_baseline() {
        let mut s = AsyncRuntime::default();
        let mut missing = row(1, 2);
        missing.new = None;
        s.pending.insert("poll".into(), missing);
        assert!(s.decide(&[]).is_err());
        s.commit(&[]); // A restarted candidate has a missing runtime descriptor.
        s.pending.insert("poll".into(), row(1, 1));
        assert!(s.decide(&[]).is_err());
        s.commit(&[]); // Restarting with a known descriptor establishes it again.
        s.pending.insert("poll".into(), row(1, 1));
        assert!(s.decide(&[]).is_ok());
    }
    #[test]
    fn superseded_proofs_remain_pending_until_delivery() {
        let mut s = AsyncRuntime::default();
        s.pending.insert("poll".into(), row(1, 2));
        let owners = [owner()];
        assert!(s.decide(&owners).is_err());
        // No runtime commit occurred. Replacing the pending proof with the original
        // region is safe for the still-running original process.
        s.pending.insert("poll".into(), row(1, 1));
        assert!(s.decide(&owners).is_ok());
        s.commit(&owners);
        assert!(s.pending.is_empty());
    }
}
