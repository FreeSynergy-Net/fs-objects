//! Trash instead of delete (R4, ADR 0073): the trash period, `trash`, `untrash`. Split from `envelope` (re-exported there).
//!
//! Pattern: Transaction script (in the caller's transaction); the trash is an UPDATE, never a DELETE and INSERT.

use fs_types::{ObjectId, Revision};

use super::codec::{get_int, id_value, int, invalid};
use super::conn::Conn;
use super::envelope::{DEFAULT_TRASH_DAYS, missed, state};
use super::error::{Error, Result};
use super::hold::{is_held, trash_auto};

const DAY_MS: i64 = 86_400_000;

/// The trash period in days: `setting.trash_days`, else 30 (draft section 3).
///
/// # Errors
/// The engine's error.
pub async fn trash_days(c: &Conn<'_>) -> Result<i64> {
    let rows = c
        .query(
            "SELECT int_value FROM setting WHERE key = 'trash_days'",
            Vec::new(),
        )
        .await?;
    rows.first()
        .map_or(Ok(DEFAULT_TRASH_DAYS), |r| get_int(r, 0))
}

/// Change the trash period (a later delete uses it, rows already in the trash keep their `purge_after`).
///
/// # Errors
/// `Invalid` for a period that is not positive.
pub async fn set_trash_days(c: &Conn<'_>, days: i64) -> Result<()> {
    if days <= 0 {
        return Err(invalid("setting.trash_days", "must be positive"));
    }
    let sql = "INSERT INTO setting (key, int_value) VALUES ('trash_days', ?) ON CONFLICT (key) DO UPDATE SET int_value = excluded.int_value";
    c.exec(sql, vec![int(days)]).await.map(|_| ())
}

/// Move an object to the trash: `deleted = now`, `purge_after = now + trash_days`, fixed now (draft R4).
///
/// # Errors
/// As [`bump`].
pub async fn trash(
    c: &Conn<'_>,
    id: ObjectId,
    expected: Revision,
    now: i64,
) -> Result<(Revision, i64)> {
    let until = trash_days(c)
        .await?
        .checked_mul(DAY_MS)
        .and_then(|span| now.checked_add(span))
        .ok_or_else(|| invalid("setting.trash_days", "the period is too long"))?;
    let exp = int(i64::try_from(expected.0).unwrap_or(i64::MAX));
    let sql = "UPDATE objects SET rev = rev + 1, updated = ?, deleted = ?, purge_after = ? WHERE id = ? AND rev = ? AND deleted IS NULL";
    let n = c
        .exec(sql, vec![int(now), int(now), int(until), id_value(id), exp])
        .await?;
    if n == 1 {
        Ok((Revision(expected.0 + 1), until))
    } else {
        Err(missed(c, id).await)
    }
}

/// Take an object out of the trash while `purge_after` has not passed, or while a hold lasts, or when the trash is not
/// emptied automatically (the object is still there then).
///
/// # Errors
/// `NotFound` when there is no row or the period is over; `Invalid` when it is not in the trash.
pub async fn untrash(c: &Conn<'_>, id: ObjectId, now: i64) -> Result<Revision> {
    let s = state(c, id).await?.ok_or(Error::NotFound(id))?;
    let open = match (s.deleted, s.purge_after) {
        (Some(_), Some(until)) => {
            until > now || is_held(c, id, now).await? || !trash_auto(c).await?
        }
        _ => return Err(invalid("id", "not in the trash")),
    };
    if !open {
        return Err(Error::NotFound(id));
    }
    let sql = "UPDATE objects SET rev = rev + 1, updated = ?, deleted = NULL, purge_after = NULL WHERE id = ?";
    c.exec(sql, vec![int(now), id_value(id)]).await?;
    Ok(Revision(s.rev + 1))
}
