//! Locks of instance defaults: lock (sets the value it locks) and unlock (with a reason). Split from `rows_instance`.
//!
//! Pattern: Repository; one transaction per call: value, lock, log row, `rev + 1`.

use fs_types::{BasicValue, Utc};

use super::SettingRows;
use super::prim::{bump, revision};
use super::rows_instance::{current, locked, store_value, write_log};
use super::{Actor, Outcome, Prim};
use crate::v5::codec::{int, invalid, text};
use crate::v5::error::Result;

const REASON: &str = "settings_instance_lock.reason";

fn need_reason(reason: &str) -> Result<()> {
    if reason.trim().is_empty() {
        Err(invalid(REASON, "reason-missing"))
    } else {
        Ok(())
    }
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
            let old = current(&c, key).await?;
            if let Some(held) = locked(&c, key).await? {
                if old.as_ref() == Some(v) && held == reason {
                    return Ok(Outcome::Same {
                        rev: revision(&c).await?,
                    });
                }
                return Err(invalid("settings_instance_lock.key", "locked"));
            }
            store_value(&c, key, v, a, at).await?;
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
            write_log(&c, "lock", key, Some(reason), old.as_ref(), Some(v), a, at).await?;
            Ok(Outcome::Changed {
                rev: bump(&c).await?,
            })
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
            if locked(&c, key).await?.is_none() {
                return Ok(Outcome::Same {
                    rev: revision(&c).await?,
                });
            }
            let value = current(&c, key).await?;
            c.exec(
                "DELETE FROM settings_instance_lock WHERE key = ?",
                vec![text(key)],
            )
            .await?;
            write_log(
                &c,
                "unlock",
                key,
                Some(reason),
                value.as_ref(),
                value.as_ref(),
                a,
                at,
            )
            .await?;
            Ok(Outcome::Changed {
                rev: bump(&c).await?,
            })
        }
        .await;
        c.finish(r).await
    }
}
