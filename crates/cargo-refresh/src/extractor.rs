//! Stateful manifest → v0 records. Commit only after a successful agent ACK.
use crate::{
    data_policy::{self, Memory},
    elf::{Image, Section, Symbol, u64_at},
    manifest::State,
    unwind,
};
use anyhow::{Context, Result, ensure};
use refresh_proto::wire::{self, Record};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
    sync::Arc,
};

struct SparseImage<'a> {
    base: &'a Image,
    state: &'a State,
}
impl Memory for SparseImage<'_> {
    fn read(&self, addr: u64, n: usize) -> Result<Vec<u8>> {
        let end = addr
            .checked_add(n as u64)
            .context("constant range overflow")?;
        if self
            .state
            .segments
            .iter()
            .any(|s| s.addr <= addr && end <= s.addr + s.len)
        {
            let mut at = addr;
            let mut out = Vec::with_capacity(n);
            while at < end {
                let page = at & !4095;
                let offset = (at - page) as usize;
                let take = ((end - at) as usize).min(4096 - offset);
                let bytes = &self
                    .state
                    .pages
                    .get(&page)
                    .context("missing sparse page")?
                    .payload;
                out.extend_from_slice(&bytes[offset..offset + take]);
                at += take as u64;
            }
            return Ok(out);
        }
        let mut out = self.base.read(addr, n)?;
        for (&a, w) in self.state.writes.range(..end) {
            let lo = addr.max(a);
            let hi = end.min(a + w.payload.len() as u64);
            if lo < hi {
                out[(lo - addr) as usize..(hi - addr) as usize]
                    .copy_from_slice(&w.payload[(lo - a) as usize..(hi - a) as usize]);
            }
        }
        Ok(out)
    }
}
#[derive(Default)]
struct Applied {
    pages: BTreeMap<u64, Arc<Record>>,
    jumps: BTreeSet<u64>,
    data: BTreeMap<u64, Vec<u8>>,
}
pub struct PreparedPatch {
    pub records: Vec<Record>,
    pub generation: u64,
    /// Symbols emitted by the compiler in this generation, for later backtrace lookup.
    pub symbols: Vec<crate::manifest::Symbol>,
    serial: u64,
    next: Applied,
}
impl PreparedPatch {
    pub fn encode(&self) -> Result<Vec<u8>> {
        Ok(wire::encode(&self.records)?)
    }
}
pub struct ManifestExtractor {
    pub base: Image,
    functions: BTreeSet<u64>,
    objects: BTreeMap<u64, Vec<Symbol>>,
    sections: BTreeMap<usize, Section>,
    original_loads: BTreeSet<(u64, u64, u32)>,
    allow_tracing: bool,
    applied: Applied,
    serial: u64,
    last_generation: u64,
    promotion_generation: u64,
}
impl ManifestExtractor {
    pub fn new(baseline: impl AsRef<Path>, allow_tracing: bool) -> Result<Self> {
        Self::from_bytes(std::fs::read(baseline)?, allow_tracing)
    }
    pub fn from_bytes(bytes: Vec<u8>, allow_tracing: bool) -> Result<Self> {
        let base = Image::parse(bytes, true)?;
        let (functions, objects, sections) = base.index()?;
        let original_loads = base.loads().map(|p| p.identity()).collect();
        Ok(Self {
            base,
            functions,
            objects,
            sections,
            original_loads,
            allow_tracing,
            applied: Applied::default(),
            serial: 0,
            last_generation: 0,
            promotion_generation: 0,
        })
    }
    /// Install an already validated, immutable restart baseline. Existing data
    /// symbols keep their offsets because every original PT_LOAD is retained.
    /// Only changed function entries need to be added from the promotion catalog.
    pub fn promote(&mut self, base: Image, catalog: &[u8]) -> Result<()> {
        #[derive(serde::Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Delta {
            version: u32,
            generation: u64,
            functions: Vec<u64>,
        }
        #[derive(serde::Deserialize)]
        struct LegacySymbol {
            address: u64,
            size: u64,
            text: bool,
        }
        #[derive(serde::Deserialize)]
        #[serde(untagged)]
        enum Catalog {
            Delta(Delta),
            Legacy(Vec<LegacySymbol>),
        }
        ensure!(base.typ == self.base.typ, "promotion ELF type changed");
        ensure!(
            self.base.loads().all(|p| base.loads().any(|q| p == q)),
            "promotion changed original loaded mappings"
        );
        let next = self
            .promotion_generation
            .checked_add(1)
            .context("promotion sequence overflow")?;
        let functions = match serde_json::from_slice::<Catalog>(catalog)? {
            Catalog::Delta(delta) => {
                ensure!(delta.version == 1, "unsupported promotion catalog version");
                ensure!(
                    delta.generation == next,
                    "stale or out-of-order promotion catalog"
                );
                delta.functions
            }
            Catalog::Legacy(symbols) => symbols
                .into_iter()
                .filter(|s| s.text && s.size >= 5)
                .map(|s| s.address)
                .collect(),
        };
        for &addr in &functions {
            ensure!(
                base.loads()
                    .any(|p| p.flags & 1 != 0 && p.contains(addr, 5, true)),
                "promotion function outside executable mapping: {addr:x}"
            );
        }
        let loads = base.loads().map(|p| p.identity()).collect();
        self.base = base;
        self.original_loads = loads;
        self.functions.extend(functions);
        self.applied = Applied::default();
        self.serial += 1;
        self.last_generation = 0;
        self.promotion_generation = next;
        Ok(())
    }
    /// Promotion catalogs can add entry symbols absent from retained ELF sections.
    pub fn add_function(&mut self, addr: u64) {
        self.functions.insert(addr);
    }
    pub fn extract(&self, state: &State, candidate: &Image) -> Result<PreparedPatch> {
        ensure!(state.generation > 0, "compiler manifest missing");
        ensure!(
            state.generation > self.last_generation,
            "stale manifest already applied or older"
        );
        self.validate_candidate(state, candidate)?;
        let new = SparseImage {
            base: &self.base,
            state,
        };
        let mut jumps = BTreeMap::new();
        let mut data = BTreeMap::new();
        for (&addr, w) in &state.writes {
            if w.kind == 1 {
                ensure!(
                    self.functions.contains(&addr) && w.payload.len() == 5 && w.payload[0] == 0xe9,
                    "invalid original entry jump {addr:x}"
                );
                let delta = i32::from_le_bytes(w.payload[1..5].try_into()?);
                let target = addr
                    .checked_add(5)
                    .and_then(|a| a.checked_add_signed(delta as i64))
                    .context("entry target overflow")?;
                ensure!(
                    state
                        .segments
                        .iter()
                        .any(|s| s.prot & 4 != 0 && s.addr <= target && target < s.addr + s.len),
                    "jump target outside appended executable memory"
                );
                if w.payload != self.base.read(addr, 5)? {
                    jumps.insert(addr, w.payload.clone());
                }
            } else {
                let off = self.base.offset(addr, w.payload.len() as u64)?;
                let owners = self
                    .objects
                    .get(&off)
                    .context("manifest unknown original data")?;
                let mut owners = owners
                    .iter()
                    .filter(|s| s.offset <= off && off < s.offset + s.size);
                let sym = owners.next().context("manifest unknown original data")?;
                ensure!(
                    owners.next().is_none(),
                    "manifest ambiguous original data {addr:x}"
                );
                ensure!(
                    w.payload.len() as u64 == sym.size,
                    "partial original data write"
                );
                if w.payload == self.base.read(addr, w.payload.len())? {
                    continue;
                }
                let section = self
                    .sections
                    .get(&sym.section)
                    .context("missing symbol section")?;
                let (payload, _) = data_policy::classify_record(
                    &self.base,
                    &new,
                    sym,
                    section,
                    self.allow_tracing,
                )?;
                data.insert(addr, payload);
            }
        }
        let mut records = Vec::new();
        let mut pages = BTreeMap::new();
        for s in &state.segments {
            let mut page = s.addr;
            while page < s.addr + s.len {
                let p = state.pages.get(&page).context("missing manifest page")?;
                let prior = self.applied.pages.get(&page);
                let same = prior.is_some_and(|r| {
                    r.prot == s.prot && r.payload == p.payload && r.fixups == p.fixups
                });
                let record = if same {
                    Arc::clone(prior.unwrap())
                } else {
                    Arc::new(Record {
                        addr: page,
                        prot: s.prot,
                        kind: 0,
                        payload: p.payload.clone(),
                        fixups: p.fixups.clone(),
                    })
                };
                if !same {
                    records.push((*record).clone());
                }
                pages.insert(page, record);
                page += 4096;
            }
        }
        let current_jumps: BTreeSet<_> = jumps.keys().copied().collect();
        for &addr in self.applied.jumps.union(&current_jumps) {
            let payload = match jumps.get(&addr) {
                Some(b) => b.clone(),
                None => self.base.read(addr, 5)?,
            };
            records.push(Record {
                addr,
                prot: 5,
                kind: 1,
                payload,
                fixups: vec![],
            });
        }
        let data_addrs: BTreeSet<_> = self
            .applied
            .data
            .keys()
            .chain(data.keys())
            .copied()
            .collect();
        for addr in data_addrs {
            let size = data
                .get(&addr)
                .or_else(|| self.applied.data.get(&addr))
                .unwrap()
                .len();
            let payload = match data.get(&addr) {
                Some(b) => b.clone(),
                None => self.base.read(addr, size)?,
            };
            let fixups = if size >= 8 {
                self.base
                    .relatives
                    .range(addr..=addr + size as u64 - 8)
                    .map(|(&at, _)| (at - addr) as u32)
                    .collect()
            } else {
                vec![]
            };
            records.push(Record {
                addr,
                prot: 1,
                kind: 2,
                payload,
                fixups,
            });
        }
        if let Some(regions) = &state.unwind_regions {
            for region in regions {
                let end = region
                    .addr
                    .checked_add(region.len)
                    .context("unwind overflow")?;
                ensure!(
                    region.len > 0
                        && state.segments.iter().any(|s| s.prot == 1
                            && s.addr <= region.addr
                            && end
                                <= (s.addr + s.len)
                                    .checked_add(4095)
                                    .map(|v| v & !4095)
                                    .unwrap_or(0)),
                    "manifest unwind outside read-only memory"
                );
                records.push(Record {
                    addr: region.addr,
                    prot: 1,
                    kind: 3,
                    payload: region.len.to_le_bytes().to_vec(),
                    fixups: vec![],
                });
            }
        } else {
            records.extend(unwind::descriptors(candidate, &self.original_loads)?);
        }
        ensure!(records.len() <= wire::MAX_RECORDS, "too many agent records");
        for r in &records {
            r.validate()?;
        }
        Ok(PreparedPatch {
            records,
            generation: state.generation,
            symbols: state.symbols.clone(),
            serial: self.serial,
            next: Applied {
                pages,
                jumps: current_jumps,
                data,
            },
        })
    }
    pub fn commit(&mut self, patch: PreparedPatch) -> Result<()> {
        ensure!(patch.serial == self.serial, "superseded prepared patch");
        self.applied = patch.next;
        self.last_generation = patch.generation;
        self.serial += 1;
        Ok(())
    }
    fn validate_candidate(&self, state: &State, new: &Image) -> Result<()> {
        ensure!(new.typ == self.base.typ, "ELF type changed");
        let filter = |image: &Image| {
            image
                .dynamic
                .iter()
                .filter(|(k, _)| !matches!(k, 7 | 8))
                .copied()
                .collect::<Vec<_>>()
        };
        ensure!(
            filter(&self.base) == filter(new),
            "unexpected dynamic table change"
        );
        ensure!(
            self.base.loads().all(|p| new.loads().any(|q| p == q)),
            "original loaded mappings changed"
        );
        ensure!(
            self.base
                .headers
                .iter()
                .filter(|p| p.typ == 7)
                .eq(new.headers.iter().filter(|p| p.typ == 7)),
            "TLS mapping changed"
        );
        let appended: Vec<_> = new
            .loads()
            .filter(|p| !self.original_loads.contains(&p.identity()) && p.offset != new.phoff)
            .collect();
        for s in &state.segments {
            ensure!(
                appended.iter().any(|p| p.contains(s.addr, s.len, false)
                    && s.prot == (((p.flags & 4) >> 2) | (p.flags & 2) | ((p.flags & 1) << 2))),
                "manifest segment not in candidate LOAD"
            );
        }
        // The manifest writer normalizes payloads. Validate appended relocations
        // against the candidate table without scanning original loaded bytes.
        if let Some(table) = new.relocation_table()? {
            for r in table.as_chunks::<24>().0 {
                let at = u64_at(r, 0)?;
                let info = u64_at(r, 8)?;
                if appended.iter().any(|p| p.contains(at, 1, false)) {
                    ensure!(info == 8, "unsupported appended dynamic relocation");
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        manifest::{Page, Segment, Write},
        test_images::{candidate, image, put},
    };
    fn setup() -> (ManifestExtractor, State, Image) {
        let mut base = image();
        put(&mut base, 0x1100, &[0x55, 0x48, 0x89, 0xe5, 0x90]);
        let mut x = ManifestExtractor {
            base,
            functions: BTreeSet::from([0x1100]),
            objects: BTreeMap::new(),
            sections: BTreeMap::new(),
            original_loads: BTreeSet::from([(0x1000, 0x1000, 4)]),
            allow_tracing: false,
            applied: Applied::default(),
            serial: 0,
            last_generation: 0,
            promotion_generation: 0,
        };
        // A Location record in original immutable memory.
        put(&mut x.base, 0x1500, b"src/lib.rs");
        put(&mut x.base, 0x1200, &0x1500_u64.to_le_bytes());
        put(&mut x.base, 0x1208, &10_u64.to_le_bytes());
        put(&mut x.base, 0x1210, &10_u32.to_le_bytes());
        put(&mut x.base, 0x1214, &2_u32.to_le_bytes());
        x.base.relatives.insert(0x1200, 0x1500);
        x.objects.insert(
            0x200,
            vec![Symbol {
                name: "rustd::static::location".into(),
                addr: 0x1200,
                size: 24,
                typ: 1,
                section: 1,
                offset: 0x200,
            }],
        );
        x.sections.insert(
            1,
            Section {
                name: ".rodata".into(),
                flags: 2,
            },
        );
        let mut s = State::default();
        s.generation = 1;
        s.segments = vec![
            Segment {
                addr: 0x4000,
                len: 0x1000,
                prot: 1,
            },
            Segment {
                addr: 0x8000,
                len: 0x1000,
                prot: 5,
            },
        ];
        let c = candidate();
        for seg in &s.segments {
            s.pages.insert(
                seg.addr,
                Arc::new(Page {
                    addr: seg.addr,
                    prot: seg.prot,
                    payload: c.read(seg.addr, 4096).unwrap(),
                    fixups: vec![],
                }),
            );
        }
        let mut jump = vec![0xe9];
        jump.extend((0x8000_i32 - 0x1100 - 5).to_le_bytes());
        s.writes.insert(
            0x1100,
            Write {
                addr: 0x1100,
                kind: 1,
                payload: jump,
                fixups: vec![],
                name: vec![],
            },
        );
        (x, s, c)
    }
    #[test]
    fn promotion_retains_helpers_and_data_index_and_rejects_stale_catalogs() {
        let (mut x, _, _) = setup();
        x.promote(
            candidate(),
            br#"{"version":1,"generation":1,"functions":[32768]}"#,
        )
        .unwrap();
        assert!(x.functions.contains(&0x1100));
        assert!(x.functions.contains(&0x8000));
        assert!(x.objects.contains_key(&0x200));
        // Refusals leave the usable baseline and its sequence intact.
        for catalog in [
            br#"{"version":1,"generation":1,"functions":[]}"#.as_slice(),
            br#"{"version":1,"generation":3,"functions":[]}"#.as_slice(),
            br#"{"version":2,"generation":2,"functions":[]}"#.as_slice(),
            br#"{"version":1,"generation":2,"functions":[16384]}"#.as_slice(),
            br#"{"version":1,"generation":2,"functions":[36863]}"#.as_slice(),
        ] {
            assert!(x.promote(candidate(), catalog).is_err());
            assert_eq!(x.promotion_generation, 1);
        }
        // An unrelated second promotion need not repeat the helper from the first.
        x.promote(
            candidate(),
            br#"{"version":1,"generation":2,"functions":[]}"#,
        )
        .unwrap();
        assert!(x.functions.contains(&0x8000));
        assert!(x.objects.contains_key(&0x200));
        // A later patch can still redirect that retained helper entry.
        let mut next = candidate();
        let crate::elf::ImageBytes::Owned(bytes) = &mut next.data else {
            unreachable!()
        };
        bytes.resize(0x4000, 0);
        next.headers
            .push(crate::test_images::program(1, 5, 0x3000, 0xc000, 0x1000));
        let mut state = State::default();
        state.generation = 1;
        state.segments = vec![Segment {
            addr: 0xc000,
            len: 4096,
            prot: 5,
        }];
        state.unwind_regions = Some(vec![]);
        state.pages.insert(
            0xc000,
            Arc::new(Page {
                addr: 0xc000,
                prot: 5,
                payload: vec![0x90; 4096],
                fixups: vec![],
            }),
        );
        let mut jump = vec![0xe9];
        jump.extend_from_slice(&(0xc000_i32 - 0x8000 - 5).to_le_bytes());
        state.writes.insert(
            0x8000,
            Write {
                addr: 0x8000,
                kind: 1,
                payload: jump,
                fixups: vec![],
                name: vec![],
            },
        );
        let patch = x.extract(&state, &next).unwrap();
        assert!(
            patch
                .records
                .iter()
                .any(|r| r.kind == 1 && r.addr == 0x8000)
        );
        x.commit(patch).unwrap();
        // Compatibility with a compiler that does not implement delta catalogs.
        x.promote(
            candidate(),
            br#"[{"name":"helper","address":33024,"size":8,"text":true}]"#,
        )
        .unwrap();
        assert!(x.functions.contains(&0x8100));
        assert_eq!(x.promotion_generation, 3);
    }
    #[test]
    fn v1_unwind_matches_elf_and_skips_its_index() {
        let (x, mut state, mut candidate) = setup();
        let legacy = x.extract(&state, &candidate).unwrap().encode().unwrap();
        state.version = 1;
        state.unwind_regions = Some(vec![crate::manifest::UnwindRegion {
            addr: 0x4000,
            len: 28,
        }]);
        assert_eq!(
            x.extract(&state, &candidate).unwrap().encode().unwrap(),
            legacy
        );
        // Proves the manifest path does not inspect PT_GNU_EH_FRAME bytes.
        put(&mut candidate, 0x1300, &[2]);
        assert_eq!(
            x.extract(&state, &candidate).unwrap().encode().unwrap(),
            legacy
        );
        state.unwind_regions = None;
        assert!(x.extract(&state, &candidate).is_err());
        state.unwind_regions = Some(vec![crate::manifest::UnwindRegion {
            addr: 0x8000,
            len: 28,
        }]);
        assert!(x.extract(&state, &candidate).is_err());
    }
    #[test]
    fn page_deltas_entry_and_data_restores_only_commit_after_ack() {
        let (mut x, mut s, c) = setup();
        let mut loc = x.base.read(0x1200, 24).unwrap();
        loc[16..20].copy_from_slice(&11_u32.to_le_bytes());
        s.writes.insert(
            0x1200,
            Write {
                addr: 0x1200,
                kind: 2,
                payload: loc,
                fixups: vec![0],
                name: vec![],
            },
        );
        let first = x.extract(&s, &c).unwrap();
        assert_eq!(
            first.records.iter().map(|r| r.kind).collect::<Vec<_>>(),
            vec![0, 0, 1, 2, 3]
        );
        assert_eq!(first.records[3].fixups, vec![0]);
        // A failed agent send drops PreparedPatch. Retrying must send identical bytes.
        assert_eq!(
            first.encode().unwrap(),
            x.extract(&s, &c).unwrap().encode().unwrap()
        );
        x.commit(first).unwrap();
        assert!(x.extract(&s, &c).is_err());
        s.generation = 2;
        s.writes.clear();
        let second = x.extract(&s, &c).unwrap();
        assert_eq!(
            second.records.iter().map(|r| r.kind).collect::<Vec<_>>(),
            vec![1, 2, 3]
        );
        assert_eq!(second.records[0].payload, x.base.read(0x1100, 5).unwrap());
        assert_eq!(second.records[1].payload, x.base.read(0x1200, 24).unwrap());
        assert_eq!(second.records[1].fixups, vec![0]);
        x.commit(second).unwrap();
        s.generation = 3;
        let third = x.extract(&s, &c).unwrap();
        assert_eq!(third.records.len(), 1);
        assert_eq!(third.records[0].kind, 3);
        x.commit(third).unwrap();
        s.generation = 4;
        Arc::make_mut(s.pages.get_mut(&0x8000).unwrap())
            .fixups
            .push(8);
        let fourth = x.extract(&s, &c).unwrap();
        assert_eq!(
            fourth.records.iter().map(|r| r.kind).collect::<Vec<_>>(),
            vec![0, 3]
        );
    }
    #[test]
    fn invalid_candidate_and_writes_fail_closed() {
        let (x, s, mut c) = setup();
        c.headers[0].flags = 6;
        assert!(x.extract(&s, &c).is_err());
        let (x, s, mut c) = setup();
        c.dynamic.push((36, 1)); // Image::parse rejects DT_RELR before extraction.
        assert!(x.extract(&s, &c).is_err());
        let (x, mut s, c) = setup();
        s.writes.get_mut(&0x1100).unwrap().payload[0] = 0x90;
        assert!(x.extract(&s, &c).is_err());
        let (x, mut s, c) = setup();
        s.writes.get_mut(&0x1100).unwrap().payload[1..5].copy_from_slice(&1_i32.to_le_bytes());
        assert!(x.extract(&s, &c).is_err());
        let (mut x, mut s, c) = setup();
        let w = Write {
            addr: 0x1200,
            kind: 2,
            payload: vec![1; 24],
            fixups: vec![],
            name: vec![],
        };
        s.writes.insert(w.addr, w);
        let duplicate = x.objects[&0x200][0].clone();
        x.objects.get_mut(&0x200).unwrap().push(duplicate);
        assert!(x.extract(&s, &c).is_err());
    }
    #[test]
    fn prepared_patch_is_fenced() {
        let (mut x, mut s, c) = setup();
        let a = x.extract(&s, &c).unwrap();
        let b = x.extract(&s, &c).unwrap();
        x.commit(a).unwrap();
        assert!(x.commit(b).is_err());
        s.generation = 2;
        assert!(x.extract(&s, &c).is_ok());
    }

    fn gunzip(path: &Path) -> Vec<u8> {
        use std::io::Read;
        let mut bytes = Vec::new();
        flate2::read::GzDecoder::new(std::fs::File::open(path).unwrap())
            .read_to_end(&mut bytes)
            .unwrap();
        bytes
    }
    fn hex(text: &str) -> Vec<u8> {
        assert!(text.len().is_multiple_of(2));
        text.as_bytes()
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
            .collect()
    }
    /// Run with REFRESH_GOLDENS=/.../work/m2-t3/goldens cargo test ... --ignored.
    /// Goldens are external because real ELF captures are too large for source git.
    #[test]
    #[ignore = "requires captured Python web-session goldens"]
    fn python_goldens() {
        use std::time::Instant;
        let root = std::path::PathBuf::from(
            std::env::var_os("REFRESH_GOLDENS").expect("set REFRESH_GOLDENS"),
        );
        let mut dirs: Vec<_> = std::fs::read_dir(&root)
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| p.join("result.json").is_file())
            .collect();
        dirs.sort();
        assert!(
            dirs.len() >= 15,
            "capture at least 15 acknowledged live saves"
        );
        let timing = std::env::var_os("REFRESH_TIMINGS").is_some();
        let mut times = Vec::new();
        let paired = std::env::var_os("REFRESH_V1_GOLDENS").is_some();
        let mut v1_times = Vec::new();
        for dir in &dirs {
            let input: serde_json::Value =
                serde_json::from_slice(&std::fs::read(dir.join("input.json")).unwrap()).unwrap();
            let mut x = ManifestExtractor::from_bytes(
                gunzip(&root.join(input["baseline"].as_str().unwrap())),
                input["allow_tracing"].as_bool().unwrap(),
            )
            .unwrap();
            for addr in input["functions"].as_array().unwrap() {
                x.add_function(addr.as_u64().unwrap());
            }
            for a in input["previous"].as_array().unwrap() {
                x.applied.jumps.insert(a.as_u64().unwrap());
            }
            for (a, b) in input["previous_data"].as_object().unwrap() {
                x.applied
                    .data
                    .insert(a.parse().unwrap(), hex(b.as_str().unwrap()));
            }
            for (a, p) in input["applied_pages"].as_object().unwrap() {
                let addr = a.parse().unwrap();
                x.applied.pages.insert(
                    addr,
                    Arc::new(Record {
                        addr,
                        prot: p[0].as_u64().unwrap() as u32,
                        kind: p[1].as_u64().unwrap() as u32,
                        payload: hex(p[2].as_str().unwrap()),
                        fixups: p[3]
                            .as_array()
                            .unwrap()
                            .iter()
                            .map(|v| v.as_u64().unwrap() as u32)
                            .collect(),
                    }),
                );
            }
            let mut paths: Vec<_> = std::fs::read_dir(dir)
                .unwrap()
                .map(|e| e.unwrap().path())
                .filter(|p| p.to_string_lossy().ends_with(".manifest.gz"))
                .collect();
            paths.sort();
            let mut state = State::default();
            let mut v1_state = State::default();
            let last_path = paths.pop().unwrap();
            for p in paths {
                state.update(&gunzip(&p)).unwrap();
                if paired {
                    v1_state
                        .update(&gunzip(
                            &p.with_file_name(
                                p.file_name()
                                    .unwrap()
                                    .to_string_lossy()
                                    .replace(".manifest.gz", ".v1.gz"),
                            ),
                        ))
                        .unwrap();
                }
            }
            let last = gunzip(&last_path);
            let candidate_bytes = gunzip(&dir.join("candidate.elf.gz"));
            let paired_input = paired.then(|| {
                (
                    gunzip(
                        &last_path.with_file_name(
                            last_path
                                .file_name()
                                .unwrap()
                                .to_string_lossy()
                                .replace(".manifest.gz", ".v1.gz"),
                        ),
                    ),
                    candidate_bytes.clone(),
                )
            });
            let start = Instant::now();
            // Include latest manifest parsing/state update, candidate parsing,
            // extraction/unwind and wire encoding. Baseline index is session setup.
            state.update(&last).unwrap();
            let candidate = Image::parse(candidate_bytes, false).unwrap();
            let patch = x
                .extract(&state, &candidate)
                .unwrap_or_else(|e| panic!("{}: {e:#}", dir.display()));
            let wire = patch.encode().unwrap();
            let elapsed = start.elapsed().as_secs_f64() * 1000.0;
            let expected = std::fs::read(dir.join("expected.wire")).unwrap();
            if wire != expected {
                let at = wire
                    .iter()
                    .zip(&expected)
                    .position(|(a, b)| a != b)
                    .unwrap_or(wire.len().min(expected.len()));
                panic!(
                    "{}: wire differs at byte {at}; actual={} expected={}",
                    dir.display(),
                    wire.len(),
                    expected.len()
                );
            }
            if let Some((v1, candidate_bytes)) = paired_input {
                let start = Instant::now();
                v1_state.update(&v1).unwrap();
                let candidate = Image::parse(candidate_bytes, false).unwrap();
                let v1_patch = x.extract(&v1_state, &candidate).unwrap();
                let v1_wire = v1_patch.encode().unwrap();
                let v1_elapsed = start.elapsed().as_secs_f64() * 1000.0;
                assert_eq!(v1_wire, wire, "paired v1/v0 mismatch {}", dir.display());
                assert!(!v1_patch.symbols.is_empty());
                assert!(
                    v1_patch
                        .symbols
                        .iter()
                        .any(|s| !s.file.is_empty() && s.first_line > 0)
                );
                assert!(
                    v1_state
                        .writes
                        .values()
                        .filter(|w| w.kind == 1)
                        .all(|w| !w.name.is_empty())
                );
                if let Some(lines) = input["helper_lines"].as_object() {
                    for symbol in &v1_patch.symbols {
                        if let Some(line) = symbol
                            .name
                            .rsplit("::")
                            .next()
                            .and_then(|name| lines.get(name))
                        {
                            assert_eq!(
                                u64::from(symbol.first_line),
                                line.as_u64().unwrap(),
                                "helper source line for {}",
                                symbol.name
                            );
                            assert!(symbol.file.ends_with("crates/api/src/handlers.rs"));
                        }
                    }
                }
                println!(
                    "PAIRED {} v0_ms={elapsed:.6} v1_ms={v1_elapsed:.6} v0_bytes={} v1_bytes={} symbols={}",
                    dir.file_name().unwrap().to_string_lossy(),
                    last.len(),
                    v1.len(),
                    v1_patch.symbols.len()
                );
                v1_times.push(v1_elapsed);
            }
            x.commit(patch).unwrap();
            if timing {
                println!(
                    "GOLDEN {} identical bytes={} extract_ms={elapsed:.6}",
                    dir.file_name().unwrap().to_string_lossy(),
                    wire.len()
                );
                times.push(elapsed);
            }
        }
        if paired && timing {
            v1_times.sort_by(f64::total_cmp);
            println!(
                "V1_EXTRACTION n={} median_ms={:.6} min_ms={:.6} max_ms={:.6}",
                v1_times.len(),
                (v1_times[(v1_times.len() - 1) / 2] + v1_times[v1_times.len() / 2]) / 2.0,
                v1_times[0],
                v1_times[v1_times.len() - 1]
            );
        }
        if timing {
            times.sort_by(f64::total_cmp);
            println!(
                "EXTRACTION n={} median_ms={:.6} min_ms={:.6} max_ms={:.6}",
                times.len(),
                (times[(times.len() - 1) / 2] + times[times.len() / 2]) / 2.0,
                times[0],
                times[times.len() - 1]
            );
        }
        println!(
            "{} / {} Python golden wires byte-identical",
            dirs.len(),
            dirs.len()
        );
    }
}
