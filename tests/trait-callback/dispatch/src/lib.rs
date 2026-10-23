pub trait Label {
    fn label(&self) -> u64;
}

#[repr(C, align(32))]
struct Aligned([u64; 4]);

#[inline(never)]
extern "C" fn consume(value: Aligned) {
    std::hint::black_box(value);
}

// rustd refuses a SysV C argument in memory aligned beyond 16 bytes, then
// forwards this generic instance to rustc. The owned callback still runs.
#[inline(never)]
pub fn dispatch<T: Label>(value: &T) -> u64 {
    consume(Aligned([0; 4]));
    value.label()
}
