use std::any::TypeId;
use std::collections::HashMap;
use std::sync::Arc;

struct abi_private(u8);
struct abi_generic<T>(T);
type abi_u8 = u8;
type abi_u64 = u64;
type abi_str = &'static str;
type abi_string = String;
type abi_external = HashMap<u32, Arc<str>>;
type abi_own = abi_generic<abi_private>;
type abi_fn_ptr = fn();
fn function_item() {}
fn show<T: 'static>(name: &str) {
    println!("{name} {:?}", TypeId::of::<T>());
}
fn item<T: 'static>(_: T) { show::<T>("function-item"); }
fn main() {
    show::<abi_u8>("abi_u8");
    show::<abi_u64>("abi_u64");
    show::<abi_str>("abi_str");
    show::<abi_string>("abi_string");
    show::<abi_external>("abi_external");
    show::<abi_private>("abi_private");
    show::<abi_own>("abi_own");
    show::<abi_fn_ptr>("abi_fn_ptr");
    item(function_item);
    let _ = abi_private(0).0;
}
