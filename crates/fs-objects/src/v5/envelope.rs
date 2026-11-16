//! The envelope row of an object: compare-and-set on `rev` (R7), trash instead of delete (R4), final delete.
//! Every function runs inside the caller's transaction (R3).
//!
//! Pattern: Transaction script (small functions on one connection guard; the caller owns commit and rollback).

use fs_types::{ObjectId, PersonRef, Revision};
use turso::Value;

use super::codec::{get_int, get_opt_int, id_value, int, invalid, person_values, text};
use super::conn::Conn;
use super::error::{Error, Result};
use super::registry::Domain;
use super::tables::OBJECTS;

pub use super::envelope_trash::{set_trash_days, trash, trash_days, untrash};

/// The trash period when `setting.trash_days` says nothing.
pub const DEFAULT_TRASH_DAYS: i64 = 30;

/// Rows of a domain table that name a dependent object: `SELECT object FROM table WHERE parent = <deleted id>`.
#[derive(Clone, Copy, Debug)]
pub struct Child {
    /// The table.
    pub table: &'static str,
    /// The column that holds the id of the parent object.
    pub parent: &'static str,
    /// The column that holds the id of the dependent object (its envelope is deleted).
    pub object: &'static str,
}

/// What the envelope says about an object.
#[derive(Clone, Copy, Debug)]
pub struct State {
    /// The revision.
    pub rev: u64,
    /// When it went to the trash.
    pub deleted: Option<i64>,
    /// When the trash period is over.
    pub purge_after: Option<i64>,
}

/// Insert the envelope of a new object, `rev = 1`.
///
/// # Errors
/// `Invalid` (a value does not fit its column) or the engine's error.
pub async fn insert(
    c: &Conn<'_>,
    id: ObjectId,
    kind: &str,
    name: &str,
    owner: &PersonRef,
    now: i64,
) -> Result<()> {
    let [pid, part, pabzug] = person_values(owner);
    let n = Value::Null;
    c.insert(
        &OBJECTS,
        vec![
            id_value(id),
            n.clone(),
            text(kind),
            text(name),
            text(&name.to_lowercase()),
            pid,
            part,
            pabzug,
            int(now),
            int(now),
            n.clone(),
            n.clone(),
            n.clone(),
            int(1),
            n.clone(),
            n,
        ],
    )
    .await
}

/// The envelope state of an object, `None` when there is no such row.
///
/// # Errors
/// The engine's error, or a damaged row.
pub async fn state(c: &Conn<'_>, id: ObjectId) -> Result<Option<State>> {
    let rows = c
        .query(
            "SELECT rev, deleted, purge_after FROM objects WHERE id = ?",
            vec![id_value(id)],
        )
        .await?;
    rows.first()
        .map(|r| {
            Ok(State {
                rev: u64::try_from(get_int(r, 0)?).unwrap_or(0),
                deleted: get_opt_int(r, 1)?,
                purge_after: get_opt_int(r, 2)?,
            })
        })
        .transpose()
}

/// Why a compare-and-set did not hit: the row changed, or it is gone or in the trash.
pub(super) async fn missed(c: &Conn<'_>, id: ObjectId) -> Error {
    match state(c, id).await {
        Ok(Some(s)) if s.deleted.is_none() => Error::Changed {
            current: Revision(s.rev),
        },
        Ok(_) => Error::NotFound(id),
        Err(e) => e,
    }
}

/// Compare-and-set: `rev + 1` only if the row is at `expected` and not in the trash.
///
/// # Errors
/// `NotFound` (no row, or in the trash), `Changed { current }` (someone wrote before).
pub async fn bump(
    c: &Conn<'_>,
    id: ObjectId,
    expected: Revision,
    name: Option<&str>,
    now: i64,
) -> Result<Revision> {
    let exp = int(i64::try_from(expected.0).unwrap_or(i64::MAX));
    let n = if let Some(n) = name {
        let sql = "UPDATE objects SET rev = rev + 1, updated = ?, name = ?, name_sort = ? WHERE id = ? AND rev = ? AND deleted IS NULL";
        c.exec(
            sql,
            vec![
                int(now),
                text(n),
                text(&n.to_lowercase()),
                id_value(id),
                exp,
            ],
        )
        .await?
    } else {
        let sql = "UPDATE objects SET rev = rev + 1, updated = ? WHERE id = ? AND rev = ? AND deleted IS NULL";
        c.exec(sql, vec![int(now), id_value(id), exp]).await?
    };
    if n == 1 {
        Ok(Revision(expected.0 + 1))
    } else {
        Err(missed(c, id).await)
    }
}

/// A system run changed the object's data: `rev + 1` and `updated`, without an expected revision (no caller holds one),
/// and the envelope follows the data (`name` and `name_sort` when given). Works on an object in the trash too (a period
/// run scrubs what the trash held).
///
/// # Errors
/// `NotFound` when there is no such row.
pub async fn touch(c: &Conn<'_>, id: ObjectId, name: Option<&str>, now: i64) -> Result<Revision> {
    let s = state(c, id).await?.ok_or(Error::NotFound(id))?;
    if let Some(n) = name {
        let sql =
            "UPDATE objects SET rev = rev + 1, updated = ?, name = ?, name_sort = ? WHERE id = ?";
        c.exec(
            sql,
            vec![int(now), text(n), text(&n.to_lowercase()), id_value(id)],
        )
        .await?;
    } else {
        c.exec(
            "UPDATE objects SET rev = rev + 1, updated = ? WHERE id = ?",
            vec![int(now), id_value(id)],
        )
        .await?;
    }
    Ok(Revision(s.rev + 1))
}

/// Final delete (end of the trash, ADR 0073): the object leaves with every row and its log. Only for an object in the
/// trash. The envelopes of the dependent objects that the domain reports (`Domain::children`, e.g. the exceptions of a
/// series) go first; the rows of the domain's own tables go by their foreign keys (`ON DELETE CASCADE`).
///
/// # Errors
/// `NotFound`, or `Invalid` when the object is not in the trash.
pub async fn delete_final(c: &Conn<'_>, d: &Domain, id: ObjectId) -> Result<()> {
    let s = state(c, id).await?.ok_or(Error::NotFound(id))?;
    if s.deleted.is_none() {
        return Err(invalid("id", "not in the trash"));
    }
    // The dependents, then theirs, until nothing new turns up (a task with subtasks); every one leaves with its marks.
    let mut all = vec![id];
    let mut at = 0;
    while at < all.len() {
        let parent = all[at];
        at += 1;
        for ch in d.children {
            let sql = format!(
                "SELECT {} FROM {} WHERE {} = ?",
                ch.object, ch.table, ch.parent
            );
            for r in c.query(&sql, vec![id_value(parent)]).await? {
                let found = super::codec::get_id(&r, 0)?;
                if !all.contains(&found) {
                    all.push(found);
                }
            }
        }
    }
    for gone in all.iter().rev() {
        c.exec("DELETE FROM objects WHERE id = ?", vec![id_value(*gone)])
            .await?;
        for t in ["purged_objects", "unlogged_ids"] {
            c.exec(
                &format!("DELETE FROM {t} WHERE object_id = ?"),
                vec![id_value(*gone)],
            )
            .await?;
        }
    }
    Ok(())
}

/// The ids of the objects whose trash period is over at `now`.
///
/// # Errors
/// The engine's error, or a damaged row.
pub async fn due(c: &Conn<'_>, now: i64) -> Result<Vec<ObjectId>> {
    let rows = c
        .query(
            "SELECT id FROM objects WHERE purge_after IS NOT NULL AND purge_after <= ? ORDER BY id",
            vec![int(now)],
        )
        .await?;
    rows.iter().map(|r| super::codec::get_id(r, 0)).collect()
}
