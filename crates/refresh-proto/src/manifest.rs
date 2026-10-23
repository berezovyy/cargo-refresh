//! Versioned RDM1 and legacy RDM09001 compiler manifests.
use crate::{Error, Reader, Result};
pub const V1_MAGIC: &[u8; 4] = b"RDM1";
pub const VERSION: u16 = 1;
pub const FLAG_CERTIFICATES: u16 = 1;
pub const EPOCH_ROOT_LIMIT: u64 = 64;
pub const EPOCH_RETAINED_LIMIT: u64 = 32 * 1024 * 1024;
/// The epoch policy is derived from the v1 footer, without another wire field.
pub fn epoch_due(cumulative_roots: u64, retained_bytes: u64) -> bool {
    cumulative_roots > EPOCH_ROOT_LIMIT || retained_bytes >= EPOCH_RETAINED_LIMIT
}
pub const MAGIC: &[u8; 8] = b"RDM09001";
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Segment {
    pub addr: u64,
    pub len: u64,
    pub prot: u32,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Page {
    pub addr: u64,
    pub prot: u32,
    pub payload: Vec<u8>,
    pub fixups: Vec<u32>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Write {
    pub addr: u64,
    pub kind: u32,
    pub payload: Vec<u8>,
    pub fixups: Vec<u32>,
    pub name: Vec<u8>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnwindRegion {
    /// ELF virtual address; add the runtime load bias for a process address.
    pub addr: u64,
    pub len: u64,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Symbol {
    pub addr: u64,
    pub size: u64,
    pub name: String,
    /// Empty with line zero when compiler debug information is disabled/absent.
    pub file: String,
    pub first_line: u32,
    /// Optional generation-local compact line-table bytes; empty in version 1 writers.
    pub line_table: Vec<u8>,
}
#[derive(Clone, Debug)]
pub struct Manifest {
    pub version: u16,
    pub flags: u16,
    /// None in v0; Some(empty) in v1 explicitly means no unwind records.
    pub unwind_regions: Option<Vec<UnwindRegion>>,
    pub symbols: Vec<Symbol>,
    pub cumulative_roots: u64,
    pub retained_bytes: u64,
    /// Derived from the two epoch counters in the footer; false for v0.
    pub epoch_due: bool,
    pub generation: u64,
    pub segments: Vec<Segment>,
    pub pages: Vec<Page>,
    pub writes: Vec<Write>,
    pub restores: Vec<Write>,
    pub compiler_prepare_us: u64,
    pub compiler_scanned_bytes: u64,
    pub compiler_relative_records: u64,
    /// Legacy opaque certificates never authorize live patching.
    pub certificates: Option<Vec<u8>>,
    pub async_certificates: Option<Vec<crate::async_certs::Certificate>>,
}
fn range(addr: u64, len: u64) -> Result<()> {
    if len == 0 || addr.checked_add(len).is_none() {
        Err(Error("invalid manifest range"))
    } else {
        Ok(())
    }
}
fn write(r: &mut Reader<'_>, named: bool) -> Result<Write> {
    let addr = r.u64()?;
    let kind = r.u32()?;
    let payload = r.blob()?;
    if !matches!(kind, 1 | 2) {
        return Err(Error("invalid manifest write kind"));
    }
    range(addr, payload.len() as u64)?;
    let fixups = r.fixups(payload.len())?;
    let name = if named { r.blob()? } else { vec![] };
    Ok(Write {
        addr,
        kind,
        payload,
        fixups,
        name,
    })
}
pub fn parse(bytes: &[u8]) -> Result<Manifest> {
    let v1 = bytes.starts_with(V1_MAGIC);
    let body = if v1 {
        let split = bytes.len().checked_sub(4).ok_or(Error("truncated CRC"))?;
        let (body, crc) = bytes.split_at(split);
        if crc32(body) != u32::from_le_bytes(crc.try_into().unwrap()) {
            return Err(Error("manifest CRC mismatch"));
        }
        body
    } else {
        bytes
    };
    let mut r = Reader::new(body);
    let (version, flags) = if v1 {
        r.take(4)?;
        let version = u16::from_le_bytes(r.take(2)?.try_into().unwrap());
        let flags = u16::from_le_bytes(r.take(2)?.try_into().unwrap());
        if version != VERSION || flags & !FLAG_CERTIFICATES != 0 {
            return Err(Error("unsupported manifest version/flags"));
        }
        (version, flags)
    } else {
        if r.take(8)? != MAGIC {
            return Err(Error("invalid manifest magic"));
        }
        (0, 0)
    };
    let generation = r.u64()?;
    if generation == 0 {
        return Err(Error("zero generation"));
    }
    let mut segments = Vec::new();
    for _ in 0..r.count(20)? {
        let addr = r.u64()?;
        let len = r.u64()?;
        let prot = r.u32()?;
        range(addr, len)?;
        if addr % 4096 != 0 || prot & !7 != 0 {
            return Err(Error("invalid manifest segment"));
        }
        segments.push(Segment { addr, len, prot });
    }
    let mut pages = Vec::new();
    for _ in 0..r.count(20)? {
        let addr = r.u64()?;
        let prot = r.u32()?;
        let mut payload = r.blob()?;
        if payload.is_empty() {
            payload.resize(4096, 0);
        }
        if addr % 4096 != 0 || payload.len() != 4096 || prot & !7 != 0 {
            return Err(Error("invalid manifest page"));
        }
        range(addr, 4096)?;
        let fixups = r.fixups(payload.len())?;
        pages.push(Page {
            addr,
            prot,
            payload,
            fixups,
        });
    }
    let mut writes = Vec::new();
    for _ in 0..r.count(24)? {
        writes.push(write(&mut r, true)?);
    }
    let mut restores = Vec::new();
    let mut symbols = Vec::new();
    let unwind_regions = if v1 {
        let mut regions = Vec::new();
        let mut last_end = 0;
        for _ in 0..r.count(16)? {
            let addr = r.u64()?;
            let len = r.u64()?;
            range(addr, len)?;
            if addr < last_end
                || !segments.iter().any(|s| {
                    s.prot == 1
                        && s.addr <= addr
                        && addr + len
                            <= (s.addr + s.len)
                                .checked_add(4095)
                                .map(|v| v & !4095)
                                .unwrap_or(0)
                })
            {
                return Err(Error("invalid unwind region"));
            }
            last_end = addr + len;
            regions.push(UnwindRegion { addr, len });
        }
        for _ in 0..r.count(32)? {
            let addr = r.u64()?;
            let size = r.u64()?;
            range(addr, size)?;
            if !segments
                .iter()
                .any(|s| s.prot & 4 != 0 && s.addr <= addr && addr + size <= s.addr + s.len)
            {
                return Err(Error("symbol outside executable segment"));
            }
            let name =
                String::from_utf8(r.blob()?).map_err(|_| Error("invalid symbol name UTF-8"))?;
            let file =
                String::from_utf8(r.blob()?).map_err(|_| Error("invalid symbol file UTF-8"))?;
            let first_line = r.u32()?;
            let line_table = r.blob()?;
            if name.is_empty() {
                return Err(Error("empty symbol name"));
            }
            symbols.push(Symbol {
                addr,
                size,
                name,
                file,
                first_line,
                line_table,
            });
        }
        Some(regions)
    } else {
        for _ in 0..r.count(20)? {
            restores.push(write(&mut r, false)?);
        }
        None
    };
    let compiler_prepare_us = r.u64()?;
    let compiler_scanned_bytes = r.u64()?;
    let compiler_relative_records = r.u64()?;
    let cumulative_roots = if v1 { r.u64()? } else { 0 };
    let retained_bytes = if v1 { r.u64()? } else { 0 };
    let mut certificates = None;
    let mut async_certificates = None;
    if (!v1 && r.remaining() > 0) || flags & FLAG_CERTIFICATES != 0 {
        let magic = r.take(8)?;
        let bytes = r.blob()?;
        if magic == b"AC109001" {
            certificates = Some(bytes);
        } else if v1 && magic == crate::async_certs::MAGIC {
            async_certificates = Some(crate::async_certs::parse(&bytes)?);
        } else {
            return Err(Error("unknown manifest trailer"));
        }
    }
    r.finish()?;
    Ok(Manifest {
        version,
        flags,
        unwind_regions,
        symbols,
        cumulative_roots,
        retained_bytes,
        epoch_due: epoch_due(cumulative_roots, retained_bytes),
        generation,
        segments,
        pages,
        writes,
        restores,
        compiler_prepare_us,
        compiler_scanned_bytes,
        compiler_relative_records,
        certificates,
        async_certificates,
    })
}
/// IEEE CRC32 of the manifest bytes preceding the checksum field.
pub fn crc32(bytes: &[u8]) -> u32 {
    crc32fast::hash(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn binary_async_section_is_crc_fenced_and_typed() {
        let mut b = v1();
        b.truncate(b.len() - 4);
        b[6..8].copy_from_slice(&FLAG_CERTIFICATES.to_le_bytes());
        b.extend(crate::async_certs::MAGIC);
        let rows = crate::async_certs::tests::sample();
        b.extend((rows.len() as u32).to_le_bytes());
        b.extend(&rows);
        let wire = seal(b.clone());
        let parsed = parse(&wire).unwrap();
        assert!(parsed.certificates.is_none());
        assert_eq!(parsed.async_certificates.unwrap().len(), 1);
        let mut bad_crc = wire;
        let end = bad_crc.len();
        bad_crc[end - 5] ^= 1;
        assert!(parse(&bad_crc).is_err());
        b[6..8].copy_from_slice(&0u16.to_le_bytes());
        assert!(parse(&seal(b)).is_err());
    }
    #[test]
    fn epoch_footer_limits_are_bounded_and_independent() {
        assert!(!epoch_due(0, 0));
        assert!(!epoch_due(64, EPOCH_RETAINED_LIMIT - 1));
        assert!(epoch_due(65, 0));
        assert!(epoch_due(0, EPOCH_RETAINED_LIMIT));
        assert!(epoch_due(u64::MAX, u64::MAX));
    }
    fn empty() -> Vec<u8> {
        let mut b = MAGIC.to_vec();
        b.extend(1_u64.to_le_bytes());
        b.extend([0; 16]);
        b.extend([0; 24]);
        b
    }
    fn seal(mut b: Vec<u8>) -> Vec<u8> {
        b.extend(crc32(&b).to_le_bytes());
        b
    }
    fn v1() -> Vec<u8> {
        let mut b = b"RDM1".to_vec();
        b.extend(1u16.to_le_bytes());
        b.extend(0u16.to_le_bytes());
        b.extend(1u64.to_le_bytes());
        b.extend(2u32.to_le_bytes());
        for (addr, prot) in [(0x4000u64, 1u32), (0x8000, 5)] {
            b.extend(addr.to_le_bytes());
            b.extend(4096u64.to_le_bytes());
            b.extend(prot.to_le_bytes());
        }
        b.extend(0u32.to_le_bytes()); // pages (parser is generation-local)
        b.extend(0u32.to_le_bytes()); // writes
        b.extend(1u32.to_le_bytes());
        b.extend(0x4000u64.to_le_bytes());
        b.extend(28u64.to_le_bytes());
        b.extend(1u32.to_le_bytes());
        b.extend(0x8000u64.to_le_bytes());
        b.extend(16u64.to_le_bytes());
        b.extend(7u32.to_le_bytes());
        b.extend(b"app::fn");
        b.extend(11u32.to_le_bytes());
        b.extend(b"src/main.rs");
        b.extend(42u32.to_le_bytes());
        b.extend(0u32.to_le_bytes());
        for n in [1u64, 8192, 0, 1, 8192] {
            b.extend(n.to_le_bytes());
        }
        seal(b)
    }
    #[test]
    fn crc_standard_vector_and_v1_sections() {
        assert_eq!(crc32(b"123456789"), 0xcbf43926);
        let m = parse(&v1()).unwrap();
        assert_eq!(m.version, 1);
        assert_eq!(
            m.unwind_regions.unwrap(),
            vec![UnwindRegion {
                addr: 0x4000,
                len: 28
            }]
        );
        assert_eq!(
            (
                m.symbols[0].name.as_str(),
                m.symbols[0].file.as_str(),
                m.symbols[0].first_line
            ),
            ("app::fn", "src/main.rs", 42)
        );
        assert!(m.restores.is_empty());
        assert_eq!(m.retained_bytes, 8192);
    }
    #[test]
    fn v1_truncation_corruption_flags_and_certificate() {
        let b = v1();
        for n in 0..b.len() {
            assert!(parse(&b[..n]).is_err(), "prefix {n}");
        }
        for at in 0..b.len() {
            let mut changed = b.clone();
            changed[at] ^= 1;
            assert!(parse(&changed).is_err(), "corruption {at}");
        }
        for (at, value) in [(4, 2), (6, 2), (84, 0xff), (108, 0xff)] {
            let mut changed = b[..b.len() - 4].to_vec();
            changed[at] = value;
            assert!(parse(&seal(changed)).is_err(), "semantic mutation {at}");
        }
        let mut cert = b[..b.len() - 4].to_vec();
        cert[6] = FLAG_CERTIFICATES as u8;
        assert!(parse(&seal(cert.clone())).is_err());
        cert.extend(b"AC109001");
        cert.extend(2u32.to_le_bytes());
        cert.extend(b"[]");
        assert_eq!(
            parse(&seal(cert)).unwrap().certificates,
            Some(b"[]".to_vec())
        );
    }
    #[test]
    fn v1_rechecksummed_fuzz_is_bounded_and_never_panics() {
        let b = v1();
        let mut seed = 73u64;
        for n in 0..8192 {
            let mut changed = b[..b.len() - 4].to_vec();
            for _ in 0..1 + n % 13 {
                seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
                let at = (seed >> 32) as usize % changed.len();
                changed[at] = (seed >> 24) as u8;
            }
            let _ = parse(&seal(changed));
        }
    }
    #[test]
    fn empty_and_certificate() {
        let mut b = empty();
        assert_eq!(parse(&b).unwrap().generation, 1);
        b.extend(b"AC109001");
        b.extend(2_u32.to_le_bytes());
        b.extend(b"[]");
        assert_eq!(parse(&b).unwrap().certificates, Some(b"[]".to_vec()));
    }
    #[test]
    fn truncation_and_garbage() {
        let b = empty();
        for n in 0..b.len() {
            assert!(parse(&b[..n]).is_err());
        }
        let mut seed = 7_u64;
        for n in 0..4096 {
            let mut b = empty();
            for _ in 0..n % 67 {
                seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
                let at = (seed >> 32) as usize % b.len();
                b[at] = (seed >> 24) as u8;
            }
            let _ = parse(&b);
        }
    }
}
