//! ELF64 x86-64 indexing and RELATIVE normalization. No section scan per save.
use anyhow::{Context, Result, bail, ensure};
use object::read::elf::{FileHeader, ProgramHeader};
use object::{LittleEndian as LE, Object, ObjectSection, ObjectSymbol, SectionFlags};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Program {
    pub typ: u32,
    pub flags: u32,
    pub offset: u64,
    pub addr: u64,
    pub paddr: u64,
    pub filesz: u64,
    pub memsz: u64,
    pub align: u64,
}
impl Program {
    pub fn contains(&self, addr: u64, n: u64, file: bool) -> bool {
        let len = if file { self.filesz } else { self.memsz };
        addr >= self.addr
            && addr
                .checked_add(n)
                .zip(self.addr.checked_add(len))
                .is_some_and(|(a, b)| a <= b)
    }
    pub fn identity(&self) -> (u64, u64, u32) {
        (self.addr, self.memsz, self.flags)
    }
}
#[derive(Clone, Debug)]
pub struct Section {
    pub name: String,
    pub flags: u64,
}
#[derive(Clone, Debug)]
pub struct Symbol {
    pub name: String,
    pub addr: u64,
    pub size: u64,
    pub typ: u8,
    pub section: usize,
    pub offset: u64,
}
pub type SymbolIndex = (
    BTreeSet<u64>,
    BTreeMap<u64, Vec<Symbol>>,
    BTreeMap<usize, Section>,
);

#[derive(Debug)]
pub struct Image {
    pub data: Vec<u8>,
    pub typ: u16,
    pub phoff: u64,
    pub headers: Vec<Program>,
    pub dynamic: Vec<(u64, u64)>,
    pub relatives: BTreeMap<u64, u64>,
    pub relas: BTreeMap<u64, (u64, u64)>,
}
pub fn slice(data: &[u8], off: u64, n: u64) -> Result<&[u8]> {
    let end = off.checked_add(n).context("ELF offset overflow")?;
    data.get(usize::try_from(off)?..usize::try_from(end)?)
        .context("ELF range outside file")
}
pub fn u32_at(data: &[u8], off: usize) -> Result<u32> {
    Ok(u32::from_le_bytes(
        data.get(off..off.checked_add(4).context("offset overflow")?)
            .context("truncated u32")?
            .try_into()?,
    ))
}
pub fn u64_at(data: &[u8], off: usize) -> Result<u64> {
    Ok(u64::from_le_bytes(
        data.get(off..off.checked_add(8).context("offset overflow")?)
            .context("truncated u64")?
            .try_into()?,
    ))
}
impl Image {
    pub fn parse(data: Vec<u8>, normalize: bool) -> Result<Self> {
        ensure!(
            data.get(..6) == Some(b"\x7fELF\x02\x01"),
            "requires ELF64 little endian"
        );
        let h = object::elf::FileHeader64::<LE>::parse(data.as_slice())?;
        let typ = h.e_type(LE);
        ensure!(
            matches!(typ, 2 | 3) && h.e_machine(LE) == 62,
            "requires x86-64 ET_EXEC/ET_DYN"
        );
        let phoff = h.e_phoff(LE);
        let headers = h
            .program_headers(LE, data.as_slice())?
            .iter()
            .map(|p| Program {
                typ: p.p_type(LE),
                flags: p.p_flags(LE),
                offset: p.p_offset(LE),
                addr: p.p_vaddr(LE),
                paddr: p.p_paddr(LE),
                filesz: p.p_filesz(LE),
                memsz: p.p_memsz(LE),
                align: p.p_align(LE),
            })
            .collect::<Vec<_>>();
        for p in &headers {
            ensure!(
                p.addr.checked_add(p.memsz).is_some(),
                "ELF segment address overflow"
            );
            slice(&data, p.offset, p.filesz)?;
            if p.typ == 1 {
                ensure!(p.filesz <= p.memsz, "LOAD filesz exceeds memsz");
            }
        }
        let mut dynamic = Vec::new();
        if let Some(p) = headers.iter().find(|p| p.typ == 2) {
            ensure!(p.filesz % 16 == 0, "invalid dynamic size");
            for b in slice(&data, p.offset, p.filesz)?.as_chunks::<16>().0 {
                let tag = u64_at(b, 0)?;
                if tag == 0 {
                    break;
                }
                dynamic.push((tag, u64_at(b, 8)?));
            }
        }
        let mut image = Self {
            data,
            typ,
            phoff,
            headers,
            dynamic,
            relatives: BTreeMap::new(),
            relas: BTreeMap::new(),
        };
        ensure!(
            !image.dynamic.iter().any(|(k, _)| *k == 36),
            "DT_RELR unsupported"
        );
        if normalize {
            image.read_relocations(true)?;
        }
        Ok(image)
    }
    pub fn loads(&self) -> impl Iterator<Item = &Program> {
        self.headers.iter().filter(|p| p.typ == 1)
    }
    pub fn offset(&self, addr: u64, n: u64) -> Result<u64> {
        let p = self
            .loads()
            .find(|p| p.contains(addr, n, true))
            .context("not file-backed LOAD memory")?;
        p.offset
            .checked_add(addr - p.addr)
            .context("ELF file offset overflow")
    }
    pub fn read(&self, addr: u64, n: usize) -> Result<Vec<u8>> {
        Ok(slice(&self.data, self.offset(addr, n as u64)?, n as u64)?.to_vec())
    }
    pub fn relocation_table(&self) -> Result<Option<&[u8]>> {
        let tags: BTreeMap<_, _> = self.dynamic.iter().copied().collect();
        let Some(&addr) = tags.get(&7) else {
            return Ok(None);
        };
        let len = *tags.get(&8).context("missing DT_RELASZ")?;
        ensure!(
            tags.get(&9).copied().unwrap_or(24) == 24 && len % 24 == 0,
            "invalid DT_RELA"
        );
        Ok(Some(slice(&self.data, self.offset(addr, len)?, len)?))
    }
    fn read_relocations(&mut self, normalize: bool) -> Result<()> {
        let Some(table) = self.relocation_table()? else {
            return Ok(());
        };
        let mut relas = BTreeMap::new();
        let mut relatives = BTreeMap::new();
        for b in table.as_chunks::<24>().0 {
            let at = u64_at(b, 0)?;
            let info = u64_at(b, 8)?;
            let value = u64_at(b, 16)?;
            if let Some(old) = relas.insert(at, (info, value)) {
                ensure!(old == (info, value), "conflicting dynamic relocation");
            }
            if info == 8 {
                relatives.insert(at, value);
            }
        }
        if normalize {
            for (&addr, &value) in &relatives {
                let off = self.offset(addr, 8)? as usize;
                self.data[off..off + 8].copy_from_slice(&value.to_le_bytes());
            }
        }
        self.relas = relas;
        self.relatives = relatives;
        Ok(())
    }
    pub fn index(&self) -> Result<SymbolIndex> {
        let file = object::read::elf::ElfFile64::<LE>::parse(self.data.as_slice())?;
        let mut functions = BTreeSet::new();
        let mut objects: BTreeMap<u64, Vec<Symbol>> = BTreeMap::new();
        let mut sections = BTreeMap::new();
        for s in file.sections() {
            let SectionFlags::Elf { sh_flags } = s.flags() else {
                bail!("non-ELF section")
            };
            sections.insert(
                s.index().0,
                Section {
                    name: s.name()?.to_owned(),
                    flags: sh_flags,
                },
            );
        }
        for s in file.symbols() {
            let addr = s.address();
            let size = s.size();
            if addr == 0 || size == 0 {
                continue;
            }
            let typ = s.elf_symbol().st_type();
            if typ == object::elf::STT_FUNC && size >= 5 {
                functions.insert(addr);
            }
            let Some(index) = s.section_index() else {
                continue;
            };
            let sec = file.section_by_index(index)?;
            let Some((offset, _)) = sec.file_range() else {
                continue;
            };
            if addr < sec.address() {
                continue;
            }
            let offset = offset
                .checked_add(addr - sec.address())
                .context("symbol offset overflow")?;
            ensure!(offset.checked_add(size).is_some(), "symbol range overflow");
            objects.entry(offset).or_default().push(Symbol {
                name: String::from_utf8_lossy(s.name_bytes()?).into_owned(),
                addr,
                size,
                typ,
                section: index.0,
                offset,
            });
        }
        Ok((functions, objects, sections))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_images::elf_bytes;
    #[test]
    fn normalizes_only_relative_and_rejects_relr() {
        let b = elf_bytes();
        let e = Image::parse(b.clone(), true).unwrap();
        assert_eq!(u64_at(&e.data, 0x100).unwrap(), 0x123456);
        assert_eq!(e.relatives[&0x1100], 0x123456);
        assert!(e.index().is_ok());
        let mut b = b;
        b[0x408..0x410].copy_from_slice(&1_u64.to_le_bytes());
        let e = Image::parse(b, true).unwrap();
        assert_eq!(u64_at(&e.data, 0x100).unwrap(), 0);
        assert!(e.relatives.is_empty());
        let mut b = elf_bytes();
        b[0x300..0x308].copy_from_slice(&36_u64.to_le_bytes());
        assert!(Image::parse(b, false).is_err());
    }
    #[test]
    fn relocation_outside_backing_and_conflicts_fail() {
        let mut b = elf_bytes();
        b[0x400..0x408].copy_from_slice(&0x3000_u64.to_le_bytes());
        assert!(Image::parse(b, true).is_err());
        let mut b = elf_bytes();
        b[0x318..0x320].copy_from_slice(&48_u64.to_le_bytes());
        let mut r = b[0x400..0x418].to_vec();
        r[16..24].copy_from_slice(&9_u64.to_le_bytes());
        b[0x418..0x430].copy_from_slice(&r);
        assert!(Image::parse(b, true).is_err());
    }
    #[test]
    fn truncated_and_header_garbage_never_panics() {
        let b = elf_bytes();
        for n in 0..b.len() {
            let _ = Image::parse(b[..n].to_vec(), true);
        }
        let mut seed = 29_u64;
        for _ in 0..1024 {
            let mut b = elf_bytes();
            for _ in 0..5 {
                seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
                let at = (seed >> 32) as usize % 200;
                b[at] = (seed >> 24) as u8;
            }
            let _ = Image::parse(b, true);
        }
    }
}
