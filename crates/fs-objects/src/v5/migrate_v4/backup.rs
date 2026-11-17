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

fn copy_new(from: &Path, to: &Path) -> Result<()> {
    let bytes = fs::read(from).map_err(|e| io("read the v4 file", &e))?;
    let mut f = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(to)
        .map_err(|e| io("create the backup (never overwrites)", &e))?;
    f.write_all(&bytes)
        .and_then(|()| f.sync_all())
        .map_err(|e| io("write the backup", &e))?;
    if sha(from)? != sha(to)? {
        return Err(Error::Unavailable(
            "the backup differs from the v4 file".into(),
        ));
    }
    Ok(())
}

/// Copy the v4 file (and its `-wal` file when there is one) to `<file>.v4.bak` (numbered when taken) and verify it.
///
/// # Errors
/// `Unavailable` when the file cannot be read, the copy cannot be created, or the copy differs.
pub fn backup_file(v4: &Path) -> Result<PathBuf> {
    let to = unique_sibling(v4, ".v4.bak");
    copy_new(v4, &to)?;
    let wal = PathBuf::from(format!("{}-wal", v4.display()));
    if wal.exists() {
        copy_new(&wal, &PathBuf::from(format!("{}-wal", to.display())))?;
    }
    Ok(to)
}
