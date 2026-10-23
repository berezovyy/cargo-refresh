//! Protocol accepted by the EXP-122/128 agent. All integers are little endian.
use crate::{Error, Reader, Result};
pub const MAGIC: u32 = 0x76313232;
pub const MAX_RECORDS: usize = 8192;
pub const MAX_PAYLOAD: usize = 128 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Record {
    pub addr: u64,
    pub prot: u32,
    pub kind: u32,
    pub payload: Vec<u8>,
    pub fixups: Vec<u32>,
}
impl Record {
    pub fn validate(&self) -> Result<()> {
        let n = self.payload.len();
        if n == 0 || n > MAX_PAYLOAD || self.addr.checked_add(n as u64).is_none() {
            return Err(Error("invalid record range"));
        }
        match self.kind {
            0 => {
                if self.prot & !7 != 0 {
                    return Err(Error("invalid page protection"));
                }
            }
            1 => {
                if self.prot != 5 || n != 5 {
                    return Err(Error("invalid entry record"));
                }
            }
            2 => {
                if self.prot != 1 || n > 4096 {
                    return Err(Error("invalid data record"));
                }
            }
            3 => {
                if self.prot != 1 || n != 8 || !self.fixups.is_empty() {
                    return Err(Error("invalid unwind record"));
                }
            }
            _ => return Err(Error("unknown record kind")),
        }
        validate_fixups(&self.fixups, n)
    }
}
pub(crate) fn validate_fixups(fix: &[u32], len: usize) -> Result<()> {
    for (i, &off) in fix.iter().enumerate() {
        if u64::from(off) + 8 > len as u64 || (i > 0 && u64::from(off) < u64::from(fix[i - 1]) + 8)
        {
            return Err(Error("out-of-range or overlapping fixup"));
        }
    }
    Ok(())
}
pub fn encode(records: &[Record]) -> Result<Vec<u8>> {
    if records.len() > MAX_RECORDS {
        return Err(Error("too many records"));
    }
    let mut out = Vec::new();
    out.extend(MAGIC.to_le_bytes());
    out.extend((records.len() as u32).to_le_bytes());
    for r in records {
        r.validate()?;
        out.extend(r.addr.to_le_bytes());
        out.extend((r.payload.len() as u64).to_le_bytes());
        out.extend(r.prot.to_le_bytes());
        out.extend(r.kind.to_le_bytes());
        out.extend((r.fixups.len() as u32).to_le_bytes());
        out.extend(&r.payload);
        for &fix in &r.fixups {
            out.extend(fix.to_le_bytes());
        }
    }
    Ok(out)
}
pub fn decode(bytes: &[u8]) -> Result<Vec<Record>> {
    let mut r = Reader::new(bytes);
    if r.u32()? != MAGIC {
        return Err(Error("invalid agent magic"));
    }
    let n = r.count(28)?;
    if n > MAX_RECORDS {
        return Err(Error("too many records"));
    }
    let mut records = Vec::new();
    for _ in 0..n {
        let addr = r.u64()?;
        let len = r.u64()?;
        if len > MAX_PAYLOAD as u64 {
            return Err(Error("oversized payload"));
        }
        let prot = r.u32()?;
        let kind = r.u32()?;
        let nfix = r.u32()? as usize;
        let payload = r.take(len as usize)?.to_vec();
        if nfix > r.remaining() / 4 || nfix > payload.len() / 8 {
            return Err(Error("invalid fixup count"));
        }
        let fixups = (0..nfix).map(|_| r.u32()).collect::<Result<Vec<_>>>()?;
        let record = Record {
            addr,
            prot,
            kind,
            payload,
            fixups,
        };
        record.validate()?;
        records.push(record);
    }
    r.finish()?;
    Ok(records)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn roundtrip_and_restored_entry() {
        let records = vec![
            Record {
                addr: 0x1000,
                prot: 5,
                kind: 1,
                payload: vec![0x55, 1, 2, 3, 4],
                fixups: vec![],
            },
            Record {
                addr: 0x2000,
                prot: 1,
                kind: 2,
                payload: vec![0; 24],
                fixups: vec![0, 8],
            },
            Record {
                addr: 0x3000,
                prot: 1,
                kind: 3,
                payload: 48_u64.to_le_bytes().to_vec(),
                fixups: vec![],
            },
        ];
        let bytes = encode(&records).unwrap();
        assert_eq!(decode(&bytes).unwrap(), records);
        for n in 0..bytes.len() {
            assert!(decode(&bytes[..n]).is_err());
        }
    }
    #[test]
    fn invalid_fixups_and_kinds() {
        for (kind, prot, n, fix) in [
            (0, 5, 16, vec![1, 8]),
            (2, 1, 16, vec![9]),
            (1, 1, 5, vec![]),
            (3, 1, 8, vec![0]),
            (4, 1, 8, vec![]),
            (2, 3, 8, vec![]),
        ] {
            assert!(
                encode(&[Record {
                    addr: 0,
                    prot,
                    kind,
                    payload: vec![0; n],
                    fixups: fix
                }])
                .is_err()
            );
        }
    }
    #[test]
    fn garbage_never_panics() {
        let mut seed = 23_u64;
        for n in 0..2048 {
            let bytes: Vec<_> = (0..n % 255)
                .map(|_| {
                    seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
                    (seed >> 32) as u8
                })
                .collect();
            let _ = decode(&bytes);
        }
    }
}
