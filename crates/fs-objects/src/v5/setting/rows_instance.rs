//! The instance layer: read, set and clear defaults, read the log. Locks are in `rows_lock`. Split from `rows`.
//!
//! Pattern: Repository; every write is idempotent (`Outcome::Same` = no log row, no `rev + 1`).

use fs_types::{BasicValue, ObjectId, Utc};

use super::prim::{bump, cols, prim_at, revision};
use super::tables::{ACTION, INSTANCE_LOG};
use super::{Actor, InstanceRows, LockRow, LogRow, Outcome, Prim, SettingRows};
use crate::v5::codec::{
    Row, damaged, get_id, get_int, get_opt_text, get_text, id_value, int, invalid, opt_text,
    reference_of, text,
};
use crate::v5::conn::Conn;
use crate::v5::error::Result;
use crate::v5::id::new_id;

/// The default of `key` now.
pub(super) async fn current(c: &Conn<'_>, key: &str) -> Result<Option<Prim>> {
    let rows = c
        .query(
            "SELECT kind, v_int, v_text FROM settings_instance_value WHERE key = ?",
            vec![text(key)],
        )
        .await?;
    rows.first().map_or(Ok(None), |r| prim_at(r, 0))
}

/// The reason of the lock on `key`, if it is locked.
pub(super) async fn locked(c: &Conn<'_>, key: &str) -> Result<Option<String>> {
    let rows = c
        .query(
            "SELECT reason FROM settings_instance_lock WHERE key = ?",
            vec![text(key)],
        )
        .await?;
    rows.first().map(|r| get_text(r, 0)).transpose()
}

/// Sets or replaces the default of `key`.
pub(super) async fn store_value(
    c: &Conn<'_>,
    key: &str,
    v: &Prim,
    a: Actor<'_>,
    at: Utc,
) -> Result<()> {
    let [k, i, t] = cols(Some(v));
    c.upsert(
        &super::tables::INSTANCE_VALUE,
        &["key"],
        vec![
            text(key),
            k,
            i,
            t,
            int(at.millis()),
            text(&a.by.canonical()),
        ],
    )
    .await
}

/// One row of the append-only log.
#[allow(clippy::too_many_arguments)]
pub(super) async fn write_log(
    c: &Conn<'_>,
    action: &str,
    key: &str,
    reason: Option<&str>,
    old: Option<&Prim>,
    new: Option<&Prim>,
    a: Actor<'_>,
    at: Utc,
) -> Result<()> {
    let mut row = vec![
        id_value(new_id(at.millis())?),
        int(at.millis()),
        text(&a.by.canonical()),
        opt_text(a.os_user),
        text(action),
        text(key),
        opt_text(reason),
    ];
    row.extend(cols(old));
    row.extend(cols(new));
    c.insert(&INSTANCE_LOG, row).await
}

fn log_row(r: &Row) -> Result<LogRow> {
    let action = get_text(r, 4)?;
    Ok(LogRow {
        id: get_id(r, 0)?,
        at: Utc::from_millis(get_int(r, 1)?),
        by: reference_of(&get_text(r, 2)?)?,
        os_user: get_opt_text(r, 3)?,
        action: ACTION
            .iter()
            .find(|w| **w == action)
            .copied()
            .ok_or_else(|| damaged("log action"))?,
        key: get_text(r, 5)?,
        reason: get_opt_text(r, 6)?,
        old: prim_at(r, 7)?,
        new: prim_at(r, 10)?,
    })
}

impl SettingRows<'_> {
    /// The defaults and the locks.
    ///
    /// # Errors
    /// The engine's.
    pub async fn instance(&self) -> Result<InstanceRows> {
        let c = self.store.read().await;
        let mut values = Vec::new();
        for r in &c
            .query(
                "SELECT key, kind, v_int, v_text FROM settings_instance_value ORDER BY key",
                Vec::new(),
            )
            .await?
        {
            if let Some(p) = prim_at(r, 1)? {
                values.push((get_text(r, 0)?, p));
            }
        }
        let mut locks = Vec::new();
        for r in &c
            .query(
                "SELECT l.key, v.kind, v.v_int, v.v_text, l.reason, l.locked_by, l.locked_at \
                 FROM settings_instance_lock l JOIN settings_instance_value v ON v.key = l.key ORDER BY l.key",
                Vec::new(),
            )
            .await?
        {
            let value = prim_at(r, 1)?.ok_or_else(|| damaged("locked value"))?;
            locks.push(LockRow {
                key: get_text(r, 0)?,
                value,
                reason: get_text(r, 4)?,
                by: reference_of(&get_text(r, 5)?)?,
                at: Utc::from_millis(get_int(r, 6)?),
            });
        }
        Ok(InstanceRows { values, locks })
    }

    /// `v = None` clears. The same value again (or clearing what is not there) is `Same`.
    ///
    /// # Errors
    /// `Invalid` `settings_instance_lock.key` `locked` when the key is locked and the value would change.
    pub async fn put_instance(
        &self,
        key: &str,
        v: Option<&Prim>,
        a: Actor<'_>,
        at: Utc,
    ) -> Result<Outcome> {
        let c = self.store.begin().await?;
        let r = async {
            let old = current(&c, key).await?;
            if old.as_ref() == v {
                return Ok(Outcome::Same {
                    rev: revision(&c).await?,
                });
            }
            if locked(&c, key).await?.is_some() {
                return Err(invalid("settings_instance_lock.key", "locked"));
            }
            match v {
                Some(p) => store_value(&c, key, p, a, at).await?,
                None => {
                    c.exec(
                        "DELETE FROM settings_instance_value WHERE key = ?",
                        vec![text(key)],
                    )
                    .await?;
                }
            }
            let action = if v.is_some() { "set" } else { "clear" };
            write_log(&c, action, key, None, old.as_ref(), v, a, at).await?;
            Ok(Outcome::Changed {
                rev: bump(&c).await?,
            })
        }
        .await;
        c.finish(r).await
    }

    /// The log, oldest first; `after` = id of the last row seen (`UUIDv7` order = time order).
    ///
    /// # Errors
    /// The engine's.
    pub async fn log(&self, after: Option<ObjectId>, size: u32) -> Result<Vec<LogRow>> {
        let after = after.map_or_else(String::new, crate::v5::codec::id_text);
        let rows = self
            .store
            .read()
            .await
            .query(
                "SELECT id, at, by, os_user, action, key, reason, old_kind, old_int, old_text, new_kind, new_int, new_text \
                 FROM settings_instance_log WHERE id > ? ORDER BY id LIMIT ?",
                vec![text(&after), int(i64::from(size))],
            )
            .await?;
        rows.iter().map(log_row).collect()
    }
}
