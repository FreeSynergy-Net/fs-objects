//! Files that are read but whose descriptor is NEVER closed. A POSIX `fcntl` lock (the kind the engine takes on a
//! file it opens) belongs to the PROCESS and is dropped when ANY descriptor of that file is closed, so opening a
//! store file only to read it and closing it again silently removes the lock of a program in the same process
//! (or the migration's own lock). The store files (v4, v5) are therefore read through one descriptor per file that
//! stays open until the process ends, and read with `pread`, which needs no position and no second descriptor. A
//! file that was replaced (another inode under the same name) gets a new descriptor; the old one is kept too.
//! Where there is no `fcntl` (not Unix) a file is simply opened and closed.
//!
//! Pattern: Registry (one shared, never-released handle per file; the cost is a few open descriptors).

use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

static KEPT: Mutex<Vec<(PathBuf, Arc<File>)>> = Mutex::new(Vec::new());

#[cfg(unix)]
fn same(file: &File, path: &Path) -> bool {
    use std::os::unix::fs::MetadataExt as _;
    match (file.metadata(), std::fs::metadata(path)) {
        (Ok(a), Ok(b)) => a.dev() == b.dev() && a.ino() == b.ino(),
        _ => false,
    }
}

/// The never-closed descriptor of `path` (opened read-only the first time, or when the file was replaced).
///
/// # Errors
/// The file cannot be opened.
pub(super) fn kept(path: &Path) -> std::io::Result<Arc<File>> {
    #[cfg(unix)]
    {
        let mut all = KEPT
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some((_, f)) = all.iter().rev().find(|(p, f)| p == path && same(f, path)) {
            return Ok(Arc::clone(f));
        }
        let f = Arc::new(File::open(path)?);
        all.push((path.to_path_buf(), Arc::clone(&f)));
        Ok(f)
    }
    #[cfg(not(unix))]
    {
        let _ = &KEPT;
        Ok(Arc::new(File::open(path)?))
    }
}

/// Reads a kept file from its start with `pread` (no shared position).
pub(super) struct KeptReader {
    file: Arc<File>,
    pos: u64,
}

impl Read for KeptReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        #[cfg(unix)]
        let n = std::os::unix::fs::FileExt::read_at(&*self.file, buf, self.pos)?;
        #[cfg(not(unix))]
        let n = (&*self.file).read(buf)?;
        self.pos += n as u64;
        Ok(n)
    }
}

/// A reader over the file: a kept descriptor when `keep`, else an ordinary open that closes at the end.
///
/// # Errors
/// The file cannot be opened.
pub(super) fn reader(path: &Path, keep: bool) -> std::io::Result<Box<dyn Read>> {
    if keep {
        Ok(Box::new(KeptReader {
            file: kept(path)?,
            pos: 0,
        }))
    } else {
        Ok(Box::new(File::open(path)?))
    }
}
