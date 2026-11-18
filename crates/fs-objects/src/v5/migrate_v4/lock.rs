//! The program must be closed while the v4 file is migrated: two advisory locks on the v4 file for as long as the
//! check runs. (1) the POSIX `fcntl` lock (`F_SETLK`), the kind the engine (turso 0.8.3) takes on a file it opens:
//! a running program (an open `TursoStore`) holds it in ITS process, so the check stops with a hint. (2) `flock`,
//! for tools that use that kind. On Linux the two kinds do not see each other, so both are taken. The `fcntl`
//! lock belongs to a PROCESS: a program in the SAME process is not seen by it (the program is the caller then).
//! A shared `fcntl` lock is taken (the file is opened read-only); it also keeps a program from opening the file
//! for writing while the check runs. Where there is no `fcntl` (not Unix) only `flock` is taken.
//!
//! Pattern: Resource guard (RAII: the locks end when the guard is dropped).

use std::fs::{File, TryLockError};
use std::path::Path;

use crate::v5::error::{Error, Result};

/// The locks on the v4 file.
#[derive(Debug)]
pub struct SourceLock {
    _file: File,
}

fn held(path: &Path) -> Error {
    Error::Unavailable(format!(
        "{} is locked by another handle: close the program that uses it, then run the check again",
        path.display()
    ))
}

#[cfg(unix)]
fn posix(file: &File, path: &Path) -> Result<()> {
    use rustix::fs::{FlockOperation, fcntl_lock};
    match fcntl_lock(file, FlockOperation::NonBlockingLockShared) {
        Ok(()) => Ok(()),
        Err(e) if e == rustix::io::Errno::AGAIN || e == rustix::io::Errno::ACCESS => {
            Err(held(path))
        }
        Err(e) => Err(super::io(
            "lock the v4 file (fcntl)",
            &std::io::Error::from(e),
        )),
    }
}

#[cfg(not(unix))]
fn posix(_file: &File, _path: &Path) -> Result<()> {
    Ok(())
}

impl SourceLock {
    /// Take the locks without waiting.
    ///
    /// # Errors
    /// `Unavailable` when another handle or process holds a lock (the program is still running) or the file
    /// cannot be opened.
    pub fn take(path: &Path) -> Result<Self> {
        let file = File::open(path).map_err(|e| super::io("open the v4 file for the lock", &e))?;
        posix(&file, path)?;
        match file.try_lock() {
            Ok(()) => Ok(Self { _file: file }),
            Err(TryLockError::WouldBlock) => Err(held(path)),
            Err(TryLockError::Error(e)) => Err(super::io("lock the v4 file", &e)),
        }
    }
}
