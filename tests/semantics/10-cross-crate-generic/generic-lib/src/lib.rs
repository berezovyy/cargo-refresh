#[inline(never)]
pub fn value<T>() -> usize {
    std::mem::size_of::<T>() + 1
}
