#![deny(unsafe_code)]

fn undocumented(p: *const u8) -> u8 {
    unsafe { *p }
}

unsafe fn bare(p: *const u8) -> u8 {
    *p
}

fn two_operations(p: *const u8) -> u8 {
    // SAFETY: never run; this file exists to be rejected.
    unsafe { bare(p) + *p }
}

fn main() {
    std::panic::set_hook(std::panic::take_hook());
    let _ = std::io::stderr();
    let _: Option<std::io::Stdout> = None;
    println!("canary");
    let byte = dbg!(0u8);
    let _ = (undocumented(&byte), two_operations(&byte));
}
