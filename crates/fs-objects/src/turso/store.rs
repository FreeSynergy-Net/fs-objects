//! The [`TursoStore`] type and the primitive "apply a change + record its activity" step.
//!
//! Pattern: Adapter — `TursoStore` adapts the embedded Turso engine to the `Store` traits
//! (one trait impl per sibling module). `apply` is the Command executor: a change is
//! (kind, before, after) and is written together with its log entry.

use std::sync::atomic::AtomicBool;

use tokio::sync::{Mutex, broadcast};
use turso::Connection;

use super::activities::{insert_activity, mark_redacted};
use super::objects::{remove_object, write_object};
use super::payload::payload_for;
use super::rows::now;
use super::schema::SCHEMA_VERSION;
use crate::{
    Activity, ActivityId, ActivityKind, Error, ObjectId, Origin, PersonRef, Result, StoreId,
    StoredObject,
};

/// Connection plus the last issued activity id (guarded by one mutex).
pub(super) struct Inner {
    pub(super) conn: Connection,
    pub(super) last_activity: Option<ActivityId>,
    /// The owner of the store (set at open, or by `birth`); behind the same lock as the writes.
    pub(super) owner: Option<PersonRef>,
}

/// Embedded Turso backend.
pub struct TursoStore {
    // Keep the database handle alive for the lifetime of the connection.
    pub(super) _db: turso::Database,
    pub(super) inner: Mutex<Inner>,
    pub(super) tx: broadcast::Sender<Activity>,
    pub(super) encrypted: bool,
    pub(super) allow_sensitive_unencrypted: bool,
    pub(super) fail_before_activity: AtomicBool,
    pub(super) fail_before_extra: AtomicBool,
    pub(super) fail_in_purge: AtomicBool,
    pub(super) on_disk: bool,
    pub(super) identity: StoreId,
}

/// Where a change comes from, written into its activity (empty on the legacy paths).
#[derive(Default)]
pub(super) struct Meta {
    pub(super) owner: Option<PersonRef>,
    pub(super) origin: Origin,
    pub(super) context: Option<String>,
    pub(super) step: Option<String>,
    pub(super) unit: Option<ActivityId>,
}

/// One state change of an object, to be written with its activity.
pub(super) struct Change {
    pub(super) kind: ActivityKind,
    pub(super) object: ObjectId,
    pub(super) target: Option<ActivityId>,
    pub(super) before: Option<StoredObject>,
    pub(super) after: Option<StoredObject>,
}

impl TursoStore {
    /// True if the file is encrypted at rest.
    pub fn is_encrypted(&self) -> bool {
        self.encrypted
    }

    /// Schema version of the open database.
    pub fn schema_version(&self) -> i64 {
        SCHEMA_VERSION
    }

    /// Send committed activities to subscribers (no receivers is fine).
    pub(super) fn publish(&self, acts: &[Activity]) {
        for a in acts {
            let _ = self.tx.send(a.clone());
        }
    }

    /// Write `before -> after` for the object plus its activity. Caller holds the lock and a
    /// transaction.
    pub(super) async fn apply(
        &self,
        inner: &mut Inner,
        actor: &str,
        ch: Change,
    ) -> Result<Activity> {
        let meta = Meta {
            owner: inner.owner.clone(),
            ..Meta::default()
        };
        self.apply_with(inner, actor, ch, &meta).await
    }

    /// [`TursoStore::apply`] with origin: stamps `rev` (one more than the image before) and the owner on the new
    /// image, then writes the activity with the same values.
    pub(super) async fn apply_with(
        &self,
        inner: &mut Inner,
        actor: &str,
        mut ch: Change,
        meta: &Meta,
    ) -> Result<Activity> {
        let revision = ch.before.as_ref().map_or(0, |b| b.rev) + 1;
        let owner = ch
            .before
            .as_ref()
            .and_then(|b| b.owner.clone())
            .or_else(|| meta.owner.clone());
        if let Some(a) = ch.after.as_mut() {
            a.rev = revision;
            a.owner.clone_from(&owner);
        }
        match &ch.after {
            Some(o) => write_object(&inner.conn, o).await?,
            None => remove_object(&inner.conn, ch.object).await?,
        }
        self.check_fault()?;
        let id = ActivityId::next_after(inner.last_activity);
        let (payload, redacted) = payload_for(&inner.conn, ch.object, ch.before, ch.after).await?;
        let act = Activity {
            id,
            kind: ch.kind,
            object: ch.object,
            actor: actor.to_string(),
            at: now(),
            target: ch.target,
            payload,
            synced: false,
            owner: meta.owner.clone(),
            origin: meta.origin.clone(),
            context: meta.context.clone(),
            step: meta.step.clone(),
            unit: meta.unit,
            revision,
        };
        insert_activity(&inner.conn, &act).await?;
        if redacted {
            mark_redacted(&inner.conn, id).await?;
        }
        inner.last_activity = Some(id);
        Ok(act)
    }

    /// Append a log-only activity row. Caller holds the lock and a transaction.
    pub(super) async fn append_log(
        &self,
        inner: &mut Inner,
        actor: &str,
        object: ObjectId,
        kind: ActivityKind,
        payload: serde_json::Value,
    ) -> Result<Activity> {
        if kind.changes_state() {
            return Err(Error::Invalid(format!(
                "{} is written by put/delete/undo/redo only",
                kind.as_str()
            )));
        }
        let id = ActivityId::next_after(inner.last_activity);
        let act = Activity {
            id,
            kind,
            object,
            actor: actor.into(),
            at: now(),
            target: None,
            payload,
            synced: false,
            owner: inner.owner.clone(),
            origin: Origin::default(),
            context: None,
            step: None,
            unit: None,
            revision: 0,
        };
        insert_activity(&inner.conn, &act).await?;
        inner.last_activity = Some(id);
        Ok(act)
    }
}
