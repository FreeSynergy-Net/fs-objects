//! [`UndoRedo`]: per-actor persistent undo and redo.
//!
//! Pattern: Command — undo/redo apply the inverse of recorded activities.

use async_trait::async_trait;

use crate::{Activity, ActivityId, By, Error, Outcome, Result};

/// Undo/redo depth for one actor.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct UndoState {
    /// Activities `undo` could revert.
    pub undoable: usize,
    /// Undone activities `redo` could restore.
    pub redoable: usize,
}

/// Undo and redo of an actor's changes.
#[async_trait]
pub trait UndoRedo: Send + Sync {
    /// Undo the last `n` undoable activities of `actor` (most recent first), atomically.
    /// Returns the new `Undo` activities. Fewer than `n` may be possible: returns what was done.
    /// Fails with `Conflict` (and changes nothing) if the object was changed since.
    async fn undo(&self, actor: &str, n: usize) -> Result<Vec<Activity>>;

    /// Redo the last `n` undone activities of `actor` (an `Undo` of an `Undo`).
    async fn redo(&self, actor: &str, n: usize) -> Result<Vec<Activity>>;

    /// How many undo/redo steps `actor` has.
    async fn undo_state(&self, actor: &str) -> Result<UndoState>;

    /// Pattern: Command – take back the whole unit of `change` (any of its activities names it) as one new unit written
    /// for `by`. Every object must still stand where the unit left it (compared by `rev`), else `Outcome::Conflict`
    /// and nothing is written; a redacted or missing entry is `Gone`; a unit with an effect outside the store is
    /// `NotRevertible`. The same `step` twice is the same undo unit (`replayed`). The standard body refuses.
    async fn revert_by(&self, change: ActivityId, by: &By) -> Result<Outcome> {
        let _ = (change, by);
        Err(Error::Invalid(
            "this store records no origin (revert_by)".into(),
        ))
    }
}
