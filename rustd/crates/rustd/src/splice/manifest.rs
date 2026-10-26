//! EXP-090: resolved ELF manifest. The first generation carries populated pages;
//! subsequent generations carry changed pages, including relocation membership.
//! Atomic publication occurs only after the executable has been written.
use super::*;
use std::collections::{BTreeMap, BTreeSet};
#[derive(Default)]
pub(super) struct Metadata {
    unwind: Vec<(u64, u64)>,
    symbols: Vec<Symbol>,
}
struct Symbol {
    addr: u64,
    size: u64,
    name: String,
    file: String,
    line: u32,
}
fn demangle(name: &[u8]) -> String {
    format!("{:#}", rustc_demangle::demangle(&String::from_utf8_lossy(name)))
}

impl Metadata {
    // Work is bounded by this generation's patch, never the baseline unwind index.
    pub(super) fn from_patch(
        patch: &Patch<'_>,
        base: u64,
        placed: &FxHashMap<SectionIndex, u64>,
        address: &impl Fn(Target, i64, bool) -> anyhow::Result<u64>,
        appended: &[u8],
        frames: &[(u64, u64)],
    ) -> anyhow::Result<Self> {
        let word = |at: u64| -> anyhow::Result<u32> {
            let off = at.checked_sub(base).context("unwind below appended memory")? as usize;
            let raw = appended.get(off..off + 4).context("truncated unwind record")?;
            Ok(u32::from_le_bytes(raw.try_into()?))
        };
        let mut entries = BTreeSet::new();
        let mut cies = BTreeSet::new();
        for &(_, fde) in frames {
            let back = word(fde + 4)?;
            ensure!(back > 0, "invalid FDE back pointer");
            let cie = (fde + 4).checked_sub(back as u64).context("CIE underflow")?;
            ensure!(word(cie + 4)? == 0, "FDE missing CIE");
            entries.extend([cie, fde]);
            cies.insert(cie);
        }
        let mut unwind = Vec::new();
        for &start in &entries {
            if unwind.last().is_some_and(|&(a, n)| start < a + n) {
                continue;
            }
            ensure!(cies.contains(&start), "unwind group starts without CIE");
            let mut end = start;
            while entries.contains(&end) {
                let len = word(end)?;
                ensure!((4..u32::MAX).contains(&len), "invalid unwind length");
                end = end.checked_add(4 + len as u64).context("unwind overflow")?;
                ensure!(end <= base + appended.len() as u64, "unwind past patch");
            }
            unwind.push((start, end - start));
        }
        let mut symbols = Vec::new();
        for symbol in patch.file.symbols() {
            if !symbol.is_definition() || symbol.kind() != SymbolKind::Text || symbol.size() == 0 {
                continue;
            }
            let Some(section) = symbol.section_index().filter(|s| placed.contains_key(s)) else {
                continue;
            };
            symbols.push(Symbol {
                addr: base + placed[&section] + symbol.address(),
                size: symbol.size(),
                name: demangle(symbol.name_bytes()?),
                file: String::new(),
                line: 0,
            });
        }
        symbols.sort_by(|a, b| (a.addr, &a.name).cmp(&(b.addr, &b.name)));
        // Decode only the patch's relocated line tables. No full debug-info scan of the ELF.
        let dwarf = gimli::DwarfSections::load(|id| -> anyhow::Result<Vec<u8>> {
            let Some(section) = patch.file.section_by_name(id.name()) else {
                return Ok(Vec::new());
            };
            let mut data = section.data()?.to_vec();
            for (at, relocation) in section.relocations() {
                let RelocationFlags::Elf { r_type } = relocation.flags() else {
                    bail!("non-ELF debug relocation")
                };
                let RelocationTarget::Symbol(index) = relocation.target() else {
                    bail!("non-symbol debug relocation")
                };
                let symbol = patch.file.symbol_by_index(index)?;
                let value = if symbol.section_index().is_some_and(|s| {
                    patch
                        .file
                        .section_by_index(s)
                        .is_ok_and(|s| s.name_bytes().is_ok_and(|n| n.starts_with(b".debug_")))
                }) {
                    symbol.address().wrapping_add_signed(relocation.addend())
                } else {
                    address(
                        *patch.targets.get(&index).context("missing debug target")?,
                        relocation.addend(),
                        true,
                    )?
                };
                match r_type {
                    elf::R_X86_64_64 => {
                        data[at as usize..][..8].copy_from_slice(&value.to_le_bytes())
                    }
                    elf::R_X86_64_32 => data[at as usize..][..4]
                        .copy_from_slice(&u32::try_from(value)?.to_le_bytes()),
                    _ => bail!("unsupported manifest debug relocation {r_type}"),
                }
            }
            Ok(data)
        })?;
        let dwarf = dwarf.borrow(|section| gimli::EndianSlice::new(section, gimli::LittleEndian));
        let mut units = dwarf.units();
        let mut locations = BTreeMap::new();
        while let Some(header) = units.next()? {
            let unit = dwarf.unit(header)?;
            let Some(program) = unit.line_program.clone() else { continue };
            let mut rows = program.rows();
            while let Some((header, row)) = rows.next_row()? {
                if row.end_sequence() || row.line().is_none() {
                    continue;
                }
                let Some(file) = row.file(header) else { continue };
                let path =
                    dwarf.attr_string(&unit, file.path_name())?.to_string_lossy().into_owned();
                let directory =
                    file.directory(header).map(|v| dwarf.attr_string(&unit, v)).transpose()?;
                let directory = directory.as_ref().map(|v| v.to_string_lossy().into_owned());
                let path = match directory {
                    Some(dir) if !Path::new(&path).is_absolute() => {
                        Path::new(&dir).join(path).to_string_lossy().into_owned()
                    }
                    _ => path,
                };
                locations.entry(row.address()).or_insert((path, row.line().unwrap().get() as u32));
            }
        }
        for symbol in &mut symbols {
            if let Some((_, (file, line))) =
                locations.range(symbol.addr..symbol.addr + symbol.size).next()
            {
                symbol.file = file.clone();
                symbol.line = *line;
            }
        }
        Ok(Self { unwind, symbols })
    }
}

fn crc32(bytes: &[u8]) -> u32 { crc32fast::hash(bytes) }

#[derive(Default)]
pub(super) struct State {
    generation: u64,
    roots: BTreeSet<String>,
    retained: BTreeSet<u64>,
    base_relatives: Option<BTreeMap<u64, u64>>,
    pages: BTreeMap<u64, (u32, Vec<u8>, Vec<u32>)>,
}
pub(super) struct Prepared {
    wire: Vec<u8>,
    legacy: Option<Vec<u8>>,
    roots: BTreeSet<String>,
    retained: BTreeSet<u64>,
    pages: BTreeMap<u64, (u32, Vec<u8>, Vec<u32>)>,
}
fn u32out(out: &mut Vec<u8>, n: u32) {
    out.extend_from_slice(&n.to_le_bytes());
}
fn u64out(out: &mut Vec<u8>, n: u64) {
    out.extend_from_slice(&n.to_le_bytes());
}
fn blob(out: &mut Vec<u8>, bytes: &[u8]) {
    u32out(out, bytes.len() as u32);
    out.extend_from_slice(bytes);
}

impl State {
    pub(super) fn prepare(
        &mut self,
        image: &Image,
        addition: &Addition,
        undo: &[(u64, Vec<u8>)],
    ) -> anyhow::Result<Prepared> {
        let started = std::time::Instant::now();
        let legacy_only = std::env::var("RUSTD_MANIFEST").as_deref() == Ok("v0");
        let dual = std::env::var_os("RUSTD_MANIFEST_V0_SIDECAR").is_some_and(|v| v == "1");
        let e = Endianness::Little;
        ensure!(matches!(image.machine, Machine::X86_64), "manifest requires x86-64");
        let mut bytes = addition.appended.clone();
        let (headers, _) = object::slice_from_bytes::<elf::ProgramHeader64<Endianness>>(
            &addition.headers,
            addition.headers.len() / size_of::<elf::ProgramHeader64<Endianness>>(),
        )
        .map_err(|()| anyhow::anyhow!("unaligned manifest headers"))?;
        let appended: Vec<_> = headers
            .iter()
            .filter(|p| {
                p.p_type(e) == elf::PT_LOAD
                    && p.p_offset(e) >= addition.offset
                    && p.p_offset(e) < addition.offset + bytes.len() as u64
            })
            .collect();
        let base = appended.first().context("no appended manifest segments")?.p_vaddr(e);
        let original_ranges: Vec<_> = addition
            .overwritten
            .iter()
            .filter_map(|(offset, raw)| {
                image
                    .segments_of(elf::PT_LOAD)
                    .find(|p| {
                        *offset >= p.p_offset(e)
                            && *offset + raw.len() as u64 <= p.p_offset(e) + p.p_filesz(e)
                    })
                    .map(|p| {
                        let addr = p.p_vaddr(e) + offset - p.p_offset(e);
                        (addr, addr + raw.len() as u64)
                    })
            })
            .collect();
        // Loader-only global unwind index and relocation tables are required in
        // the standalone candidate ELF, but the live agent never installs the
        // moved PHDR/dynamic pointers. Keep only pages used by actual sections.
        let mut loader_end = headers
            .iter()
            .filter(|p| p.p_type(e) == elf::PT_GNU_EH_FRAME)
            .map(|p| p.p_vaddr(e) + p.p_memsz(e))
            .max()
            .unwrap_or(base);
        let mut relatives = BTreeMap::new();
        if let Some(rel) = &image.relocations {
            let read_word = |at| {
                addition
                    .overwritten
                    .iter()
                    .find(|(o, b)| *o == at && b.len() == 8)
                    .map(|(_, b)| u64::from_le_bytes(b[..8].try_into().unwrap()))
            };
            if let (Some(address), Some(len)) = (read_word(rel.address_at), read_word(rel.size_at))
            {
                loader_end = loader_end.max(address + len);
                let start = (address - base) as usize;
                for entry in bytes[start..start + len as usize].chunks_exact(24) {
                    let at = u64::from_le_bytes(entry[..8].try_into().unwrap());
                    let info = u64::from_le_bytes(entry[8..16].try_into().unwrap());
                    if info == 8 {
                        if at >= base || original_ranges.iter().any(|&(lo, hi)| lo <= at && at < hi)
                        {
                            relatives
                                .insert(at, u64::from_le_bytes(entry[16..24].try_into().unwrap()));
                        }
                    } else {
                        ensure!(
                            at < base || at >= base + bytes.len() as u64,
                            "unsupported appended relocation"
                        );
                    }
                }
            }
        }
        for (&at, &value) in relatives.range(base..base + bytes.len() as u64) {
            bytes[(at - base) as usize..][..8].copy_from_slice(&value.to_le_bytes());
        }
        let mut segments = Vec::new();
        let mut pages = BTreeMap::new();
        for p in appended {
            let mut addr = p.p_vaddr(e);
            let mut len = p.p_memsz(e);
            if p.p_flags(e) == elf::PF_R && addr == base {
                let align = 8;
                let cut = loader_end.next_multiple_of(align) & !4095;
                ensure!(cut >= addr && cut <= addr + len, "live table cut outside read segment");
                len -= cut - addr;
                addr = cut;
            }
            if len == 0 {
                continue;
            }
            let flags = p.p_flags(e);
            let prot = ((flags & 4) >> 2) | (flags & 2) | ((flags & 1) << 2);
            segments.push((addr, len, prot));
            for off in (0..len).step_by(4096) {
                let n = (len - off).min(4096) as usize;
                let at = addr + off;
                let mut payload = vec![0; 4096];
                payload[..n].copy_from_slice(&bytes[(at - base) as usize..][..n]);
                let fix: Vec<u32> =
                    relatives.range(at..at + n as u64).map(|(&v, _)| (v - at) as u32).collect();
                ensure!(fix.iter().all(|v| *v as usize + 8 <= n), "pointer crosses manifest page");
                // Zero pages still need a mapping; omitting their bytes saves reservation IO.
                pages.insert(at, (prot, payload, fix));
            }
        }
        if (legacy_only || dual) && self.base_relatives.is_none() {
            let mut baseline = BTreeMap::new();
            if let Some(rel) = &image.relocations {
                for r in rel.table.chunks_exact(24) {
                    if u64::from_le_bytes(r[8..16].try_into().unwrap()) == 8 {
                        baseline.insert(
                            u64::from_le_bytes(r[..8].try_into().unwrap()),
                            u64::from_le_bytes(r[16..24].try_into().unwrap()),
                        );
                    }
                }
            }
            self.base_relatives = Some(baseline);
        }
        let mut writes = Vec::new();

        for (offset, raw) in &addition.overwritten {
            if *offset < 64
                || image
                    .segments_of(elf::PT_DYNAMIC)
                    .any(|p| *offset >= p.p_offset(e) && *offset < p.p_offset(e) + p.p_filesz(e))
            {
                continue;
            }
            let Some(p) = image.segments_of(elf::PT_LOAD).find(|p| {
                *offset >= p.p_offset(e)
                    && *offset + raw.len() as u64 <= p.p_offset(e) + p.p_filesz(e)
            }) else {
                continue;
            };
            let addr = p.p_vaddr(e) + offset - p.p_offset(e);
            let kind = if p.p_flags(e) & elf::PF_X != 0 { 1 } else { 2 };
            let mut payload = raw.clone();
            let mut fix = Vec::new();
            if kind == 1 {
                ensure!(payload.len() == 5 && payload[0] == 0xe9, "non-entry text write");
            }
            for (&at, &value) in relatives.range(addr..addr + payload.len() as u64) {
                let within = (at - addr) as usize;
                ensure!(within + 8 <= payload.len(), "partial data relocation");
                payload[within..within + 8].copy_from_slice(&value.to_le_bytes());
                fix.push(within as u32);
            }
            // Carry canonical symbol identity; controller enforces the existing data policy.
            let target = if kind == 1 {
                addr.wrapping_add(5)
                    .wrapping_add_signed(i32::from_le_bytes(payload[1..5].try_into()?) as i64)
            } else {
                addr
            };
            let name = if kind == 1 {
                addition
                    .metadata
                    .symbols
                    .iter()
                    .find(|s| s.addr == target)
                    .map(|s| s.name.clone())
                    .unwrap_or_default()
            } else {
                addition
                    .catalog
                    .iter()
                    .find(|(_, s)| s.address == target)
                    .map(|(n, _)| demangle(n))
                    .unwrap_or_default()
            };
            writes.push((addr, kind, payload, fix, name.into_bytes()));
        }
        let mut wire = b"RDM1".to_vec();
        wire.extend_from_slice(&1u16.to_le_bytes());
        wire.extend_from_slice(&0u16.to_le_bytes());
        u64out(&mut wire, self.generation + 1);
        u32out(&mut wire, segments.len() as u32);
        for &(addr, len, prot) in &segments {
            u64out(&mut wire, addr);
            u64out(&mut wire, len);
            u32out(&mut wire, prot);
        }
        let changes: Vec<_> =
            pages.iter().filter(|(at, p)| self.pages.get(at) != Some(p)).collect();
        u32out(&mut wire, changes.len() as u32);
        for (&addr, (prot, payload, fix)) in changes {
            u64out(&mut wire, addr);
            u32out(&mut wire, *prot);
            blob(&mut wire, if payload.iter().all(|b| *b == 0) { &[] } else { payload });
            u32out(&mut wire, fix.len() as u32);
            for off in fix {
                u32out(&mut wire, *off);
            }
        }
        u32out(&mut wire, writes.len() as u32);
        for (addr, kind, payload, fix, name) in writes {
            u64out(&mut wire, addr);
            u32out(&mut wire, kind);
            blob(&mut wire, &payload);
            u32out(&mut wire, fix.len() as u32);
            for off in fix {
                u32out(&mut wire, off);
            }
            blob(&mut wire, &name);
        }
        let legacy_prefix = wire.len();
        let mut legacy = if legacy_only || dual {
            let mut old = b"RDM09001".to_vec();
            old.extend_from_slice(&wire[8..legacy_prefix]);
            Some(old)
        } else {
            None
        };
        // Restore records are retained only in the one-release v0 compatibility format.
        let mut restores = Vec::new();
        for (offset, raw) in undo.iter().filter(|_| legacy.is_some()) {
            if *offset < 64
                || image
                    .segments_of(elf::PT_DYNAMIC)
                    .any(|p| *offset >= p.p_offset(e) && *offset < p.p_offset(e) + p.p_filesz(e))
            {
                continue;
            }
            let Some(p) = image.segments_of(elf::PT_LOAD).find(|p| {
                *offset >= p.p_offset(e)
                    && *offset + raw.len() as u64 <= p.p_offset(e) + p.p_filesz(e)
            }) else {
                continue;
            };
            let addr = p.p_vaddr(e) + offset - p.p_offset(e);
            let kind = if p.p_flags(e) & elf::PF_X != 0 { 1 } else { 2 };
            let mut payload = raw.clone();
            let mut fix = Vec::new();
            for (&at, &value) in
                self.base_relatives.as_ref().unwrap().range(addr..addr + raw.len() as u64)
            {
                let within = (at - addr) as usize;
                ensure!(within + 8 <= payload.len(), "partial restore pointer");
                payload[within..within + 8].copy_from_slice(&value.to_le_bytes());
                fix.push(within as u32);
            }
            restores.push((addr, kind, payload, fix));
        }
        if let Some(old) = &mut legacy {
            u32out(old, restores.len() as u32);
            for (addr, kind, payload, fix) in restores {
                u64out(old, addr);
                u32out(old, kind);
                blob(old, &payload);
                u32out(old, fix.len() as u32);
                for off in fix {
                    u32out(old, off);
                }
            }
        }
        u32out(&mut wire, addition.metadata.unwind.len() as u32);
        for &(addr, len) in &addition.metadata.unwind {
            u64out(&mut wire, addr);
            u64out(&mut wire, len);
        }
        u32out(&mut wire, addition.metadata.symbols.len() as u32);
        let mut roots = BTreeSet::new();
        for symbol in &addition.metadata.symbols {
            u64out(&mut wire, symbol.addr);
            u64out(&mut wire, symbol.size);
            blob(&mut wire, symbol.name.as_bytes());
            blob(&mut wire, symbol.file.as_bytes());
            u32out(&mut wire, symbol.line);
            u32out(&mut wire, 0); // optional compact line table (absent)
            if !self.roots.contains(&symbol.name) {
                roots.insert(symbol.name.clone());
            }
        }
        let retained: BTreeSet<_> =
            pages.keys().filter(|p| !self.retained.contains(p)).copied().collect();
        let prepare_us = started.elapsed().as_micros() as u64;
        for out in std::iter::once(&mut wire).chain(legacy.iter_mut()) {
            u64out(out, prepare_us);
            u64out(out, bytes.len() as u64);
            u64out(out, relatives.len() as u64);
        }
        u64out(&mut wire, (self.roots.len() + roots.len()) as u64);
        u64out(&mut wire, (self.retained.len() + retained.len()) as u64 * 4096);
        if legacy_only {
            wire = legacy.take().unwrap();
        }
        Ok(Prepared { wire, legacy, pages, roots, retained })
    }
    pub(super) fn commit(&mut self, prepared: Prepared, binary: &Path) -> anyhow::Result<()> {
        let path = binary.with_file_name("patch.manifest");
        let temp = path.with_extension("manifest.tmp");
        let finish = |mut wire: Vec<u8>| -> anyhow::Result<Vec<u8>> {
            #[cfg(feature = "async-certificates")]
            {
                if wire.starts_with(b"RDM1") {
                    let certificates = match std::fs::read(binary.with_file_name("async-certificates.bin")) {
                        Ok(bytes) => bytes,
                        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
                        Err(e) => return Err(e.into()),
                    };
                    if !certificates.is_empty() {
                        wire[6..8].copy_from_slice(&1u16.to_le_bytes());
                        wire.extend_from_slice(&certificates);
                    }
                }
            }
            if wire.starts_with(b"RDM1") {
                let crc = crc32(&wire);
                u32out(&mut wire, crc);
            }
            Ok(wire)
        };
        // Sidecar is published first. A failure never publishes a new primary generation.
        if let Some(legacy) = prepared.legacy {
            let legacy_path = binary.with_file_name("patch.manifest.v0");
            let legacy_temp = legacy_path.with_extension("v0.tmp");
            std::fs::write(&legacy_temp, finish(legacy)?)?;
            std::fs::rename(legacy_temp, legacy_path)?;
        }
        std::fs::write(&temp, finish(prepared.wire)?)?;
        std::fs::rename(temp, path)?;
        self.roots.extend(prepared.roots);
        self.retained.extend(prepared.retained);
        self.pages = prepared.pages;
        self.generation += 1;
        Ok(())
    }
}
