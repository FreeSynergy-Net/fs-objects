//! The phases of a migration and the small state file that keeps them over a restart.
//!
//! Pattern: State (the enum) and Memento (the state file: a few `key=value` lines).

use std::fmt::Write;
use std::fs;
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
}

impl Saved {
    pub(super) fn new() -> Self {
        Self {
            phase: Phase::Checking,
            backup: None,
            report: None,
            v5_fingerprint: None,
            loss_confirmed: false,
        }
    }

    /// Read the state file, when there is one.
    pub(super) fn read(file: &Path) -> Result<Self> {
        let mut s = Self::new();
        if !file.is_file() {
            return Ok(s);
        }
        let text = fs::read_to_string(file).map_err(|e| super::io("read the state file", &e))?;
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
        if self.loss_confirmed {
            s.push_str("loss=confirmed\n");
        }
        fs::write(file, s).map_err(|e| super::io("write the state file", &e))
    }
}
