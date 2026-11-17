//! Locks of instance defaults: lock (sets the value it locks) and unlock (with a reason). Split from `rows_instance`.
//!
//! Pattern: Repository; one transaction per call: value, lock, log row, `rev + 1`.

use fs_types::{BasicValue, Utc};

use super::SettingRows;
use super::rows_instance::{current, locked, outcome, store_value, write_log};
use super::{Actor, Outcome, Prim};
use crate::v5::codec::{int, invalid, text};
use crate::v5::conn::Conn;
use crate::v5::error::Result;

const REASON: &str = "settings_instance_lock.reason";

pub(super) fn need_reason(reason: &str) -> Result<()> {
    if reason.trim().is_empty() {
        Err(invalid(REASON, "reason-missing"))
    } else {
        Ok(())
    }
}

/// Lock inside a transaction; `true` = changed.
pub(super) async fn lock_in(
    c: &Conn<'_>,
    key: &str,
    v: &Prim,
    reason: &str,
    a: Actor<'_>,
    at: Utc,
) -> Result<bool> {
    need_reason(reason)?;
    let old = current(c, key).await?;
    if let Some(held) = locked(c, key).await? {
        if old.as_ref() == Some(v) && held == reason {
            return Ok(false);
        }
        return Err(invalid("settings_instance_lock.key", "locked"));
    }
    store_value(c, key, v, a, at).await?;
    c.insert(
        &super::tables::INSTANCE_LOCK,
        vec![
            text(key),
            text(reason),
            text(&a.by.canonical()),
            int(at.millis()),
        ],
    )
    .await?;
    write_log(c, "lock", key, Some(reason), old.as_ref(), Some(v), a, at).await?;
    Ok(true)
}

/// Unlock inside a transaction; `true` = changed.
pub(super) async fn unlock_in(
    c: &Conn<'_>,
    key: &str,
    reason: &str,
    a: Actor<'_>,
    at: Utc,
) -> Result<bool> {
    need_reason(reason)?;
    if locked(c, key).await?.is_none() {
        return Ok(false);
    }
    let value = current(c, key).await?;
    c.exec(
        "DELETE FROM settings_instance_lock WHERE key = ?",
        vec![text(key)],
    )
    .await?;
    write_log(
        c,
        "unlock",
        key,
        Some(reason),
        value.as_ref(),
        value.as_ref(),
        a,
        at,
    )
    .await?;
    Ok(true)
}

impl SettingRows<'_> {
    /// Sets the value AND the lock in one transaction (ADR 0100 decision 5: the locker sets the value it locks). Already
    /// locked with the same value and reason: `Same`.
    ///
    /// # Errors
    /// `Invalid` `settings_instance_lock.reason` `reason-missing` (empty after trim); `settings_instance_lock.key` `locked`
    /// (locked with another value or reason).
    pub async fn lock(
        &self,
        key: &str,
        v: &Prim,
        reason: &str,
        a: Actor<'_>,
        at: Utc,
    ) -> Result<Outcome> {
        need_reason(reason)?;
        let c = self.store.begin().await?;
        let r = async {
            let changed = lock_in(&c, key, v, reason, a, at).await?;
            outcome(&c, changed).await
        }
        .await;
        c.finish(r).await
    }

    /// The value stays as default; the reason is mandatory (c0 decision 6). Not locked: `Same`.
    ///
    /// # Errors
    /// `Invalid` `settings_instance_lock.reason` `reason-missing` (checked first).
    pub async fn unlock(&self, key: &str, reason: &str, a: Actor<'_>, at: Utc) -> Result<Outcome> {
        need_reason(reason)?;
        let c = self.store.begin().await?;
        let r = async {
            let changed = unlock_in(&c, key, reason, a, at).await?;
            outcome(&c, changed).await
        }
        .await;
        c.finish(r).await
    }
}
