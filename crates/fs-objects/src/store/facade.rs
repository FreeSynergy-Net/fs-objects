//! The [`Store`] facade: the whole object store as one trait.
//!
//! Pattern: Facade — `Store` re-exposes the seven focused traits ([`ObjectRepository`],
//! [`ObjectQuery`], [`ActivityLog`], [`ChangeFeed`], [`UndoRedo`], [`HistoryRetention`], [`StoreInfo`]) under one
//! name, so dependents keep writing `Arc<dyn Store>` and `use fs_objects::Store` while small
//! consumers can depend on just the trait they need. It is blanket-implemented: any type that
//! implements the seven traits is a `Store`, and its methods are callable with only `Store` in scope.

use async_trait::async_trait;
use tokio::sync::broadcast;

use crate::{
    Activity, ActivityFilter, ActivityId, ActivityKind, ActivityLog, By, ChangeFeed, ChangeFilter,
    CompactionReport, HistoryRetention, LogEntry, LogStats, ObjectId, ObjectQuery,
    ObjectRepository, Outcome, PurgeReport, Query, Result, Retention, StoreId, StoreInfo,
    StoredObject, Timestamp, UndoRedo, UndoState, Write,
};

/// Object store: objects + append-only activity log, every change atomic with its log entry.
///
/// Backend-neutral: a remote JSON/HTTPS backend can implement the focused traits too (ADR 0019).
/// See the focused traits for the documentation of each method.
#[async_trait]
pub trait Store: Send + Sync {
    /// See [`ObjectRepository::put`].
    async fn put(&self, object: StoredObject, actor: &str) -> Result<Activity>;
    /// See [`ObjectRepository::put_and_log`].
    async fn put_and_log(
        &self,
        object: StoredObject,
        actor: &str,
        extra: Vec<LogEntry>,
    ) -> Result<Vec<Activity>>;
    /// See [`ObjectRepository::write_by`].
    async fn write_by(&self, by: &By, writes: Vec<Write>) -> Result<Outcome>;
    /// See [`ObjectRepository::birth`]. Default: refuses (records no origin).
    async fn birth(&self, by: &By, object: StoredObject) -> Result<Outcome> {
        let _ = (by, object);
        Err(crate::Error::Invalid(
            "this store records no origin (birth)".into(),
        ))
    }
    /// See [`ObjectRepository::get`].
    async fn get(&self, id: ObjectId) -> Result<Option<StoredObject>>;
    /// See [`ObjectRepository::delete`].
    async fn delete(&self, id: ObjectId, actor: &str) -> Result<Activity>;
    /// See [`ObjectQuery::query`].
    async fn query(&self, query: &Query) -> Result<Vec<StoredObject>>;
    /// See [`ObjectQuery::count`].
    async fn count(&self, query: &Query) -> Result<usize>;
    /// See [`ActivityLog::log`].
    async fn log(
        &self,
        kind: ActivityKind,
        object: ObjectId,
        actor: &str,
        payload: serde_json::Value,
    ) -> Result<Activity>;
    /// See [`ChangeFeed::subscribe`].
    fn subscribe(&self) -> broadcast::Receiver<Activity>;
    /// See [`ActivityLog::activities`].
    async fn activities(&self, filter: &ActivityFilter) -> Result<Vec<Activity>>;
    /// See [`UndoRedo::undo`].
    async fn undo(&self, actor: &str, n: usize) -> Result<Vec<Activity>>;
    /// See [`UndoRedo::redo`].
    async fn redo(&self, actor: &str, n: usize) -> Result<Vec<Activity>>;
    /// See [`UndoRedo::undo_state`].
    async fn undo_state(&self, actor: &str) -> Result<UndoState>;
    /// See [`UndoRedo::revert_by`]. Default: refuses (records no origin).
    async fn revert_by(&self, change: ActivityId, by: &By) -> Result<Outcome> {
        let _ = (change, by);
        Err(crate::Error::Invalid(
            "this store records no origin (revert_by)".into(),
        ))
    }
    /// See [`ActivityLog::changes_by`]. Default: refuses (records no origin).
    async fn changes_by(&self, f: &ChangeFilter) -> Result<Vec<Activity>> {
        let _ = f;
        Err(crate::Error::Invalid(
            "this store records no origin (changes_by)".into(),
        ))
    }
    /// See [`HistoryRetention::retention`].
    async fn retention(&self) -> Result<Retention>;
    /// See [`HistoryRetention::set_retention`].
    async fn set_retention(&self, retention: Retention) -> Result<()>;
    /// See [`HistoryRetention::compact_at`].
    async fn compact_at(&self, now: Timestamp) -> Result<CompactionReport>;
    /// See [`HistoryRetention::compact`].
    async fn compact(&self) -> Result<CompactionReport>;
    /// See [`HistoryRetention::clear_history`].
    async fn clear_history(&self) -> Result<CompactionReport>;
    /// See [`HistoryRetention::mark_synced`].
    async fn mark_synced(&self, up_to: ActivityId) -> Result<usize>;
    /// See [`HistoryRetention::log_stats`].
    async fn log_stats(&self) -> Result<LogStats>;
    /// See [`HistoryRetention::clean_files`]. Default: unsupported.
    async fn clean_files(&self) -> Result<bool> {
        Err(crate::Error::Backend(
            "clean_files is not supported by this store".into(),
        ))
    }
    /// See [`StoreInfo::identity`].
    fn identity(&self) -> StoreId;
    /// See [`StoreInfo::is_encrypted`].
    fn is_encrypted(&self) -> bool;
    /// See [`HistoryRetention::purge_object`]. Default: unsupported.
    async fn purge_object(&self, id: ObjectId) -> Result<PurgeReport> {
        let _ = id;
        Err(crate::Error::Backend(
            "purge_object is not supported by this store".into(),
        ))
    }
}

#[async_trait]
impl<T> Store for T
where
    T: ObjectRepository
        + ObjectQuery
        + ActivityLog
        + ChangeFeed
        + UndoRedo
        + HistoryRetention
        + StoreInfo,
{
    async fn put(&self, object: StoredObject, actor: &str) -> Result<Activity> {
        ObjectRepository::put(self, object, actor).await
    }
    async fn put_and_log(
        &self,
        object: StoredObject,
        actor: &str,
        extra: Vec<LogEntry>,
    ) -> Result<Vec<Activity>> {
        ObjectRepository::put_and_log(self, object, actor, extra).await
    }
    async fn write_by(&self, by: &By, writes: Vec<Write>) -> Result<Outcome> {
        ObjectRepository::write_by(self, by, writes).await
    }
    async fn birth(&self, by: &By, object: StoredObject) -> Result<Outcome> {
        ObjectRepository::birth(self, by, object).await
    }
    async fn get(&self, id: ObjectId) -> Result<Option<StoredObject>> {
        ObjectRepository::get(self, id).await
    }
    async fn delete(&self, id: ObjectId, actor: &str) -> Result<Activity> {
        ObjectRepository::delete(self, id, actor).await
    }
    async fn query(&self, query: &Query) -> Result<Vec<StoredObject>> {
        ObjectQuery::query(self, query).await
    }
    async fn count(&self, query: &Query) -> Result<usize> {
        ObjectQuery::count(self, query).await
    }
    async fn log(
        &self,
        kind: ActivityKind,
        object: ObjectId,
        actor: &str,
        payload: serde_json::Value,
    ) -> Result<Activity> {
        ActivityLog::log(self, kind, object, actor, payload).await
    }
    fn subscribe(&self) -> broadcast::Receiver<Activity> {
        ChangeFeed::subscribe(self)
    }
    async fn activities(&self, filter: &ActivityFilter) -> Result<Vec<Activity>> {
        ActivityLog::activities(self, filter).await
    }
    async fn undo(&self, actor: &str, n: usize) -> Result<Vec<Activity>> {
        UndoRedo::undo(self, actor, n).await
    }
    async fn redo(&self, actor: &str, n: usize) -> Result<Vec<Activity>> {
        UndoRedo::redo(self, actor, n).await
    }
    async fn undo_state(&self, actor: &str) -> Result<UndoState> {
        UndoRedo::undo_state(self, actor).await
    }
    async fn revert_by(&self, change: ActivityId, by: &By) -> Result<Outcome> {
        UndoRedo::revert_by(self, change, by).await
    }
    async fn changes_by(&self, f: &ChangeFilter) -> Result<Vec<Activity>> {
        ActivityLog::changes_by(self, f).await
    }
    async fn retention(&self) -> Result<Retention> {
        HistoryRetention::retention(self).await
    }
    async fn set_retention(&self, retention: Retention) -> Result<()> {
        HistoryRetention::set_retention(self, retention).await
    }
    async fn compact_at(&self, now: Timestamp) -> Result<CompactionReport> {
        HistoryRetention::compact_at(self, now).await
    }
    async fn compact(&self) -> Result<CompactionReport> {
        HistoryRetention::compact(self).await
    }
    async fn clear_history(&self) -> Result<CompactionReport> {
        HistoryRetention::clear_history(self).await
    }
    async fn mark_synced(&self, up_to: ActivityId) -> Result<usize> {
        HistoryRetention::mark_synced(self, up_to).await
    }
    async fn log_stats(&self) -> Result<LogStats> {
        HistoryRetention::log_stats(self).await
    }
    async fn clean_files(&self) -> Result<bool> {
        HistoryRetention::clean_files(self).await
    }
    fn identity(&self) -> StoreId {
        StoreInfo::identity(self)
    }
    fn is_encrypted(&self) -> bool {
        StoreInfo::is_encrypted(self)
    }
    async fn purge_object(&self, id: ObjectId) -> Result<PurgeReport> {
        HistoryRetention::purge_object(self, id).await
    }
}
