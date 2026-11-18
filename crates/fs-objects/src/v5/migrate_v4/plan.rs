//! The switch: a state machine that decides which file is active. The v4 file stays the active one until a person
//! has confirmed; every step is written to a small state file, so a restart does not lose it.
//!
//! Pattern: State machine (Checking, Ready, Confirmed, Switched, `FellBack`); the transitions are the only way to
//! change [`MigrationPlan::active_path`]. States: `Checking` (nothing built yet), `Ready` (the new file is built,
//! the report is written, a person has to look at it), `Confirmed` (a person said yes), `Switched` (the new file
//! is the active one), `FellBack` (the v4 file stays; the new file is left for a person to delete).

use std::fs;
use std::path::{Path, PathBuf};

use super::backup::{backup_file, fingerprint, unique_sibling};
use super::build::build;
use super::convert::Converter;
use super::lock::SourceLock;
use super::report::Report;
use super::state::Saved;
use crate::v5::error::{Error, Result};
use crate::{OpenOptions, TursoStore};

pub use super::state::Phase;

/// One migration of one v4 file.
#[derive(Debug)]
pub struct MigrationPlan {
    pub(super) v4: PathBuf,
    pub(super) v5: PathBuf,
    state: PathBuf,
    pub(super) saved: Saved,
    report: Option<Report>,
    abort_note: Option<String>,
}

fn wrong(from: Phase, to: &str) -> Error {
    Error::Unavailable(format!("cannot {to} from the state {}", from.word()))
}

/// The files a check run makes beside the v5 file (the engine's own side files).
const V5_SIDE: [&str; 3] = ["", "-wal", "-shm"];

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
        let state = name(".migration-state");
        Ok(Self {
            v5: name(".v5"),
            saved: Saved::read(&state)?,
            state,
            v4,
            report: None,
            abort_note: None,
        })
    }

    pub(super) fn go(&mut self, phase: Phase) -> Result<()> {
        let mut next = self.saved.clone();
        next.phase = phase;
        next.write(&self.state)?;
        self.saved = next;
        Ok(())
    }

    fn v5_side(&self) -> Vec<PathBuf> {
        V5_SIDE
            .iter()
            .map(|s| PathBuf::from(format!("{}{s}", self.v5.display())))
            .collect()
    }

    /// Lock, check the source, back up, build the new file, round-trip every object, write the report: `Checking`
    /// to `Ready`. The v4 file is only READ (opened read-only, after the program is shown to be closed by an
    /// exclusive lock and the schema version is shown to be 4). Nothing is converted without this call, and
    /// nothing is switched by it. A run that fails removes what IT made (the half-built new file, the backup) and
    /// says so in the error and in [`MigrationPlan::abort_note`]; files that were there before are never touched.
    ///
    /// # Errors
    /// `Unavailable` when the state is not `Checking`, a result file already exists, the program still holds the
    /// v4 file, the file is no v4 store (another schema version, empty, foreign), the source changed during the
    /// run, or a file operation fails.
    pub async fn check(
        &mut self,
        conv: &dyn Converter,
        v4_options: OpenOptions,
        now_ms: i64,
    ) -> Result<&Report> {
        if self.saved.phase != Phase::Checking {
            return Err(wrong(self.saved.phase, "check"));
        }
        if let Some(p) = self.v5_side().iter().find(|p| p.exists()) {
            return Err(Error::Unavailable(format!(
                "{} already exists: it is never overwritten; a person removes it",
                p.display()
            )));
        }
        let _lock = SourceLock::take(&self.v4)?;
        let before = fingerprint(&self.v4)?;
        let v4 = TursoStore::open_read_only(&self.v4, v4_options)
            .await
            .map_err(|e| Error::Unavailable(format!("{}: {e}", self.v4.display())))?;
        let backup = backup_file(&self.v4)?;
        match self.run(conv, &v4, &backup, &before, now_ms).await {
            Ok(report) => {
                self.saved.backup = Some(backup);
                self.saved.v4_fingerprint = Some(before);
                self.go(Phase::Ready)?;
                Ok(self.report.insert(report))
            }
            Err(e) => {
                drop(v4);
                let note = self.clean_up(&backup);
                self.abort_note = Some(note.clone());
                Err(Error::Unavailable(format!("{e}; {note}")))
            }
        }
    }

    async fn run(
        &mut self,
        conv: &dyn Converter,
        v4: &TursoStore,
        backup: &Path,
        before: &str,
        now_ms: i64,
    ) -> Result<Report> {
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
        build(v4, &target, conv, now_ms, &mut report).await?;
        if fingerprint(&self.v4)? != before {
            return Err(Error::Unavailable(
                "the v4 file changed while the check ran".into(),
            ));
        }
        let file = unique_sibling(&self.v4, ".migration-report.txt");
        fs::write(&file, report.render()).map_err(|e| super::io("write the report", &e))?;
        self.saved.report = Some(file);
        Ok(report)
    }

    /// Remove what this run made: the half-built new file with its side files and the backup. Returns the sentence
    /// for the person (the files this run made are named; the v4 file is untouched).
    fn clean_up(&mut self, backup: &Path) -> String {
        let mut gone = Vec::new();
        let wal_of_backup = PathBuf::from(format!("{}-wal", backup.display()));
        let mut mine = self.v5_side();
        mine.push(backup.to_path_buf());
        mine.push(wal_of_backup);
        for p in mine {
            if p.exists() && fs::remove_file(&p).is_ok() {
                gone.push(p.display().to_string());
            }
        }
        if let Some(r) = self.saved.report.take()
            && fs::remove_file(&r).is_ok()
        {
            gone.push(r.display().to_string());
        }
        format!(
            "the check was aborted, the v4 file {} was not changed; removed what this run had made: [{}]; nothing was switched",
            self.v4.display(),
            gone.join(", ")
        )
    }

    /// A person confirms after reading the report: `Ready` to `Confirmed`.
    ///
    /// # Errors
    /// `Unavailable` when the state is not `Ready`, or the v4 file changed since the check.
    pub fn confirm(&mut self) -> Result<()> {
        if self.saved.phase != Phase::Ready {
            return Err(wrong(self.saved.phase, "confirm"));
        }
        self.v4_unchanged("confirm")?;
        self.go(Phase::Confirmed)
    }

    /// Make the new file the active one: only from `Confirmed`. Returns its path. The fingerprint of the new file
    /// is kept, so that a later fall-back can tell whether the new file was changed since.
    ///
    /// # Errors
    /// `Unavailable` when no person has confirmed, or the v4 file changed since the check.
    pub fn switch(&mut self) -> Result<&Path> {
        if self.saved.phase != Phase::Confirmed {
            return Err(wrong(self.saved.phase, "switch"));
        }
        self.v4_unchanged("switch")?;
        self.saved.v5_fingerprint = Some(fingerprint(&self.v5)?);
        self.go(Phase::Switched)?;
        Ok(&self.v5)
    }

    /// Stay with (or return to) the v4 file for good. The v4 file was never changed, so before the switch nothing
    /// is lost. FROM `Switched` the new file may hold changes that the v4 file does not have: then the fall-back is
    /// REFUSED, unless `confirm_loss` is true (a person decided to give those changes up; the state file says so).
    /// The new file is left where it is for a person to delete.
    ///
    /// # Errors
    /// `Unavailable` when the new file changed since the switch and `confirm_loss` is false, or the state file
    /// cannot be written.
    pub fn fall_back(&mut self, confirm_loss: bool) -> Result<()> {
        if self.saved.phase == Phase::Switched && self.v5_changed()? {
            if !confirm_loss {
                return Err(Error::Unavailable(format!(
                    "{} was changed (or may have been) since the switch: those changes are NOT in the v4 file and \
                     would be lost. Keep the new file, or confirm the loss explicitly (confirm_loss)",
                    self.v5.display()
                )));
            }
            self.saved.loss_confirmed = true;
        }
        self.go(Phase::FellBack)
    }

    /// The file a program must open now: the v4 file until the switch.
    #[must_use]
    pub fn active_path(&self) -> &Path {
        if self.saved.phase == Phase::Switched {
            &self.v5
        } else {
            &self.v4
        }
    }

    /// Where the migration is.
    #[must_use]
    pub fn phase(&self) -> Phase {
        self.saved.phase
    }

    /// The report of this run (not after a restart: read the report file).
    #[must_use]
    pub fn report(&self) -> Option<&Report> {
        self.report.as_ref()
    }

    /// The report file, once written.
    #[must_use]
    pub fn report_file(&self) -> Option<&Path> {
        self.saved.report.as_deref()
    }

    /// The backup of the v4 file, once made and kept.
    #[must_use]
    pub fn backup(&self) -> Option<&Path> {
        self.saved.backup.as_deref()
    }

    /// What the last aborted check run removed (the visible message of the clean-up), if one was aborted.
    #[must_use]
    pub fn abort_note(&self) -> Option<&str> {
        self.abort_note.as_deref()
    }
}
