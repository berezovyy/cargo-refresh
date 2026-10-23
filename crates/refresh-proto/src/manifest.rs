//! RDM09001 manifest, with an optional opaque AC109001 certificate trailer.
use crate::{Error, Reader, Result};
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
#[derive(Clone, Debug)]
pub struct Manifest {
    pub generation: u64,
    pub segments: Vec<Segment>,
    pub pages: Vec<Page>,
    pub writes: Vec<Write>,
    pub restores: Vec<Write>,
    pub compiler_prepare_us: u64,
    pub compiler_scanned_bytes: u64,
    pub compiler_relative_records: u64,
    /// Certificates are ignored by the v0.1 classifier; no JSON decoding on saves.
    pub certificates: Option<Vec<u8>>,
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
    let mut r = Reader::new(bytes);
    if r.take(8)? != MAGIC {
        return Err(Error("invalid manifest magic"));
    }
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
    for _ in 0..r.count(20)? {
        restores.push(write(&mut r, false)?);
    }
    let compiler_prepare_us = r.u64()?;
    let compiler_scanned_bytes = r.u64()?;
    let compiler_relative_records = r.u64()?;
    let certificates = if r.remaining() > 0 {
        if r.take(8)? != b"AC109001" {
            return Err(Error("unknown manifest trailer"));
        }
        Some(r.blob()?)
    } else {
        None
    };
    r.finish()?;
    Ok(Manifest {
        generation,
        segments,
        pages,
        writes,
        restores,
        compiler_prepare_us,
        compiler_scanned_bytes,
        compiler_relative_records,
        certificates,
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    fn empty() -> Vec<u8> {
        let mut b = MAGIC.to_vec();
        b.extend(1_u64.to_le_bytes());
        b.extend([0; 16]);
        b.extend([0; 24]);
        b
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
