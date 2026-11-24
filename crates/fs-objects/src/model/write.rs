//! [`Write`], [`Recorded`], [`Outcome`], [`FieldClash`]: the vocabulary of `ObjectRepository::write_by`.
//!
//! Pattern: Unit of Work (`Write`, `Recorded`) + Result Object (`Outcome`) — one command is one transaction and one undo unit;
//! a factual result (conflict, nothing to revert) is a value, not a new `Error` branch (`Error` is not `non_exhaustive`).

use crate::{Activity, ObjectId, StoredObject};

/// Pattern: Unit of Work – one write of a command; all writes of one call are one transaction and one undo unit.
#[derive(Clone, Debug)]
#[non_exhaustive]
#[allow(clippy::large_enum_variant)] // an owned image on purpose: the caller hands it over
pub enum Write {
    /// Create or update an object. `expect` = the `rev` the caller saw (compare and set).
    Put {
        /// The new image.
        object: StoredObject,
        /// The revision the caller based the change on; `None` = no check.
        expect: Option<u64>,
    },
    /// Delete an object.
    Delete {
        /// The object.
        id: ObjectId,
        /// The revision the caller based the change on; `None` = no check.
        expect: Option<u64>,
    },
}

/// Pattern: Result Object – what a successful `write_by` recorded.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct Recorded {
    /// The head activity of the unit (carries the `step`).
    pub head: Activity,
    /// All activities of the unit, head first.
    pub all: Vec<Activity>,
    /// True if `(context, step)` was already in the log: nothing was written, this is the old unit.
    pub replayed: bool,
}

/// Pattern: Value Object – one path whose value changed since the caller's `expect`.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct FieldClash {
    /// Dotted path inside the object image, e.g. `content.name.full`.
    pub path: String,
}

/// Pattern: Result Object – the factual outcome of a write without a new `Error` branch.
#[derive(Clone, Debug)]
#[allow(clippy::large_enum_variant)] // the common case (`Done`) is the big one
#[non_exhaustive]
pub enum Outcome {
    /// Written (or replayed).
    Done(Recorded),
    /// A `expect` did not match; nothing was written.
    Conflict {
        /// The last state-changing activity of the object.
        by: Box<Activity>,
        /// Paths whose value changed since `expect` (empty if the old image is gone).
        fields: Vec<FieldClash>,
    },
    /// The change cannot be reverted (reserved; nobody sets it yet).
    NotRevertible(String),
    /// The object or its activity is gone.
    Gone,
}
