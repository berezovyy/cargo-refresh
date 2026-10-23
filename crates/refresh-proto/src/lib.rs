//! Bounded readers and shared types for rustd manifests and the v0 agent wire.
pub mod async_certs;
pub mod manifest;
pub mod wire;

use std::fmt;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error(pub &'static str);
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}
impl std::error::Error for Error {}
pub type Result<T> = std::result::Result<T, Error>;

pub(crate) struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}
impl<'a> Reader<'a> {
    pub fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, at: 0 }
    }
    pub fn remaining(&self) -> usize {
        self.bytes.len() - self.at
    }
    pub fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self.at.checked_add(n).ok_or(Error("length overflow"))?;
        let out = self
            .bytes
            .get(self.at..end)
            .ok_or(Error("truncated input"))?;
        self.at = end;
        Ok(out)
    }
    pub fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    pub fn u64(&mut self) -> Result<u64> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }
    pub fn count(&mut self, minimum_size: usize) -> Result<usize> {
        let n = self.u32()? as usize;
        if n > self.remaining() / minimum_size {
            return Err(Error("count exceeds input"));
        }
        Ok(n)
    }
    pub fn blob(&mut self) -> Result<Vec<u8>> {
        let n = self.u32()? as usize;
        Ok(self.take(n)?.to_vec())
    }
    pub fn fixups(&mut self, len: usize) -> Result<Vec<u32>> {
        let n = self.count(4)?;
        if n > len / 8 {
            return Err(Error("too many fixups"));
        }
        let out = (0..n).map(|_| self.u32()).collect::<Result<Vec<_>>>()?;
        wire::validate_fixups(&out, len)?;
        Ok(out)
    }
    pub fn finish(&self) -> Result<()> {
        if self.remaining() == 0 {
            Ok(())
        } else {
            Err(Error("trailing bytes"))
        }
    }
}
