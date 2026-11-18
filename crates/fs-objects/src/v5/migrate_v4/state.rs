//! The phases of a migration and the small state file that keeps them over a restart.
//!
//! Pattern: State (the enum) and Memento (the state file: a few `key=value` lines, the last one `end=state`). The
//! file is written ATOMICALLY (temp file, sync, rename, sync of the folder); a file without `phase=` or without
//! the closing `end=state` line is an ERROR, never a phase.

use std::fmt::Write as _;
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use crate::v5::error::{Error, Result};

/// Where a migration is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// Nothing built yet.
    Checking,
    /// The new file is built and the report written; waiting for a person.
    Ready,
    /// A person confirmed.
    Confirmed,
    /// The new file is the active one.
    Switched,
    /// Back to v4 (or never switched): the v4 file is the active one for good.
    FellBack,
}

impl Phase {
    pub(super) fn word(self) -> &'static str {
        match self {
            Self::Checking => "checking",
            Self::Ready => "ready",
            Self::Confirmed => "confirmed",
            Self::Switched => "switched",
            Self::FellBack => "fellback",
        }
    }

    fn of(word: &str) -> Option<Self> {
        [
            Self::Checking,
            Self::Ready,
            Self::Confirmed,
            Self::Switched,
            Self::FellBack,
        ]
        .into_iter()
        .find(|p| p.word() == word)
    }
}

/// What the state file keeps.
#[derive(Debug, Clone)]
pub(super) struct Saved {
    pub phase: Phase,
    pub backup: Option<PathBuf>,
    pub report: Option<PathBuf>,
    /// The fingerprint of the v5 file at the moment of the switch (see `backup::fingerprint`).
    pub v5_fingerprint: Option<String>,
    /// A person confirmed that changes made in the v5 file after the switch are given up by the fall-back.
    pub loss_confirmed: bool,
    /// The fingerprint of the v4 file at the end of the check: `confirm` and `switch` compare with it.
    pub v4_fingerprint: Option<String>,
}

impl Saved {
    pub(super) fn new() -> Self {
        Self {
            phase: Phase::Checking,
            backup: None,
            report: None,
            v5_fingerprint: None,
            loss_confirmed: false,
            v4_fingerprint: None,
        }
    }

    /// Read the state file, when there is one.
    pub(super) fn read(file: &Path) -> Result<Self> {
        let mut s = Self::new();
        if !file.is_file() {
            return Ok(s);
        }
        let text = fs::read_to_string(file).map_err(|e| super::io("read the state file", &e))?;
        let bad = |why: &str| {
            Error::Unavailable(format!(
                "the state file {} is damaged ({why}): it is never read as a phase; a person looks at it",
                file.display()
            ))
        };
        if !text.lines().any(|l| l.starts_with("phase=")) {
            return Err(bad("no phase= line"));
        }
        if text.lines().last() != Some("end=state") || !text.ends_with('\n') {
            return Err(bad("incomplete: the closing end=state line is missing"));
        }
        for line in text.lines() {
            match line.split_once('=') {
                Some(("phase", w)) => {
                    s.phase = Phase::of(w).ok_or_else(|| {
                        Error::Unavailable("unknown phase in the state file".into())
                    })?;
                }
                Some(("backup", p)) => s.backup = Some(p.into()),
                Some(("report", p)) => s.report = Some(p.into()),
                Some(("v5fingerprint", f)) => s.v5_fingerprint = Some(f.to_owned()),
                Some(("v4fingerprint", f)) => s.v4_fingerprint = Some(f.to_owned()),
                Some(("loss", "confirmed")) => s.loss_confirmed = true,
                _ => {}
            }
        }
        Ok(s)
    }

    pub(super) fn write(&self, file: &Path) -> Result<()> {
        let mut s = format!("phase={}\n", self.phase.word());
        if let Some(b) = &self.backup {
            let _ = writeln!(s, "backup={}", b.display());
        }
        if let Some(r) = &self.report {
            let _ = writeln!(s, "report={}", r.display());
        }
        if let Some(f) = &self.v5_fingerprint {
            let _ = writeln!(s, "v5fingerprint={f}");
        }
        if let Some(f) = &self.v4_fingerprint {
            let _ = writeln!(s, "v4fingerprint={f}");
        }
        if self.loss_confirmed {
            s.push_str("loss=confirmed\n");
        }
        s.push_str("end=state\n");
        write_atomic(file, s.as_bytes()).map_err(|e| super::io("write the state file", &e))
    }
}

/// Temp file beside the target, `sync_all`, `rename` over the target, then `sync` of the folder: after a crash the
/// file is the old one or the new one, never half of it.
fn write_atomic(file: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = PathBuf::from(format!("{}.tmp", file.display()));
    let mut f = fs::File::create(&tmp)?;
    f.write_all(bytes)?;
    f.sync_all()?;
    drop(f);
    fs::rename(&tmp, file)?;
    // The folder sync makes the rename durable; not every platform can open a folder, then the rename stands alone.
    if let Some(dir) = file.parent().filter(|d| !d.as_os_str().is_empty())
        && let Ok(d) = fs::File::open(dir)
    {
        let _ = d.sync_all();
    }
    Ok(())
}
