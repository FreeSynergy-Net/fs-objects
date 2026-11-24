//! [`ObjectRepository`]: create, update, read and delete objects.
//!
//! Pattern: Repository — the CRUD contract of the store; every write is atomic with its
//! activity-log entry.

use async_trait::async_trait;

use crate::{Activity, By, Error, LogEntry, ObjectId, Outcome, Result, StoredObject, Write};

/// Write and read single objects.
#[async_trait]
pub trait ObjectRepository: Send + Sync {
    /// Create or update (decided by whether `object.envelope.id` exists).
    /// The store sets `updated` to now; `published` of an existing object is kept.
    /// Writes object + `Create`/`Update` activity in one transaction.
    async fn put(&self, object: StoredObject, actor: &str) -> Result<Activity>;

    /// Like [`ObjectRepository::put`], and append further log-only activities (e.g. `Accept` /
    /// `TentativeAccept` / `Reject` for an invitation reply) **in the same transaction**: all or
    /// nothing. The extra entries refer to the same object and actor and follow the
    /// `Create`/`Update` entry in log order. Returns `Create`/`Update` first, then the extras.
    /// State-changing kinds are refused as extras.
    async fn put_and_log(
        &self,
        object: StoredObject,
        actor: &str,
        extra: Vec<LogEntry>,
    ) -> Result<Vec<Activity>>;

    /// Pattern: Unit of Work – write several objects as ONE undo unit in one transaction and record where
    /// the change comes from (`by`). Rules: a `step` already in the log is not written twice (`replayed`); every
    /// `expect` is compared with the object's `rev` (a mismatch is `Outcome::Conflict`, nothing is written); a
    /// new object belongs to `by.owner`.
    ///
    /// The standard body refuses: a backend that does not record origin must not pretend it did.
    async fn write_by(&self, by: &By, writes: Vec<Write>) -> Result<Outcome> {
        let _ = (by, writes);
        Err(Error::Invalid(
            "this store records no origin (write_by)".into(),
        ))
    }

    /// Pattern: Unit of Work (the one exception) – the first write of a store: a single `Put` of a person whose owner is
    /// the object itself (`by.owner.id` is `urn:uuid:` + the object's id, `art` is `Person`). Allowed once, while the store
    /// has no owner; it sets the owner for good. A second one is `Invalid("store already owned")`. The standard body
    /// refuses.
    async fn birth(&self, by: &By, object: StoredObject) -> Result<Outcome> {
        let _ = (by, object);
        Err(Error::Invalid(
            "this store records no origin (birth)".into(),
        ))
    }

    /// Read one object by id.
    async fn get(&self, id: ObjectId) -> Result<Option<StoredObject>>;

    /// Delete an object, writing a `Delete` activity (carrying the old image so it can be undone).
    async fn delete(&self, id: ObjectId, actor: &str) -> Result<Activity>;
}
