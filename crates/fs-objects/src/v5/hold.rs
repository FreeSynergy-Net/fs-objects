//! A hold keeps a trashed object past its trash period (ADR 0073 addendum 2026-11-16: "never delete automatically",
//! `keep_until` goes before the period), and the setting `trash_auto` (the period run may empty the trash or not).
//! The program decides what a hold means, e.g. it holds an object until the `keep_until` of its original.
//!
//! Pattern: Transaction script (small functions in the caller's transaction); the hold is a row, the rule lives in
//! `envelope::due` and `envelope::untrash`.

use fs_types::ObjectId;

use super::codec::{get_int, get_opt_int, id_value, int};
use super::conn::Conn;
use super::envelope::state;
use super::error::{Error, Result};

/// Until when a trashed object can still be taken back (what a program shows as "kept until ...").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Restorable {
    /// Until this time (UTC milliseconds): the later of the end of the period and the end of the hold.
    Until(i64),
    /// Without end: a hold without end, or the trash is not emptied automatically.
    Never,
}

/// Hold an object: `Some(t)` until `t` (UTC milliseconds), `None` = never released. Replaces an earlier hold.
///
/// # Errors
/// `Invalid` when there is no such object, or the engine's error.
pub async fn hold(c: &Conn<'_>, id: ObjectId, until: Option<i64>) -> Result<()> {
    let sql = "INSERT INTO object_hold (object, until) VALUES (?, ?) ON CONFLICT (object) DO UPDATE SET until = excluded.until";
    let until = until.map_or(turso::Value::Null, int);
    c.exec(sql, vec![id_value(id), until]).await.map(|_| ())
}

/// Release a hold (no hold: nothing happens).
///
/// # Errors
/// The engine's error.
pub async fn release(c: &Conn<'_>, id: ObjectId) -> Result<()> {
    c.exec(
        "DELETE FROM object_hold WHERE object = ?",
        vec![id_value(id)],
    )
    .await
    .map(|_| ())
}

/// The hold of an object: `None` = no hold, `Some(None)` = never released, `Some(Some(t))` = until `t`.
///
/// # Errors
/// The engine's error, or a damaged row.
pub async fn held_until(c: &Conn<'_>, id: ObjectId) -> Result<Option<Option<i64>>> {
    let rows = c
        .query(
            "SELECT until FROM object_hold WHERE object = ?",
            vec![id_value(id)],
        )
        .await?;
    rows.first().map(|r| get_opt_int(r, 0)).transpose()
}

/// Whether the hold of an object is still on at `now` (never released, or `until > now`).
///
/// # Errors
/// The engine's error, or a damaged row.
pub async fn is_held(c: &Conn<'_>, id: ObjectId, now: i64) -> Result<bool> {
    Ok(held_until(c, id)
        .await?
        .is_some_and(|u| u.is_none_or(|t| t > now)))
}

/// Whether the period run empties the trash: `setting.trash_auto` (0 = off), else on.
///
/// # Errors
/// The engine's error.
pub async fn trash_auto(c: &Conn<'_>) -> Result<bool> {
    let rows = c
        .query(
            "SELECT int_value FROM setting WHERE key = 'trash_auto'",
            Vec::new(),
        )
        .await?;
    rows.first()
        .map_or(Ok(true), |r| get_int(r, 0).map(|v| v != 0))
}

/// Switch the automatic emptying of the trash on or off (off = "never delete automatically").
///
/// # Errors
/// The engine's error.
pub async fn set_trash_auto(c: &Conn<'_>, on: bool) -> Result<()> {
    let sql = "INSERT INTO setting (key, int_value) VALUES ('trash_auto', ?) ON CONFLICT (key) DO UPDATE SET int_value = excluded.int_value";
    c.exec(sql, vec![int(i64::from(on))]).await.map(|_| ())
}

/// Until when a trashed object can be taken back, for the "kept until ..." line of a frontend: `None` when the object
/// is not in the trash.
///
/// # Errors
/// `NotFound` when there is no such object.
pub async fn restorable_until(c: &Conn<'_>, id: ObjectId) -> Result<Option<Restorable>> {
    let s = state(c, id).await?.ok_or(Error::NotFound(id))?;
    let Some(period) = s.purge_after.filter(|_| s.deleted.is_some()) else {
        return Ok(None);
    };
    if !trash_auto(c).await? {
        return Ok(Some(Restorable::Never));
    }
    Ok(Some(match held_until(c, id).await? {
        Some(None) => Restorable::Never,
        Some(Some(t)) => Restorable::Until(t.max(period)),
        None => Restorable::Until(period),
    }))
}
