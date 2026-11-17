//! The switch: a state machine that decides which file is active. The v4 file stays the active one until a person
//! has confirmed; every step is written to a small state file, so a restart does not lose it.
//!
//! Pattern: State machine (Checking, Ready, Confirmed, Switched, `FellBack`); the transitions are the only way to
//! change [`MigrationPlan::active_path`]. States: `Checking` (nothing built yet), `Ready` (the new file is built,
//! the report is written, a person has to look at it), `Confirmed` (a person said yes), `Switched` (the new file
//! is the active one), `FellBack` (the v4 file stays; the new file is left for a person to delete).

use std::fmt::Write;
use std::fs;
use std::path::{Path, PathBuf};

use super::backup::{backup_file, unique_sibling};
use super::build::build;
use super::convert::Converter;
use super::report::Report;
use crate::v5::error::{Error, Result};
use crate::{OpenOptions, TursoStore};

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
    fn word(self) -> &'static str {
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

/// One migration of one v4 file.
#[derive(Debug)]
pub struct MigrationPlan {
    v4: PathBuf,
    v5: PathBuf,
    state: PathBuf,
    backup: Option<PathBuf>,
    report_file: Option<PathBuf>,
    phase: Phase,
    report: Option<Report>,
}

fn io(what: &str, e: &std::io::Error) -> Error {
    Error::Unavailable(format!("{what}: {e}"))
}

fn wrong(from: Phase, to: &str) -> Error {
    Error::Unavailable(format!("cannot {to} from the state {}", from.word()))
}

impl MigrationPlan {
    /// The plan for a v4 file; continues from the state file when a migration was started before.
    ///
    /// # Errors
    /// `Unavailable` when the file does not exist or the state file is unreadable.
    pub fn open(v4: impl Into<PathBuf>) -> Result<Self> {
        let v4: PathBuf = v4.into();
        if !v4.is_file() {
            return Err(Error::Unavailable(format!(
                "no v4 file at {}",
                v4.display()
            )));
        }
        let name = |s: &str| PathBuf::from(format!("{}{s}", v4.display()));
        let mut plan = Self {
            v5: name(".v5"),
            state: name(".migration-state"),
            v4,
            backup: None,
            report_file: None,
            phase: Phase::Checking,
            report: None,
        };
        if plan.state.is_file() {
            let text =
                fs::read_to_string(&plan.state).map_err(|e| io("read the state file", &e))?;
            for line in text.lines() {
                match line.split_once('=') {
                    Some(("phase", w)) => {
                        plan.phase = Phase::of(w).ok_or_else(|| {
                            Error::Unavailable("unknown phase in the state file".into())
                        })?;
                    }
                    Some(("backup", p)) => plan.backup = Some(p.into()),
                    Some(("report", p)) => plan.report_file = Some(p.into()),
                    _ => {}
                }
            }
        }
        Ok(plan)
    }

    fn persist(&self, phase: Phase) -> Result<()> {
        let mut s = format!("phase={}\n", phase.word());
        if let Some(b) = &self.backup {
            let _ = writeln!(s, "backup={}", b.display());
        }
        if let Some(r) = &self.report_file {
            let _ = writeln!(s, "report={}", r.display());
        }
        fs::write(&self.state, s).map_err(|e| io("write the state file", &e))
    }

    fn go(&mut self, phase: Phase) -> Result<()> {
        self.persist(phase)?;
        self.phase = phase;
        Ok(())
    }

    /// Back up, build the new file, round-trip every object, write the report: `Checking` to `Ready`. The v4 file
    /// is not changed. Nothing is converted without this call, and nothing is switched by it.
    ///
    /// # Errors
    /// `Unavailable` when the state is not `Checking`, the new file already exists, or a file operation fails.
    pub async fn check(
        &mut self,
        conv: &dyn Converter,
        v4_options: OpenOptions,
        now_ms: i64,
    ) -> Result<&Report> {
        if self.phase != Phase::Checking {
            return Err(wrong(self.phase, "check"));
        }
        if self.v5.exists() {
            return Err(Error::Unavailable(format!(
                "{} already exists: it is never overwritten; a person removes it",
                self.v5.display()
            )));
        }
        let backup = backup_file(&self.v4)?;
        self.backup = Some(backup.clone());
        let v4 = TursoStore::open_with(&self.v4, v4_options)
            .await
            .map_err(|e| Error::Unavailable(e.to_string()))?;
        let mut report = Report {
            source: self.v4.display().to_string(),
            backup: backup.display().to_string(),
            target: self.v5.display().to_string(),
            ..Report::default()
        };
        let target = self
            .v5
            .to_str()
            .ok_or_else(|| Error::Unavailable("path is not UTF-8".into()))?
            .to_owned();
        build(&v4, &target, conv, now_ms, &mut report).await?;
        let file = unique_sibling(&self.v4, ".migration-report.txt");
        fs::write(&file, report.render()).map_err(|e| io("write the report", &e))?;
        self.report_file = Some(file);
        self.go(Phase::Ready)?;
        Ok(self.report.insert(report))
    }

    /// A person confirms after reading the report: `Ready` to `Confirmed`.
    ///
    /// # Errors
    /// `Unavailable` when the state is not `Ready`.
    pub fn confirm(&mut self) -> Result<()> {
        if self.phase != Phase::Ready {
            return Err(wrong(self.phase, "confirm"));
        }
        self.go(Phase::Confirmed)
    }

    /// Make the new file the active one: only from `Confirmed`. Returns its path.
    ///
    /// # Errors
    /// `Unavailable` when no person has confirmed.
    pub fn switch(&mut self) -> Result<&Path> {
        if self.phase != Phase::Confirmed {
            return Err(wrong(self.phase, "switch"));
        }
        self.go(Phase::Switched)?;
        Ok(&self.v5)
    }

    /// Stay with (or return to) the v4 file for good. The v4 file was never changed, so nothing is lost; the new
    /// file is left where it is for a person to delete.
    ///
    /// # Errors
    /// `Unavailable` when the state file cannot be written.
    pub fn fall_back(&mut self) -> Result<()> {
        self.go(Phase::FellBack)
    }

    /// The file a program must open now: the v4 file until the switch.
    #[must_use]
    pub fn active_path(&self) -> &Path {
        if self.phase == Phase::Switched {
            &self.v5
        } else {
            &self.v4
        }
    }

    /// Where the migration is.
    #[must_use]
    pub fn phase(&self) -> Phase {
        self.phase
    }

    /// The report of this run (not after a restart: read the report file).
    #[must_use]
    pub fn report(&self) -> Option<&Report> {
        self.report.as_ref()
    }

    /// The report file, once written.
    #[must_use]
    pub fn report_file(&self) -> Option<&Path> {
        self.report_file.as_deref()
    }

    /// The backup of the v4 file, once made.
    #[must_use]
    pub fn backup(&self) -> Option<&Path> {
        self.backup.as_deref()
    }
}
