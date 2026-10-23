//! PT_GNU_EH_FRAME decoding, matching unwind121.py regions().
use crate::elf::{Image, Program, slice, u32_at};
use anyhow::{Context, Result, ensure};
use refresh_proto::wire::Record;
use std::collections::BTreeSet;

fn added(image: &Image, original: &BTreeSet<(u64, u64, u32)>) -> Vec<Program> {
    image
        .loads()
        .filter(|p| !original.contains(&p.identity()) && p.memsz > 0 && p.offset != image.phoff)
        .cloned()
        .collect()
}
fn source_range<'a>(
    image: &Image,
    appended: &'a [Program],
    addr: u64,
    n: u64,
) -> Result<(&'a Program, u64)> {
    let end = addr.checked_add(n).context("unwind range overflow")?;
    let p = appended
        .iter()
        .find(|p| {
            p.memsz
                .checked_add(4095)
                .and_then(|v| p.addr.checked_add(v & !4095))
                .is_some_and(|limit| p.addr <= addr && end <= limit)
        })
        .context("unwind outside appended memory")?;
    ensure!(
        p.flags == 4,
        "unwind region must be appended read-only memory"
    );
    ensure!(
        !image
            .loads()
            .any(|q| q != p && q.addr < end && q.addr + q.memsz > addr.max(p.addr + p.memsz)),
        "unwind padding overlaps another LOAD"
    );
    let take = n.min((p.addr + p.filesz).saturating_sub(addr));
    if take > 0 {
        slice(&image.data, p.offset + addr - p.addr, take)?;
    }
    Ok((p, take))
}
fn read(image: &Image, appended: &[Program], addr: u64, n: u64) -> Result<Vec<u8>> {
    // Callers read only length/back-pointer fields. Region validation below
    // deliberately does not allocate an attacker-controlled CIE/FDE length.
    ensure!(n <= 8, "oversized unwind field read");
    let (p, take) = source_range(image, appended, addr, n)?;
    let mut out = if take > 0 {
        slice(&image.data, p.offset + addr - p.addr, take)?.to_vec()
    } else {
        vec![]
    };
    out.resize(n as usize, 0);
    Ok(out)
}
fn relative(addr: u64, off: i32) -> Result<u64> {
    addr.checked_add_signed(i64::from(off))
        .context("unwind relative pointer overflow")
}
pub fn regions(image: &Image, original: &BTreeSet<(u64, u64, u32)>) -> Result<Vec<(u64, u64)>> {
    let hdr = image
        .headers
        .iter()
        .find(|p| p.typ == 0x6474e550)
        .context("missing PT_GNU_EH_FRAME")?;
    let raw = slice(&image.data, hdr.offset, hdr.filesz)?;
    ensure!(
        raw.get(..4) == Some(&[1, 0x1b, 3, 0x3b]),
        "unsupported unwind-header encoding"
    );
    let count = u32_at(raw, 8)? as usize;
    ensure!(
        count <= raw.len().saturating_sub(12) / 8,
        "truncated unwind table"
    );
    let appended = added(image, original);
    let mut fdes = BTreeSet::new();
    for j in 0..count {
        let fde = relative(hdr.addr, u32_at(raw, 16 + j * 8)? as i32)?;
        if appended.iter().any(|p| p.contains(fde, 1, false)) {
            fdes.insert(fde);
        }
    }
    let mut cies = BTreeSet::new();
    for &f in &fdes {
        let raw = read(image, &appended, f, 8)?;
        let length = u32_at(&raw, 0)?;
        let back = u32_at(&raw, 4)?;
        ensure!(length > 0 && length < u32::MAX && back > 0, "invalid FDE");
        let cie = f
            .checked_add(4)
            .and_then(|v| v.checked_sub(back as u64))
            .context("CIE pointer overflow")?;
        ensure!(
            u32_at(
                &read(
                    image,
                    &appended,
                    cie.checked_add(4).context("CIE overflow")?,
                    4
                )?,
                0
            )? == 0,
            "FDE does not point to a CIE"
        );
        cies.insert(cie);
    }
    let entries: BTreeSet<_> = cies.union(&fdes).copied().collect();
    let mut groups: Vec<(u64, u64)> = Vec::new();
    for &start in &entries {
        if groups.last().is_some_and(|p| start < p.1) {
            continue;
        }
        let mut at = start;
        while entries.contains(&at) {
            let length = u32_at(&read(image, &appended, at, 4)?, 0)?;
            ensure!((4..u32::MAX).contains(&length), "invalid CIE/FDE length");
            // Validate the full source region without copying it.
            let n = 4 + u64::from(length);
            source_range(image, &appended, at, n)?;
            at = at.checked_add(n).context("unwind region overflow")?;
        }
        ensure!(cies.contains(&start), "region starts without a CIE");
        groups.push((start, at));
    }
    ensure!(
        fdes.iter()
            .all(|f| groups.iter().any(|&(a, b)| a <= *f && *f < b)),
        "uncovered FDE"
    );
    Ok(groups.into_iter().map(|(a, b)| (a, b - a)).collect())
}
pub fn descriptors(image: &Image, original: &BTreeSet<(u64, u64, u32)>) -> Result<Vec<Record>> {
    Ok(regions(image, original)?
        .into_iter()
        .map(|(addr, len)| Record {
            addr,
            prot: 1,
            kind: 3,
            payload: len.to_le_bytes().to_vec(),
            fixups: vec![],
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_images::{candidate, put};
    fn original() -> BTreeSet<(u64, u64, u32)> {
        BTreeSet::from([(0x1000, 0x1000, 4)])
    }
    #[test]
    fn cie_fde_group_and_descriptor() {
        let e = candidate();
        assert_eq!(regions(&e, &original()).unwrap(), vec![(0x4000, 28)]);
        let d = descriptors(&e, &original()).unwrap();
        assert_eq!(d[0].kind, 3);
        assert_eq!(d[0].payload, 28_u64.to_le_bytes());
    }
    #[test]
    fn rejects_bad_header_backlink_length_or_permissions() {
        for (addr, bytes) in [
            (0x1300, vec![2]),
            (0x1308, u32::MAX.to_le_bytes().to_vec()),
            (0x4010, 0_u32.to_le_bytes().to_vec()),
            (0x400c, u32::MAX.to_le_bytes().to_vec()),
            (0x4004, 1_u32.to_le_bytes().to_vec()),
        ] {
            let mut e = candidate();
            put(&mut e, addr, &bytes);
            assert!(regions(&e, &original()).is_err());
        }
        let mut e = candidate();
        e.headers[1].flags = 6;
        assert!(regions(&e, &original()).is_err());
    }
}
