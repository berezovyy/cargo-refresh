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
