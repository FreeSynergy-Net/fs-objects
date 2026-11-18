//! The two comparisons that keep the switch from losing data: the v4 file against the fingerprint of the check (before
//! `confirm` and `switch`), the new file against the fingerprint of the switch (before the fall-back).
//!
//! Pattern: Guard (precondition checks of the state machine's transitions).

use super::backup::fingerprint;
use super::plan::MigrationPlan;
use super::state::Phase;
use crate::v5::error::{Error, Result};

impl MigrationPlan {
    /// The v4 file stays the active one (and the program keeps writing to it) until the switch: what was written
    /// after the check is NOT in the new file. So `confirm` and `switch` compare the v4 file with the fingerprint of
    /// the check; when it differs (or is unknown), the step is REFUSED, the state goes back to `Checking`, and a
    /// new check is needed (the person removes the stale new file first; it is never overwritten).
    pub(super) fn v4_unchanged(&mut self, step: &str) -> Result<()> {
        let now = fingerprint(&self.v4)?;
        if self.saved.v4_fingerprint.as_deref() == Some(now.as_str()) {
            return Ok(());
        }
        let stale = self.v5.display().to_string();
        self.saved.v4_fingerprint = None;
        self.saved.backup = None;
        self.saved.report = None;
        self.go(Phase::Checking)?;
        Err(Error::Unavailable(format!(
            "cannot {step}: the v4 file {} changed since the check (or the check did not record it), so the new file {stale} \
             does not have everything. Nothing was switched. Remove the stale new file (and look at the old backup and \
             report beside the v4 file), then run a new check",
            self.v4.display()
        )))
    }

    /// Whether the new file differs from the moment of the switch (or the moment is unknown): then going back to
    /// v4 would give up what was written since.
    pub(super) fn v5_changed(&self) -> Result<bool> {
        if !self.v5.is_file() {
            return Ok(false);
        }
        Ok(self.saved.v5_fingerprint.as_deref() != Some(fingerprint(&self.v5)?.as_str()))
    }
}
