#![forbid(unsafe_code)]

use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::os::unix::process::ExitStatusExt;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

const CHILD: &str = "PANIC_EXIT_CHILD";

fn in_child() -> bool {
    if std::env::var_os(CHILD).is_none() {
        return false;
    }
    panic_exit::exit_on_panic();
    true
}

fn worker_panic() -> ! {
    let _ = std::thread::spawn(|| panic!("hunter2")).join();
    std::process::exit(0);
}

enum Stderr {
    Read,
    DevFull,
    ReaderlessPipe,
    StalledPipe,
}

struct Exit {
    signal: Option<i32>,
    code: Option<i32>,
    stderr: String,
}

fn spawn(test: &str, stdin: Stdio, stdout: Stdio, stderr: Stdio) -> Child {
    Command::new(std::env::current_exe().unwrap())
        .args(["--exact", test, "--test-threads=1", "--nocapture"])
        .env(CHILD, "1")
        .stdin(stdin)
        .stdout(stdout)
        .stderr(stderr)
        .spawn()
        .unwrap()
}

fn wait(mut child: Child) -> ExitStatus {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            return status;
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("the child did not exit within 10 seconds");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn run(test: &str, stderr: Stderr) -> Exit {
    let mut unread = None;
    let mut captured = None;
    let target: Stdio = match stderr {
        Stderr::Read => {
            let (reader, writer) = std::io::pipe().unwrap();
            captured = Some(reader);
            writer.into()
        }
        Stderr::DevFull => OpenOptions::new()
            .write(true)
            .open("/dev/full")
            .unwrap()
            .into(),
        Stderr::ReaderlessPipe => std::io::pipe().unwrap().1.into(),
        Stderr::StalledPipe => {
            let (reader, writer) = std::io::pipe().unwrap();
            vigil_log::testing::fill(&writer);
            unread = Some(reader);
            writer.into()
        }
    };
    let status = wait(spawn(test, Stdio::null(), Stdio::null(), target));
    drop(unread);
    let mut stderr = String::new();
    if let Some(mut reader) = captured {
        reader.read_to_string(&mut stderr).unwrap();
    }
    Exit {
        signal: status.signal(),
        code: status.code(),
        stderr,
    }
}

#[test]
fn a_worker_panic_exits_101_and_names_only_the_place() {
    if in_child() {
        worker_panic();
    }
    let exit = run(
        "a_worker_panic_exits_101_and_names_only_the_place",
        Stderr::Read,
    );
    assert_eq!(
        (exit.signal, exit.code),
        (None, Some(101)),
        "{}",
        exit.stderr
    );
    assert!(
        exit.stderr.lines().any(|line| line.starts_with("panic at ")
            && line.contains("exit_on_panic.rs:")
            && line.ends_with("; exiting")),
        "{}",
        exit.stderr
    );
    assert!(!exit.stderr.contains("hunter2"), "{}", exit.stderr);
}

#[test]
fn a_main_thread_panic_exits_101() {
    if in_child() {
        panic!("hunter2");
    }
    let exit = run("a_main_thread_panic_exits_101", Stderr::Read);
    assert_eq!(
        (exit.signal, exit.code),
        (None, Some(101)),
        "{}",
        exit.stderr
    );
    assert!(!exit.stderr.contains("hunter2"), "{}", exit.stderr);
}

#[test]
fn a_full_disk_on_stderr_still_exits_101() {
    if in_child() {
        worker_panic();
    }
    let exit = run("a_full_disk_on_stderr_still_exits_101", Stderr::DevFull);
    assert_eq!((exit.signal, exit.code), (None, Some(101)));
}

#[test]
fn a_stderr_pipe_with_no_reader_still_exits_101() {
    if in_child() {
        worker_panic();
    }
    let exit = run(
        "a_stderr_pipe_with_no_reader_still_exits_101",
        Stderr::ReaderlessPipe,
    );
    assert_eq!((exit.signal, exit.code), (None, Some(101)));
}

#[test]
fn a_stderr_pipe_nobody_drains_still_exits_101() {
    if in_child() {
        worker_panic();
    }
    let exit = run(
        "a_stderr_pipe_nobody_drains_still_exits_101",
        Stderr::StalledPipe,
    );
    assert_eq!((exit.signal, exit.code), (None, Some(101)));
}

// std::process::exit would flush the buffered half line into the stalled
// pipe and block there, so the child exits only if the hook skips the flush.
#[test]
fn buffered_stdout_on_a_stalled_pipe_does_not_hold_up_the_exit() {
    if in_child() {
        vigil_log::out!("ready");
        #[expect(
            clippy::disallowed_methods,
            reason = "the test needs a buffered half line"
        )]
        let _ = std::io::stdout().write_all(b"half a line");
        let _ = std::io::stdin().read(&mut [0]);
        worker_panic();
    }
    let (stdin, mut go) = std::io::pipe().unwrap();
    let (mut stdout, writer) = std::io::pipe().unwrap();
    let stall = writer.try_clone().unwrap();
    let child = spawn(
        "buffered_stdout_on_a_stalled_pipe_does_not_hold_up_the_exit",
        stdin.into(),
        writer.into(),
        Stdio::null(),
    );
    let mut seen = Vec::new();
    let mut byte = [0];
    while !seen.ends_with(b"ready\n") && stdout.read(&mut byte).unwrap() == 1 {
        seen.push(byte[0]);
    }
    assert!(
        seen.ends_with(b"ready\n"),
        "{}",
        String::from_utf8_lossy(&seen)
    );
    vigil_log::testing::fill(&stall);
    go.write_all(b"x").unwrap();
    let status = wait(child);
    assert_eq!((status.signal(), status.code()), (None, Some(101)));
    drop((stdout, stall));
}
