//! The program must be closed while the v4 file is migrated: two advisory locks on the v4 file for as long as the
//! check runs. (1) the POSIX `fcntl` lock (`F_SETLK`), the kind the engine (turso 0.8.3) takes on a file it opens:
//! a running program (an open `TursoStore`) holds it in ITS process, so the check stops with a hint, and a program
//! that starts WHILE the check runs cannot open the file either (the engine cannot take its write lock). (2)
//! `flock`, for tools that use that kind. On Linux the two kinds do not see each other, so both are taken.
//!
//! A `fcntl` lock belongs to a PROCESS and ends when ANY descriptor of the file is closed. So the locks are taken on
//! a descriptor that is never closed (see `keep.rs`), every other read of the store files uses that descriptor too
//! (fingerprint, backup), and the release is an explicit unlock, never a close. A program in the SAME process is
//! still not seen by the lock (the process owns both): the program must be closed before the check, in every case,
//! and the unlock at the end also ends a lock the program's own process might hold. Where there is no `fcntl`
//! (not Unix) only `flock` is taken.
//!
//! Pattern: Resource guard (RAII: the locks end when the guard is dropped).

use std::fs::{File, TryLockError};
use std::path::Path;
use std::sync::Arc;

use super::keep::kept;
use crate::v5::error::{Error, Result};

/// The locks on the v4 file.
#[derive(Debug)]
pub struct SourceLock {
    file: Arc<File>,
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

#[cfg(unix)]
fn posix_unlock(file: &File) {
    let _ = rustix::fs::fcntl_lock(file, rustix::fs::FlockOperation::NonBlockingUnlock);
}

#[cfg(not(unix))]
fn posix_unlock(_file: &File) {}

impl SourceLock {
    /// Take the locks without waiting.
    ///
    /// # Errors
    /// `Unavailable` when another handle or process holds a lock (the program is still running) or the file
    /// cannot be opened.
    pub fn take(path: &Path) -> Result<Self> {
        let file = kept(path).map_err(|e| super::io("open the v4 file for the lock", &e))?;
        posix(&file, path)?;
        match file.try_lock() {
            Ok(()) => Ok(Self { file }),
            Err(e) => {
                posix_unlock(&file);
                Err(match e {
                    TryLockError::WouldBlock => held(path),
                    TryLockError::Error(e) => super::io("lock the v4 file", &e),
                })
            }
        }
    }
}

impl Drop for SourceLock {
    /// Unlock; the descriptor stays open (closing it would drop locks of other users of the file in this process).
    fn drop(&mut self) {
        let _ = self.file.unlock();
        posix_unlock(&self.file);
    }
}
