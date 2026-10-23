//! Pinned Rust 1.99 x86-64 immutable-data policy. Unknown data fails closed.
use crate::elf::{Image, Section, Symbol, u32_at, u64_at};
use anyhow::{Result, ensure};

pub trait Memory {
    fn read(&self, addr: u64, n: usize) -> Result<Vec<u8>>;
}
impl Memory for Image {
    fn read(&self, addr: u64, n: usize) -> Result<Vec<u8>> {
        Image::read(self, addr, n)
    }
}
fn read(e: &impl Memory, addr: u64, n: usize) -> Result<Vec<u8>> {
    ensure!(n <= 16384, "constant read length");
    e.read(addr, n)
}
fn string(e: &impl Memory, addr: u64, n: u64) -> Result<String> {
    ensure!(n > 0 && n < 4096, "string length");
    let raw = read(e, addr, n as usize)?;
    let text = String::from_utf8(raw)?;
    ensure!(!text.chars().any(|c| (c as u32) < 32), "non-text constant");
    Ok(text)
}
fn bounded(n: u32) -> bool {
    n > 0 && n < 10_000_000
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DataClass {
    PanicLocation,
    TracingMetadata,
}
pub fn classify_record(
    base: &Image,
    new: &impl Memory,
    sym: &Symbol,
    section: &Section,
    allow_tracing: bool,
) -> Result<(Vec<u8>, DataClass)> {
    let addr = sym.addr;
    let size = usize::try_from(sym.size)?;
    ensure!(
        matches!(section.name.as_str(), ".rodata" | ".data.rel.ro")
            && (section.flags & 1 == 0
                || base
                    .headers
                    .iter()
                    .any(|p| p.typ == 0x6474e552 && p.contains(addr, sym.size, false))),
        "original writable or unclassified section {}",
        section.name
    );
    ensure!(sym.typ == 1, "not a bounded data object");
    let old = read(base, addr, size)?;
    let fresh = read(new, addr, size)?;
    if size == 24
        && (sym.name.starts_with(".Lrustd.const.") || sym.name.starts_with("rustd::static::"))
    {
        let path = string(base, u64_at(&old, 0)?, u64_at(&old, 8)?)?;
        ensure!(
            path.ends_with(".rs") && path == string(new, u64_at(&fresh, 0)?, u64_at(&fresh, 8)?)?,
            "Location filename differs"
        );
        ensure!(
            [
                u32_at(&old, 16)?,
                u32_at(&old, 20)?,
                u32_at(&fresh, 16)?,
                u32_at(&fresh, 20)?
            ]
            .into_iter()
            .all(bounded),
            "Location bounds"
        );
        let mut out = old;
        out[16..].copy_from_slice(&fresh[16..]);
        return Ok((out, DataClass::PanicLocation));
    }
    ensure!(
        allow_tracing && size == 120 && sym.name.starts_with("rustd::static::"),
        "not an allowed Location symbol/record"
    );
    let a = (0..15)
        .map(|i| u64_at(&old, i * 8))
        .collect::<Result<Vec<_>>>()?;
    let b = (0..15)
        .map(|i| u64_at(&fresh, i * 8))
        .collect::<Result<Vec<_>>>()?;
    ensure!(
        old[..12] == fresh[..12]
            && (1..=5).contains(&a[0])
            && u32_at(&old, 8)? == 1
            && matches!(a[14], 1 | 2)
            && a[14] == b[14],
        "tracing scalar/discriminant changed"
    );
    let line = u32_at(&old, 12)?;
    let nline = u32_at(&fresh, 12)?;
    ensure!(bounded(line) && bounded(nline), "tracing line bounds");
    ensure!(
        [5, 7, 8, 11, 13].into_iter().all(|i| a[i] == b[i]),
        "tracing identity/shape changed"
    );
    let module = string(base, a[10], a[11])?;
    let file = string(base, a[12], a[13])?;
    ensure!(
        file.ends_with(".rs") && module.contains("::"),
        "tracing source path"
    );
    for i in [4, 10, 12] {
        ensure!(
            string(base, a[i], a[i + 1])? == string(new, b[i], b[i + 1])?,
            "tracing target/module/file differs"
        );
    }
    let oldname = string(base, a[2], a[3])?;
    let newname = string(new, b[2], b[3])?;
    ensure!(
        oldname == newname
            || (oldname == format!("event {file}:{line}")
                && newname == format!("event {file}:{nline}")),
        "tracing name is not the same span/event"
    );
    ensure!(a[7] <= 128, "tracing field count");
    for i in 0..a[7] {
        let op = read(
            base,
            a[6].checked_add(16 * i)
                .ok_or_else(|| anyhow::anyhow!("field overflow"))?,
            16,
        )?;
        let np = read(
            new,
            b[6].checked_add(16 * i)
                .ok_or_else(|| anyhow::anyhow!("field overflow"))?,
            16,
        )?;
        ensure!(
            string(base, u64_at(&op, 0)?, u64_at(&op, 8)?)?
                == string(new, u64_at(&np, 0)?, u64_at(&np, 8)?)?,
            "tracing fields changed"
        );
    }
    ensure!(
        u64_at(&read(base, a[8], 24)?, 0)? == addr && u64_at(&read(new, b[8], 24)?, 0)? == addr,
        "tracing callsite backlink mismatch"
    );
    ensure!(
        read(base, a[9], 48)? == read(new, b[9], 48)?,
        "tracing vtable differs"
    );
    let mut out = old;
    out[12..16].copy_from_slice(&fresh[12..16]);
    if oldname != newname {
        out[16..32].copy_from_slice(&fresh[16..32]);
    }
    Ok((out, DataClass::TracingMetadata))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::elf::Program;
    use crate::test_images::{image, put, word};
    fn symbol(size: u64) -> Symbol {
        Symbol {
            name: "rustd::static::test".into(),
            addr: 0x1100,
            size,
            typ: 1,
            section: 1,
            offset: 0x100,
        }
    }
    fn section() -> Section {
        Section {
            name: ".rodata".into(),
            flags: 2,
        }
    }
    fn location() -> (Image, Image, Symbol) {
        let mut base = image();
        let mut fresh = image();
        for e in [&mut base, &mut fresh] {
            put(e, 0x1500, b"src/lib.rs");
            word(e, 0x1100, 0x1500);
            word(e, 0x1108, 10);
            put(e, 0x1110, &10_u32.to_le_bytes());
            put(e, 0x1114, &2_u32.to_le_bytes());
        }
        put(&mut fresh, 0x1110, &11_u32.to_le_bytes());
        (base, fresh, symbol(24))
    }
    #[test]
    fn location_canonicalizes_filename_and_checks_bounds() {
        let (base, mut fresh, sym) = location();
        put(&mut fresh, 0x1600, b"src/lib.rs");
        word(&mut fresh, 0x1100, 0x1600);
        let (out, class) = classify_record(&base, &fresh, &sym, &section(), false).unwrap();
        assert_eq!(class, DataClass::PanicLocation);
        assert_eq!(u64_at(&out, 0).unwrap(), 0x1500);
        assert_eq!(u32_at(&out, 16).unwrap(), 11);
        for value in [0, 10_000_000, u32::MAX] {
            put(&mut fresh, 0x1110, &value.to_le_bytes());
            assert!(classify_record(&base, &fresh, &sym, &section(), false).is_err());
        }
    }
    #[test]
    fn rejects_unknown_mutable_and_changed_file() {
        let (base, mut fresh, mut sym) = location();
        put(&mut fresh, 0x1500, b"src/mod.rs");
        assert!(classify_record(&base, &fresh, &sym, &section(), false).is_err());
        let (base, fresh, _) = location();
        sym.name = "ordinary_static".into();
        assert!(classify_record(&base, &fresh, &sym, &section(), false).is_err());
        sym.name = "rustd::static::test".into();
        sym.typ = 0;
        assert!(classify_record(&base, &fresh, &sym, &section(), false).is_err());
        sym.typ = 1;
        for name in [".data", ".bss", ".text"] {
            assert!(
                classify_record(
                    &base,
                    &fresh,
                    &sym,
                    &Section {
                        name: name.into(),
                        flags: 2
                    },
                    false
                )
                .is_err()
            );
        }
        let sec = Section {
            name: ".data.rel.ro".into(),
            flags: 3,
        };
        assert!(classify_record(&base, &fresh, &sym, &sec, false).is_err());
        let mut base = base;
        base.headers.push(Program {
            typ: 0x6474e552,
            flags: 4,
            offset: 0x100,
            addr: 0x1100,
            paddr: 0x1100,
            filesz: 24,
            memsz: 24,
            align: 1,
        });
        assert!(classify_record(&base, &fresh, &sym, &sec, false).is_ok());
    }
    fn metadata() -> (Image, Image, Symbol) {
        let mut base = image();
        let mut fresh = image();
        for e in [&mut base, &mut fresh] {
            put(e, 0x1500, b"src/lib.rs");
            put(e, 0x1520, b"crate::module");
            put(e, 0x1540, b"target");
            put(e, 0x1560, b"event src/lib.rs:10");
            put(e, 0x1590, b"field");
            let a = [
                3,
                (10_u64 << 32) | 1,
                0x1560,
                19,
                0x1540,
                6,
                0x1600,
                1,
                0x1700,
                0x1800,
                0x1520,
                13,
                0x1500,
                10,
                2,
            ];
            for (i, v) in a.into_iter().enumerate() {
                word(e, 0x1100 + 8 * i as u64, v);
            }
            word(e, 0x1600, 0x1590);
            word(e, 0x1608, 5);
            word(e, 0x1700, 0x1100);
        }
        put(&mut fresh, 0x1560, b"event src/lib.rs:11");
        put(&mut fresh, 0x110c, &11_u32.to_le_bytes());
        (base, fresh, symbol(120))
    }
    #[test]
    fn tracing_opt_in_and_canonicalization() {
        let (base, mut fresh, sym) = metadata();
        assert!(classify_record(&base, &fresh, &sym, &section(), false).is_err());
        put(&mut fresh, 0x15b0, b"event src/lib.rs:11");
        word(&mut fresh, 0x1110, 0x15b0);
        // An equal target string moved to a new address is kept at its baseline pointer.
        put(&mut fresh, 0x15d0, b"target");
        word(&mut fresh, 0x1120, 0x15d0);
        let (out, class) = classify_record(&base, &fresh, &sym, &section(), true).unwrap();
        assert_eq!(class, DataClass::TracingMetadata);
        assert_eq!(u32_at(&out, 12).unwrap(), 11);
        assert_eq!(u64_at(&out, 16).unwrap(), 0x15b0);
        assert_eq!(u64_at(&out, 32).unwrap(), 0x1540);
    }
    #[test]
    fn all_tracing_refusal_fields() {
        // Each change independently breaks one scalar/identity/text/graph rule.
        for (addr, value) in [
            (0x1100, 6),
            (0x1108, 0),
            (0x1170, 1),
            (0x1128, 7),
            (0x1138, 129),
            (0x1140, 0x1710),
            (0x1158, 12),
            (0x1168, 9),
            (0x1700, 0x1110),
            (0x1800, 1),
            (0x1608, 4),
            (0x1130, u64::MAX),
            (0x1148, u64::MAX),
        ] {
            let (base, mut fresh, sym) = metadata();
            word(&mut fresh, addr, value);
            assert!(
                classify_record(&base, &fresh, &sym, &section(), true).is_err(),
                "accepted change at {addr:x}"
            );
        }
        for addr in [0x1500, 0x1520, 0x1540, 0x1560, 0x1590] {
            let (base, mut fresh, sym) = metadata();
            put(&mut fresh, addr, b"X");
            assert!(
                classify_record(&base, &fresh, &sym, &section(), true).is_err(),
                "accepted text change at {addr:x}"
            );
        }
    }
}
