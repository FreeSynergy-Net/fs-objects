//! The error of the v5 layer.
//!
//! Pattern: Value object (a closed list of causes). The v4 [`crate::Error`] is untouched; a program maps this one to
//! the error of its own trait (`Invalid` carries one `table.column` problem per field, as `fs-types` defines it).

use fs_types::{ObjectId, Problem, Revision};

/// What can go wrong in the v5 layer.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// There is no such object (or it is in the trash where a live one is needed).
    #[error("object not found")]
    NotFound(ObjectId),
    /// A value breaks a rule; one problem per field (`field` is `table.column`, `reason` a code).
    #[error("invalid input: {}", .0.iter().map(|p| format!("{}: {}", p.field, p.reason)).collect::<Vec<_>>().join("; "))]
    Invalid(Vec<Problem>),
    /// Compare-and-set lost: someone wrote before; `current` is what the row says now.
    #[error("changed in the meantime (revision {})", .current.0)]
    Changed {
        /// The revision the row has now.
        current: Revision,
    },
    /// The store or the engine cannot do it now (also: a damaged row, no OS randomness).
    #[error("unavailable: {0}")]
    Unavailable(String),
    /// The steps a program reports differ from the ones the store has applied (reordered, inserted or changed: steps
    /// are only appended).
    #[error("step {step} of {domain} differs from the one applied to this store")]
    StepMismatch {
        /// The domain prefix.
        domain: String,
        /// The number of the step, from 1.
        step: usize,
    },
    /// A schema step or a registration failed; the store is as it was before that step.
    #[error("schema: {0}")]
    Schema(String),
}

/// The result type of the v5 layer.
pub type Result<T> = std::result::Result<T, Error>;
