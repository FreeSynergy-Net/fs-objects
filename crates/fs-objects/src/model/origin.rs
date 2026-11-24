//! [`Origin`] and [`By`]: where a change comes from and who asks for it.
//!
//! Pattern: Value Object (`Origin`) + Parameter Object (`By`) — the store gets the person with EVERY call (ADR 0104 E1).

use fs_types::PersonRef;
use serde::{Deserialize, Serialize};

/// Pattern: Value Object – origin of a change; every part except the person (kept in [`By`]) may be empty
/// (single place, mesh, no server needed).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Origin {
    /// The program that wrote.
    pub program: String,
    /// The instance of that program.
    pub instance: String,
    /// The device.
    pub device: String,
}

impl Origin {
    /// An origin with all three parts.
    pub fn new(
        program: impl Into<String>,
        instance: impl Into<String>,
        device: impl Into<String>,
    ) -> Self {
        Self {
            program: program.into(),
            instance: instance.into(),
            device: device.into(),
        }
    }
}

/// Pattern: Parameter Object – who writes, for whom, from where, in which run. The driver gets the person with every
/// call (0104 E1, S3 C1).
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct By {
    /// IRI/identifier of the acting person or program (as in `Activity::actor`).
    pub actor: String,
    /// The owner the written objects belong to.
    pub owner: PersonRef,
    /// Where the change comes from.
    pub origin: Origin,
    /// The run (context) of `step`.
    pub context: Option<String>,
    /// The step inside `context`: the same pair is written once (idempotent repetition).
    pub step: Option<String>,
}

impl By {
    /// Actor and owner, no origin, no step.
    pub fn new(actor: impl Into<String>, owner: PersonRef) -> Self {
        Self {
            actor: actor.into(),
            owner,
            origin: Origin::default(),
            context: None,
            step: None,
        }
    }
    /// Builder: set the origin.
    #[must_use]
    pub fn origin(mut self, o: Origin) -> Self {
        self.origin = o;
        self
    }
    /// Builder: set run and step.
    #[must_use]
    pub fn step(mut self, run: &str, step: &str) -> Self {
        self.context = Some(run.into());
        self.step = Some(step.into());
        self
    }
}
