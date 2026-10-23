#[inline(never)]
pub fn values() -> String {
    format!(
        "{},{}",
        generic_lib::value::<u8>(),
        generic_lib::value::<u64>()
    )
}
