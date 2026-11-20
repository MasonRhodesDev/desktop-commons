use std::ffi::OsStr;
use std::fs::{File, OpenOptions, Permissions};
use std::io::{PipeReader, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy)]
pub enum Stderr {
    DevFull,
    ReaderlessPipe,
    StalledPipe,
}

pub fn exit_code(binary: &str, args: &[impl AsRef<OsStr>], stderr: Stderr) -> Option<i32> {
    let mut _unread: Option<PipeReader> = None;
    let target: Stdio = match stderr {
        Stderr::DevFull => OpenOptions::new()
            .write(true)
            .open("/dev/full")
            .expect("open /dev/full")
            .into(),
        Stderr::ReaderlessPipe => std::io::pipe().expect("pipe").1.into(),
        Stderr::StalledPipe => {
            let (reader, writer) = std::io::pipe().expect("pipe");
            fill(&writer);
            _unread = Some(reader);
            writer.into()
        }
    };
    let mut child = Command::new(binary)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(target)
        .spawn()
        .expect("spawn");
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if let Some(status) = child.try_wait().expect("wait") {
            return status.code();
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let _ = child.kill();
    let _ = child.wait();
    None
}

pub fn fill(writer: &impl AsRawFd) {
    let mut own: File = OpenOptions::new()
        .write(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(format!("/proc/self/fd/{}", writer.as_raw_fd()))
        .expect("second description of the pipe");
    for chunk in [&[b'x'; 4096][..], b"x"] {
        while own.write(chunk).is_ok() {}
    }
}

/// A new pty, master first, that is nobody's controlling terminal.
pub fn pty() -> (File, File) {
    let master = OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(libc::O_NOCTTY)
        .open("/dev/ptmx")
        .expect("open /dev/ptmx");
    let (mut unlock, mut index) = (0, 0);
    assert!(crate::sys::ioctl(
        master.as_raw_fd(),
        libc::TIOCSPTLCK,
        &mut unlock
    ));
    assert!(crate::sys::ioctl(
        master.as_raw_fd(),
        libc::TIOCGPTN,
        &mut index
    ));
    let slave = OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(libc::O_NOCTTY)
        .open(format!("/dev/pts/{index}"))
        .expect("open the pty's slave");
    (master, slave)
}

/// Mode 000 stands in for a tty another account owns: su leaves one on
/// fd 2, and the account it switched to cannot reopen it through /proc.
/// Root can, so this fails loudly when run as root.
pub fn lock_out(slave: &File) {
    slave
        .set_permissions(Permissions::from_mode(0o000))
        .expect("chmod the pty's slave");
    let reopen = OpenOptions::new()
        .write(true)
        .open(format!("/proc/self/fd/{}", slave.as_raw_fd()));
    assert!(
        reopen.is_err(),
        "this process reopened a mode 000 tty, as root can, so it cannot stand in for su; run the tests as an ordinary user"
    );
}

/// What the master read until every slave was closed, within 5 seconds.
pub fn read_until_closed(mut master: File) -> String {
    let (done, finished) = mpsc::channel();
    std::thread::spawn(move || {
        let mut got = Vec::new();
        // the master reads EIO once the slave is closed and drained
        let _ = master.read_to_end(&mut got);
        let _ = done.send(got);
    });
    let got = finished
        .recv_timeout(Duration::from_secs(5))
        .expect("the pty's master read nothing within 5 seconds");
    String::from_utf8_lossy(&got).into_owned()
}
