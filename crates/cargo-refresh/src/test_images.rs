//! Small deterministic ELF-memory fixtures; no compiler or daemon needed.
use crate::elf::{Image, Program};
use std::collections::BTreeMap;
pub fn program(typ: u32, flags: u32, offset: u64, addr: u64, len: u64) -> Program {
    Program {
        typ,
        flags,
        offset,
        addr,
        paddr: addr,
        filesz: len,
        memsz: len,
        align: 4096,
    }
}
pub fn image() -> Image {
    Image {
        data: vec![0; 0x3000],
        typ: 3,
        phoff: 64,
        headers: vec![program(1, 4, 0, 0x1000, 0x1000)],
        dynamic: vec![],
        relatives: BTreeMap::new(),
        relas: BTreeMap::new(),
    }
}
pub fn put(image: &mut Image, addr: u64, bytes: &[u8]) {
    let at = image.offset(addr, bytes.len() as u64).unwrap() as usize;
    image.data[at..at + bytes.len()].copy_from_slice(bytes);
}
pub fn word(image: &mut Image, addr: u64, value: u64) {
    put(image, addr, &value.to_le_bytes());
}
pub fn candidate() -> Image {
    let mut e = image();
    e.headers.push(program(1, 4, 0x1000, 0x4000, 0x1000));
    e.headers.push(program(1, 5, 0x2000, 0x8000, 0x1000));
    e.headers.push(program(0x6474e550, 4, 0x300, 0x1300, 20));
    put(&mut e, 0x1300, &[1, 0x1b, 3, 0x3b]);
    put(&mut e, 0x1308, &1_u32.to_le_bytes());
    put(&mut e, 0x130c, &(0x8000_i32 - 0x1300).to_le_bytes());
    put(&mut e, 0x1310, &(0x400c_i32 - 0x1300).to_le_bytes());
    // A 12-byte CIE followed by a 16-byte FDE whose backward pointer is 16.
    put(&mut e, 0x4000, &8_u32.to_le_bytes());
    put(&mut e, 0x400c, &12_u32.to_le_bytes());
    put(&mut e, 0x4010, &16_u32.to_le_bytes());
    e
}
pub fn elf_bytes() -> Vec<u8> {
    let mut data = vec![0; 0x1000];
    data[..7].copy_from_slice(b"\x7fELF\x02\x01\x01");
    data[16..18].copy_from_slice(&3_u16.to_le_bytes());
    data[18..20].copy_from_slice(&62_u16.to_le_bytes());
    data[20..24].copy_from_slice(&1_u32.to_le_bytes());
    data[32..40].copy_from_slice(&64_u64.to_le_bytes());
    for (o, v) in [(52, 64_u16), (54, 56), (56, 2), (58, 64)] {
        data[o..o + 2].copy_from_slice(&v.to_le_bytes());
    }
    for (i, p) in [
        program(1, 4, 0, 0x1000, 0x1000),
        program(2, 4, 0x300, 0x1300, 64),
    ]
    .into_iter()
    .enumerate()
    {
        let o = 64 + i * 56;
        data[o..o + 4].copy_from_slice(&p.typ.to_le_bytes());
        data[o + 4..o + 8].copy_from_slice(&p.flags.to_le_bytes());
        for (j, v) in [p.offset, p.addr, p.paddr, p.filesz, p.memsz, p.align]
            .into_iter()
            .enumerate()
        {
            data[o + 8 + j * 8..o + 16 + j * 8].copy_from_slice(&v.to_le_bytes());
        }
    }
    for (i, (tag, value)) in [(7_u64, 0x1400_u64), (8, 24), (9, 24), (0, 0)]
        .into_iter()
        .enumerate()
    {
        let o = 0x300 + i * 16;
        data[o..o + 8].copy_from_slice(&tag.to_le_bytes());
        data[o + 8..o + 16].copy_from_slice(&value.to_le_bytes());
    }
    data[0x400..0x408].copy_from_slice(&0x1100_u64.to_le_bytes());
    data[0x408..0x410].copy_from_slice(&8_u64.to_le_bytes());
    data[0x410..0x418].copy_from_slice(&0x123456_u64.to_le_bytes());
    data
}
