#![deny(unsafe_code)]

/// The first call in every main. A panic on any thread then ends the process
/// at once with status 101, so it never runs half alive with a dead worker or
/// a poisoned lock. The report names only the place, since a panic message can
/// quote input. It goes out through `vigil_log::say!`, which allocates nothing,
/// does not panic, and drops the line rather than wait on a stalled pipe,
/// socket or tty, so a broken stderr cannot hold up the exit, short of the
/// pipe race that `say!` describes.
pub fn exit_on_panic() {
    #[expect(clippy::disallowed_methods, reason = "this is the one panic hook")]
    std::panic::set_hook(Box::new(|info| {
        match info.location() {
            Some(at) => vigil_log::say!("panic at {}:{}; exiting", at.file(), at.line()),
            None => vigil_log::say!("panic at an unknown place; exiting"),
        }
        exit_now(101);
    }));
}

// std::process::exit runs C atexit handlers while other threads still run, and
// it flushes a buffered stdout, which blocks on a pipe nobody drains.
#[expect(unsafe_code, reason = "_exit has no safe wrapper in std")]
fn exit_now(status: i32) -> ! {
    // SAFETY: _exit takes no pointers and only ends the process.
    unsafe { libc::_exit(status) }
}
