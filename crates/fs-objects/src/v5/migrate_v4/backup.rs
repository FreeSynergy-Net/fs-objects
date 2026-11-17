//! The backup of the v4 file: never overwrites, numbers a name that is taken, checks the copy.
//!
//! Pattern: Transaction script (copy, then compare the SHA-256 of both files).

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::v5::error::{Error, Result};

fn io(what: &str, e: &std::io::Error) -> Error {
    Error::Unavailable(format!("{what}: {e}"))
}

/// `<path><suffix>`, or `<path><suffix>.1`, `.2`, ... for the first name that does not exist yet.
#[must_use]
pub fn unique_sibling(path: &Path, suffix: &str) -> PathBuf {
    let base = format!("{}{suffix}", path.display());
    let mut cand = PathBuf::from(&base);
    let mut n = 0_u32;
    while cand.exists() {
        n += 1;
        cand = PathBuf::from(format!("{base}.{n}"));
    }
    cand
}

fn sha(path: &Path) -> Result<Vec<u8>> {
    let bytes = fs::read(path).map_err(|e| io("read for the checksum", &e))?;
    Ok(Sha256::digest(&bytes).to_vec())
}

/// Copy to a NEW file. A copy that fails after it was created is removed again (it is this run's own file).
fn copy_new(from: &Path, to: &Path) -> Result<()> {
    let bytes = fs::read(from).map_err(|e| io("read the v4 file", &e))?;
    let mut f = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(to)
        .map_err(|e| io("create the backup (never overwrites)", &e))?;
    let done = f
        .write_all(&bytes)
        .and_then(|()| f.sync_all())
        .map_err(|e| io("write the backup", &e))
        .and_then(|()| {
            if sha(from)? == sha(to)? {
                Ok(())
            } else {
                Err(Error::Unavailable(
                    "the backup differs from the v4 file".into(),
                ))
            }
        });
    if done.is_err() {
        let _ = fs::remove_file(to);
    }
    done
}

/// SHA-256 (hex) over the file and, when there is one, its `-wal` file: what a person or a program changed in a
/// store file shows here, also while the change still sits in the `-wal` file.
///
/// # Errors
/// `Unavailable` when a file cannot be read.
pub fn fingerprint(path: &Path) -> Result<String> {
    let mut h = Sha256::new();
    h.update(fs::read(path).map_err(|e| io("read for the fingerprint", &e))?);
    let wal = PathBuf::from(format!("{}-wal", path.display()));
    if wal.is_file() {
        h.update(b"\0wal\0");
        h.update(fs::read(&wal).map_err(|e| io("read the -wal file for the fingerprint", &e))?);
    }
    Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

/// Copy the v4 file (and its `-wal` file when there is one) to `<file>.v4.bak` (numbered when taken) and verify it.
///
/// # Errors
/// `Unavailable` when the file cannot be read, the copy cannot be created, or the copy differs.
pub fn backup_file(v4: &Path) -> Result<PathBuf> {
    let to = unique_sibling(v4, ".v4.bak");
    copy_new(v4, &to)?;
    let wal = PathBuf::from(format!("{}-wal", v4.display()));
    if wal.exists()
        && let Err(e) = copy_new(&wal, &PathBuf::from(format!("{}-wal", to.display())))
    {
        let _ = fs::remove_file(&to);
        return Err(e);
    }
    Ok(to)
}
