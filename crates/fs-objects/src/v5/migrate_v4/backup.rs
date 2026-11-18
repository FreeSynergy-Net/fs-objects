//! The backup of the v4 file: never overwrites, numbers a name that is taken, checks the copy.
//!
//! Pattern: Transaction script (copy, then compare the SHA-256 of both files). Files are read in blocks, never whole.

use std::fmt::Write as _;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use super::keep::reader;

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

/// The size of one block read from a file: the memory a checksum needs, however large the file is.
pub(super) const BLOCK: usize = 64 * 1024;

/// Feed a reader into a hash in blocks of at most [`BLOCK`] bytes.
pub(super) fn feed(r: &mut impl Read, h: &mut Sha256) -> std::io::Result<()> {
    let mut buf = vec![0_u8; BLOCK];
    loop {
        match r.read(&mut buf) {
            Ok(0) => return Ok(()),
            Ok(n) => h.update(&buf[..n]),
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
}

/// `keep`: read through the never-closed descriptor (store files), see `keep.rs`.
fn feed_file(path: &Path, h: &mut Sha256, what: &str, keep: bool) -> Result<()> {
    let mut f = reader(path, keep).map_err(|e| io(what, &e))?;
    feed(&mut f, h).map_err(|e| io(what, &e))
}

fn sha(path: &Path, keep: bool) -> Result<Vec<u8>> {
    let mut h = Sha256::new();
    feed_file(path, &mut h, "read for the checksum", keep)?;
    Ok(h.finalize().to_vec())
}

/// The copy must have the same SHA-256 as the original (`keep`: the original is a store file).
fn verify_copy_of(from: &Path, to: &Path, keep: bool) -> Result<()> {
    if sha(from, keep)? == sha(to, false)? {
        Ok(())
    } else {
        Err(Error::Unavailable(
            "the backup differs from the v4 file".into(),
        ))
    }
}

/// Copy to a NEW file. A copy that fails after it was created is removed again (it is this run's own file).
fn copy_new(from: &Path, to: &Path, keep: bool) -> Result<()> {
    let mut src = reader(from, keep).map_err(|e| io("read the v4 file", &e))?;
    let mut f = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(to)
        .map_err(|e| io("create the backup (never overwrites)", &e))?;
    let done = std::io::copy(&mut src, &mut f)
        .map(|_| ())
        .and_then(|()| f.sync_all())
        .map_err(|e| io("write the backup", &e))
        .and_then(|()| verify_copy_of(from, to, keep));
    if done.is_err() {
        let _ = fs::remove_file(to);
    }
    done
}

/// The copy must have the same SHA-256 as the original.
pub(super) fn verify_copy(from: &Path, to: &Path) -> Result<()> {
    verify_copy_of(from, to, false)
}

/// SHA-256 (hex) over the file and, when there is one, its `-wal` file: what a person or a program changed in a
/// store file shows here, also while the change still sits in the `-wal` file.
///
/// # Errors
/// `Unavailable` when a file cannot be read.
pub fn fingerprint(path: &Path) -> Result<String> {
    let mut h = Sha256::new();
    feed_file(path, &mut h, "read for the fingerprint", true)?;
    let wal = PathBuf::from(format!("{}-wal", path.display()));
    if wal.is_file() {
        h.update(b"\0wal\0");
        feed_file(
            &wal,
            &mut h,
            "read the -wal file for the fingerprint",
            false,
        )?;
    }
    Ok(h.finalize().iter().fold(String::new(), |mut a, b| {
        let _ = write!(a, "{b:02x}");
        a
    }))
}

/// Copy the v4 file (and its `-wal` file when there is one) to `<file>.v4.bak` (numbered when taken) and verify it.
///
/// # Errors
/// `Unavailable` when the file cannot be read, the copy cannot be created, or the copy differs.
pub fn backup_file(v4: &Path) -> Result<PathBuf> {
    let to = unique_sibling(v4, ".v4.bak");
    copy_new(v4, &to, true)?;
    let wal = PathBuf::from(format!("{}-wal", v4.display()));
    if wal.exists()
        && let Err(e) = copy_new(&wal, &PathBuf::from(format!("{}-wal", to.display())), false)
    {
        let _ = fs::remove_file(&to);
        return Err(e);
    }
    Ok(to)
}

#[cfg(test)]
#[path = "backup_tests.rs"]
mod tests;
