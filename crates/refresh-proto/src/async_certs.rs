//! RDM1 async proof section `ACRT0001` (length-prefixed binary, little endian).
//! Version 1 protects frame/capture layouts and every suspension-delimited MIR
//! region. Only the entry region may change. Unknown formats fail closed.
use crate::{Error, Reader, Result};

pub const MAGIC: &[u8; 8] = b"ACRT0001";
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Descriptor {
    /// 1 = coroutine, 2 = synchronous closure used by an async region.
    pub kind: u8,
    pub owner_file: String,
    pub owner_path: String,
    pub frame: [u8; 16],
    pub entry: [u8; 16],
    pub resumes: Vec<(u32, [u8; 16])>,
}
impl Descriptor {
    pub fn protected_eq(&self, other: &Self) -> bool {
        self.kind == other.kind
            && self.owner_file == other.owner_file
            && self.owner_path == other.owner_path
            && self.frame == other.frame
            && self.resumes == other.resumes
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Certificate {
    pub symbol: String,
    /// Baseline proof captured when this instance was first compiled.
    pub old: Option<Descriptor>,
    pub new: Option<Descriptor>,
}
fn string(r: &mut Reader<'_>) -> Result<String> {
    String::from_utf8(r.blob()?).map_err(|_| Error("invalid async proof UTF-8"))
}
fn descriptor(r: &mut Reader<'_>) -> Result<Option<Descriptor>> {
    let kind = r.take(1)?[0];
    if kind == 0 {
        return Ok(None);
    }
    if !matches!(kind, 1 | 2) {
        return Err(Error("unknown async proof kind"));
    }
    let owner_file = string(r)?;
    let owner_path = string(r)?;
    let frame = r.take(16)?.try_into().unwrap();
    let entry = r.take(16)?.try_into().unwrap();
    let mut resumes = Vec::new();
    for _ in 0..r.count(20)? {
        let state = r.u32()?;
        if resumes
            .last()
            .is_some_and(|(previous, _)| *previous >= state)
        {
            return Err(Error("unordered async resume states"));
        }
        resumes.push((state, r.take(16)?.try_into().unwrap()));
    }
    if (kind == 1 && resumes.iter().any(|r| r.0 < 3))
        || (kind == 2 && (entry != [0; 16] || resumes.len() != 1 || resumes[0].0 != 0))
    {
        return Err(Error("invalid async resume states"));
    }
    Ok(Some(Descriptor {
        kind,
        owner_file,
        owner_path,
        frame,
        entry,
        resumes,
    }))
}
pub fn parse(bytes: &[u8]) -> Result<Vec<Certificate>> {
    let mut r = Reader::new(bytes);
    let mut rows = Vec::new();
    let mut names = std::collections::BTreeSet::new();
    for _ in 0..r.count(6)? {
        let symbol = string(&mut r)?;
        if symbol.is_empty() || !names.insert(symbol.clone()) {
            return Err(Error("invalid/duplicate async proof symbol"));
        }
        rows.push(Certificate {
            symbol,
            old: descriptor(&mut r)?,
            new: descriptor(&mut r)?,
        });
    }
    r.finish()?;
    Ok(rows)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    fn blob(b: &mut Vec<u8>, s: &[u8]) {
        b.extend((s.len() as u32).to_le_bytes());
        b.extend(s);
    }
    fn proof(b: &mut Vec<u8>, kind: u8, states: &[u32]) {
        b.push(kind);
        blob(b, b"src/lib.rs");
        blob(b, b"app::handler");
        b.extend([7; 16]);
        b.extend([0; 16]);
        b.extend((states.len() as u32).to_le_bytes());
        for state in states {
            b.extend(state.to_le_bytes());
            b.extend([8; 16]);
        }
    }
    pub(crate) fn sample() -> Vec<u8> {
        let mut b = 1u32.to_le_bytes().to_vec();
        blob(&mut b, b"poll_handler");
        proof(&mut b, 1, &[3, 4]);
        proof(&mut b, 1, &[3, 4]);
        b
    }
    #[test]
    fn bounded_decode_and_protected_semantics() {
        let b = sample();
        let rows = parse(&b).unwrap();
        let old = rows[0].old.as_ref().unwrap();
        let mut new = rows[0].new.clone().unwrap();
        assert!(old.protected_eq(&new));
        new.entry[0] = 9;
        assert!(old.protected_eq(&new));
        new.resumes[1].1[0] = 9;
        assert!(!old.protected_eq(&new));
        for end in 0..b.len() {
            assert!(parse(&b[..end]).is_err(), "accepted truncation {end}");
        }
        let mut huge = b.clone();
        huge[..4].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(parse(&huge).is_err());
        let mut trailing = b;
        trailing.push(0);
        assert!(parse(&trailing).is_err());
    }
    #[test]
    fn unknown_kinds_duplicate_states_and_closure_shape_fail_closed() {
        for (kind, states) in [
            (3, vec![]),
            (1, vec![3, 3]),
            (1, vec![2]),
            (2, vec![]),
            (2, vec![3]),
        ] {
            let mut b = 1u32.to_le_bytes().to_vec();
            blob(&mut b, b"poll");
            proof(&mut b, kind, &states);
            b.push(0);
            assert!(parse(&b).is_err());
        }
    }
}
