//! The profile index: read it, change it in ONE transaction. Split from `rows`.
//!
//! Pattern: Repository; `change_index` is the read-change-write of the database (the closure runs inside the transaction).

use fs_types::Utc;
use turso::Value;

use super::prim::{bump, device};
use super::rows::live;
use super::tables::PROFILE;
use super::{IndexChange, IndexRows, ProfileRow, SettingRows};
use crate::v5::codec::{get_int, get_opt_int, get_opt_text, int, invalid, text};
use crate::v5::conn::Conn;
use crate::v5::error::Result;

async fn read(c: &Conn<'_>) -> Result<IndexRows> {
    let rows = c
        .query(
            "SELECT id, name, created, deleted FROM settings_profile ORDER BY id",
            Vec::new(),
        )
        .await?;
    let mut profiles = Vec::new();
    for r in &rows {
        profiles.push(ProfileRow {
            id: get_int(r, 0)?,
            name: get_opt_text(r, 1)?,
            created: Utc::from_millis(get_int(r, 2)?),
            deleted: get_opt_int(r, 3)?.map(Utc::from_millis),
        });
    }
    let active = c
        .query(
            "SELECT profile FROM settings_active WHERE device = ?",
            vec![text(&device(c).await?)],
        )
        .await?;
    let active = active.first().map_or(Ok(0), |r| get_int(r, 0))?;
    let top = profiles.iter().map(|p| p.id).max().unwrap_or(0);
    Ok(IndexRows {
        profiles,
        active,
        top,
    })
}

async fn name_taken(c: &Conn<'_>, key: &str, except: i64) -> Result<bool> {
    let rows = c
        .query(
            "SELECT 1 FROM settings_profile WHERE name_key = ? AND id <> ?",
            vec![text(key), int(except)],
        )
        .await?;
    Ok(!rows.is_empty())
}

async fn apply(c: &Conn<'_>, ch: &IndexChange, at: Utc) -> Result<()> {
    match ch {
        IndexChange::Create { id, name, name_key } => {
            let top = read(c).await?.top;
            if *id <= top {
                return Err(invalid("settings_profile.id", "id-invalid"));
            }
            if name_taken(c, name_key, -1).await? {
                return Err(invalid("settings_profile.name_key", "exists"));
            }
            c.insert(
                &PROFILE,
                vec![
                    int(*id),
                    text(name),
                    text(name_key),
                    int(at.millis()),
                    Value::Null,
                ],
            )
            .await?;
            Ok(())
        }
        _ => apply_existing(c, ch, at).await,
    }
}

async fn apply_existing(c: &Conn<'_>, ch: &IndexChange, at: Utc) -> Result<()> {
    match ch {
        IndexChange::Rename { id, name, name_key } => {
            live_named(c, *id).await?;
            if name_taken(c, name_key, *id).await? {
                return Err(invalid("settings_profile.name_key", "exists"));
            }
            c.exec(
                "UPDATE settings_profile SET name = ?, name_key = ? WHERE id = ?",
                vec![text(name), text(name_key), int(*id)],
            )
            .await?;
        }
        IndexChange::Delete { id } => {
            live_named(c, *id).await?;
            c.exec(
                "DELETE FROM settings_value WHERE profile = ?",
                vec![int(*id)],
            )
            .await?;
            c.exec(
                "UPDATE settings_profile SET deleted = ?, name_key = NULL WHERE id = ?",
                vec![int(at.millis()), int(*id)],
            )
            .await?;
            c.exec(
                "UPDATE settings_active SET profile = 0 WHERE profile = ?",
                vec![int(*id)],
            )
            .await?;
        }
        IndexChange::Switch { id } => {
            live(c, *id, "settings_profile.id").await?;
            c.exec(
                "INSERT INTO settings_active (device, profile) VALUES (?, ?) \
                 ON CONFLICT (device) DO UPDATE SET profile = excluded.profile",
                vec![text(&device(c).await?), int(*id)],
            )
            .await?;
        }
        IndexChange::Create { .. } => {}
    }
    Ok(())
}

/// A live named profile (not the base, not a tombstone).
async fn live_named(c: &Conn<'_>, id: i64) -> Result<()> {
    if id == 0 {
        return Err(invalid("settings_profile.id", "id-invalid"));
    }
    live(c, id, "settings_profile.id").await
}

impl SettingRows<'_> {
    /// The index as stored.
    ///
    /// # Errors
    /// The engine's.
    pub async fn index(&self) -> Result<IndexRows> {
        read(&self.store.read().await).await
    }

    /// Read-change-write in ONE transaction: `f` sees the index as stored now and answers the changes (or its own error `E`,
    /// then nothing changes: the outer `Ok(Err(e))`). Ids: `Create.id` must be > `top`, a tombstone is never revived. No
    /// change at all = no write (no `rev + 1`). Not `Send`: the server runs it on its own current-thread runtime.
    ///
    /// # Errors
    /// `Invalid` `settings_profile.id` `id-invalid` / `unknown`; `settings_profile.name_key` `exists`; the engine's. Then
    /// nothing changed.
    pub async fn change_index<E>(
        &self,
        at: Utc,
        f: &mut dyn FnMut(&IndexRows) -> std::result::Result<Vec<IndexChange>, E>,
    ) -> Result<std::result::Result<IndexRows, E>> {
        let c = self.store.begin().await?;
        let r = async {
            let now = read(&c).await?;
            let changes = match f(&now) {
                Ok(v) => v,
                Err(e) => return Ok(Err(e)),
            };
            if changes.is_empty() {
                return Ok(Ok(now));
            }
            for ch in &changes {
                apply(&c, ch, at).await?;
            }
            bump(&c).await?;
            Ok(Ok(read(&c).await?))
        }
        .await;
        c.finish(r).await
    }
}
