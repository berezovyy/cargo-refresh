pub fn panic_site(trigger: bool) -> u64 { if trigger { panic!("backtrace fixture panic"); } 1 }
