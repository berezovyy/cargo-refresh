//! ELF64 x86-64 indexing and RELATIVE normalization. No section scan per save.
use anyhow::{Context, Result, bail, ensure};
use object::read::elf::{FileHeader, ProgramHeader};
use object::{LittleEndian as LE, Object, ObjectSection, ObjectSymbol, SectionFlags};
use std::collections::{BTreeMap, BTreeSet};
use std::ops::{Deref, DerefMut};

#[derive(Debug)]
pub enum ImageBytes {
    Owned(Vec<u8>),
    Mapped(memmap2::MmapMut),
}
impl From<Vec<u8>> for ImageBytes {
    fn from(bytes: Vec<u8>) -> Self {
        Self::Owned(bytes)
    }
}
impl ImageBytes {
    fn as_slice(&self) -> &[u8] {
        self
    }
}
impl Deref for ImageBytes {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        match self {
            Self::Owned(bytes) => bytes,
            Self::Mapped(bytes) => bytes,
        }
    }
}
impl DerefMut for ImageBytes {
    fn deref_mut(&mut self) -> &mut [u8] {
        match self {
            Self::Owned(bytes) => bytes,
            Self::Mapped(bytes) => bytes,
        }
    }
}

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
    pub data: ImageBytes,
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

/// Loaded pages, excluding sparse holes such as M3's 256-MiB code-region gap.
/// Read only the ELF header and program table, even for a large sparse candidate.
pub fn loaded_bytes(path: &std::path::Path) -> Result<u64> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = std::fs::File::open(path)?;
    let len = file.metadata()?.len();
    let mut header = [0; 64];
    file.read_exact(&mut header)?;
    ensure!(
        &header[..6] == b"\x7fELF\x02\x01",
        "requires ELF64 little endian"
    );
    ensure!(
        matches!(u16::from_le_bytes(header[16..18].try_into()?), 2 | 3)
            && u16::from_le_bytes(header[18..20].try_into()?) == 62,
        "requires x86-64 ET_EXEC/ET_DYN"
    );
    let offset = u64_at(&header, 32)?;
    let stride = u16::from_le_bytes(header[54..56].try_into()?);
    let count = u16::from_le_bytes(header[56..58].try_into()?);
    ensure!(
        stride == 56 && count > 0 && count != 0xffff,
        "invalid ELF program table"
    );
    let size = u64::from(count) * u64::from(stride);
    ensure!(
        offset.checked_add(size).is_some_and(|end| end <= len),
        "ELF program table outside file"
    );
    file.seek(SeekFrom::Start(offset))?;
    let mut table = vec![0; size as usize];
    file.read_exact(&mut table)?;
    let mut total = 0_u64;
    for p in table.as_chunks::<56>().0 {
        if u32_at(p, 0)? != 1 {
            continue;
        }
        let addr = u64_at(p, 16)?;
        let filesz = u64_at(p, 32)?;
        let memsz = u64_at(p, 40)?;
        ensure!(
            filesz <= memsz && addr.checked_add(memsz).is_some(),
            "invalid ELF load size"
        );
        ensure!(
            u64_at(p, 8)?
                .checked_add(filesz)
                .is_some_and(|end| end <= len),
            "ELF load outside file"
        );
        if memsz == 0 {
            continue;
        }
        let pages = memsz
            .checked_add(addr % 4096)
            .and_then(|n| n.checked_add(4095))
            .context("ELF load size overflow")?
            & !4095;
        total = total
            .checked_add(pages)
            .context("ELF loaded size overflow")?;
    }
    ensure!(total > 0, "ELF has no loaded pages");
    Ok(total)
}
impl Image {
    pub fn parse(data: Vec<u8>, normalize: bool) -> Result<Self> {
        Self::parse_bytes(data.into(), normalize)
    }
    /// Map a completed worker candidate without copying untouched program bytes.
    ///
    /// # Safety
    /// The file must not be changed or truncated until this Image is dropped.
    pub unsafe fn map_candidate(path: &std::path::Path) -> Result<Self> {
        let file = std::fs::File::open(path)?;
        // A private mapping also prevents accidental normalization from writing
        // the compiler artifact. Only accessed metadata pages are faulted in.
        let bytes = unsafe { memmap2::MmapOptions::new().map_copy(&file)? };
        Self::parse_bytes(ImageBytes::Mapped(bytes), false)
    }
    /// Map and normalize a promotion baseline privately, without copying sparse holes.
    ///
    /// # Safety
    /// The file must never be changed or truncated while this Image exists.
    pub unsafe fn map_baseline(path: &std::path::Path) -> Result<Self> {
        let mut image = unsafe { Self::map_candidate(path)? };
        image.read_relocations(true)?;
        Ok(image)
    }
    fn parse_bytes(data: ImageBytes, normalize: bool) -> Result<Self> {
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
    fn image_budget_excludes_sparse_region_gap_and_rejects_invalid_loads() {
        use std::io::{Seek, SeekFrom, Write};
        let mut file = tempfile::NamedTempFile::new().unwrap();
        file.write_all(&elf_bytes()).unwrap();
        file.as_file().set_len(300 * 1024 * 1024).unwrap();
        assert!(file.as_file().metadata().unwrap().len() > 256 * 1024 * 1024);
        assert_eq!(loaded_bytes(file.path()).unwrap(), 4096);
        // A malformed table cannot bypass the budget through a tiny memsz.
        file.seek(SeekFrom::Start(64 + 32)).unwrap();
        file.write_all(&8192_u64.to_le_bytes()).unwrap();
        assert!(loaded_bytes(file.path()).is_err());
    }
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
    fn mapped_candidate_matches_owned_validation_and_is_private() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("candidate");
        let bytes = elf_bytes();
        std::fs::write(&file, &bytes).unwrap();
        let owned = Image::parse(bytes.clone(), false).unwrap();
        // SAFETY: this test owns the file, which stays unchanged until drop.
        let mut mapped = unsafe { Image::map_candidate(&file) }.unwrap();
        assert_eq!(mapped.headers, owned.headers);
        assert_eq!(mapped.dynamic, owned.dynamic);
        assert_eq!(
            mapped.relocation_table().unwrap(),
            owned.relocation_table().unwrap()
        );
        assert_eq!(&*mapped.data, bytes);
        mapped.data[0x100] = 42;
        assert_eq!(std::fs::read(&file).unwrap(), bytes);
        drop(mapped);
        for bytes in [vec![], vec![0; 64], b"not an ELF".to_vec()] {
            std::fs::write(&file, &bytes).unwrap();
            assert!(unsafe { Image::map_candidate(&file) }.is_err());
        }
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
