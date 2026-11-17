//! The program must be closed while the v4 file is migrated: an exclusive advisory lock (`flock`) on the v4 file
//! for as long as the check runs. A program that has the file open (and holds the same kind of lock) makes the
//! check stop with a hint; nothing is read or copied while somebody may still write.
//!
//! Pattern: Resource guard (RAII: the lock ends when the guard is dropped).

use std::fs::{File, TryLockError};
use std::path::Path;

use crate::v5::error::{Error, Result};

/// The exclusive lock on the v4 file.
#[derive(Debug)]
pub struct SourceLock {
    _file: File,
}

impl SourceLock {
    /// Take the lock without waiting.
    ///
    /// # Errors
    /// `Unavailable` when another handle holds a lock (the program is still running) or the file cannot be opened.
    pub fn take(path: &Path) -> Result<Self> {
        let file = File::open(path).map_err(|e| super::io("open the v4 file for the lock", &e))?;
        match file.try_lock() {
            Ok(()) => Ok(Self { _file: file }),
            Err(TryLockError::WouldBlock) => Err(Error::Unavailable(format!(
                "{} is locked by another handle: close the program that uses it, then run the check again",
                path.display()
            ))),
            Err(TryLockError::Error(e)) => Err(super::io("lock the v4 file", &e)),
        }
    }
}
