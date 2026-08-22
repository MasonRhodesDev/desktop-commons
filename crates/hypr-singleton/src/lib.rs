//! flock(2)-based single-instance guard.
//!
//! Ownership is an exclusive lock on a file under `$XDG_RUNTIME_DIR`. The
//! kernel releases the lock on any process death — SIGKILL and
//! `std::process::exit` paths that skip `Drop` included — so a stale lock
//! file can never wedge future instances; only a *live* owner blocks
//! acquisition. The file is deliberately never unlinked: unlinking would let
//! a racing opener lock a fresh inode under the same path, and two owners
//! would then coexist.
//!
//! First consumer: vigil-lock, which pairs this with a Unix-socket join RPC
//! (MasonRhodesDev/vigil#50 — concurrent lockers used to stack).

use std::fs::File;
use std::io;
use std::os::fd::AsRawFd;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

/// Held for as long as the process is the single instance. Dropping it (or
/// dying) releases ownership.
#[derive(Debug)]
pub struct SingletonGuard {
    _file: File,
    path: PathBuf,
}

impl SingletonGuard {
    /// The lock file backing this guard.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

#[derive(Debug)]
pub enum Error {
    /// `$XDG_RUNTIME_DIR` missing, empty, or not absolute.
    RuntimeDir(hypr_paths::Error),
    /// Opening or locking the lock file failed for a reason other than a
    /// live owner.
    Io(PathBuf, io::Error),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::RuntimeDir(e) => write!(f, "singleton lock: {e}"),
            Error::Io(path, e) => write!(f, "singleton lock {}: {e}", path.display()),
        }
    }
}

impl std::error::Error for Error {}

/// Try to become the single instance named `name`, backed by
/// `$XDG_RUNTIME_DIR/<name>.lock`.
///
/// - `Ok(Some(guard))` — this process owns the name; keep the guard alive
///   for the lifetime of the process.
/// - `Ok(None)` — another live process owns it.
pub fn try_acquire(name: &str) -> Result<Option<SingletonGuard>, Error> {
    let dirs = hypr_paths::BaseDirs::from_env().map_err(Error::RuntimeDir)?;
    try_acquire_at(&dirs.runtime_dir().join(format!("{name}.lock")))
}

/// [`try_acquire`] with an explicit path (test seam; production code should
/// key on the runtime dir via [`try_acquire`]).
pub fn try_acquire_at(path: &Path) -> Result<Option<SingletonGuard>, Error> {
    let file = File::options()
        .create(true)
        .write(true)
        .truncate(false)
        .mode(0o600)
        .open(path)
        .map_err(|e| Error::Io(path.to_owned(), e))?;
    // LOCK_NB: a held lock means a live owner — report it, never wait.
    match unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } {
        0 => Ok(Some(SingletonGuard {
            _file: file,
            path: path.to_owned(),
        })),
        _ => {
            let err = io::Error::last_os_error();
            if err.kind() == io::ErrorKind::WouldBlock {
                Ok(None)
            } else {
                Err(Error::Io(path.to_owned(), err))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_lock_path(tag: &str) -> PathBuf {
        let mut path = std::env::temp_dir();
        path.push(format!("hypr-singleton-test-{tag}-{}", std::process::id()));
        path
    }

    #[test]
    fn second_acquire_is_refused_while_owner_lives() {
        let path = temp_lock_path("contention");
        let owner = try_acquire_at(&path).unwrap();
        assert!(owner.is_some());
        // A separate open file description contends even within one process.
        assert!(try_acquire_at(&path).unwrap().is_none());
        drop(owner);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn dropping_the_guard_releases_ownership() {
        let path = temp_lock_path("release");
        drop(try_acquire_at(&path).unwrap());
        assert!(try_acquire_at(&path).unwrap().is_some());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn guard_reports_its_path() {
        let path = temp_lock_path("path");
        let guard = try_acquire_at(&path).unwrap().unwrap();
        assert_eq!(guard.path(), path);
        let _ = std::fs::remove_file(&path);
    }
}
