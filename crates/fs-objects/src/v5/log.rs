//! The typed log: one `activity` row per action and one `activity_change` row per changed field (R10).
//!
//! Pattern: Transaction script (in the caller's transaction). Log purge (ADR 0029) removes the entries, never the
//! object; ids whose writes are not logged stand in `unlogged_ids`.

use fs_types::{ObjectId, PersonRef};
use turso::Value;

use super::codec::{
    damaged, get_opt_text, get_text, id_text, id_value, int, opt_text, person_values, text,
};
use super::conn::Conn;
use super::envelope::state;
use super::error::{Error, Result};
use super::id::new_id;
use super::log_codec::{LogValue, decode, encode};
use super::tables::ACTIVITY_CHANGE;
use super::tables_kind::{ACTIVITY_KINDS, ACTIVITY_V2, SET};

/// The kind of an activity (AS2 names).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Made.
    Create,
    /// Changed.
    Update,
    /// Deleted (to the trash).
    Delete,
    /// Taken back.
    Undo,
    /// Invited.
    Invite,
    /// Accepted.
    Accept,
    /// Accepted with reservation.
    TentativeAccept,
    /// Declined.
    Reject,
    /// Something was added to a collection (v0.0.5).
    Add,
    /// Something was taken out of a collection (v0.0.5).
    Remove,
    /// A person joined (v0.0.5).
    Join,
    /// A person left (v0.0.5).
    Leave,
    /// Something was moved (v0.0.5).
    Move,
}

const ALL: [Kind; 13] = [
    Kind::Create,
    Kind::Update,
    Kind::Delete,
    Kind::Undo,
    Kind::Invite,
    Kind::Accept,
    Kind::TentativeAccept,
    Kind::Reject,
    Kind::Add,
    Kind::Remove,
    Kind::Join,
    Kind::Leave,
    Kind::Move,
];

impl Kind {
    /// The stored word (`activity.kind`).
    #[must_use]
    pub fn word(self) -> &'static str {
        ACTIVITY_KINDS[ALL.iter().position(|k| *k == self).unwrap_or(0)]
    }

    /// The kind of a stored word.
    ///
    /// # Errors
    /// `Unavailable` for a word that is none (a damaged row).
    pub fn from_word(w: &str) -> Result<Self> {
        ACTIVITY_KINDS
            .iter()
            .position(|k| *k == w)
            .map(|i| ALL[i])
            .ok_or_else(|| damaged("activity kind"))
    }
}

/// One changed field.
#[derive(Clone, Debug, PartialEq)]
pub struct FieldChange {
    /// The field (a program's name, e.g. `title` or `override.title`).
    pub field: String,
    /// The row it belongs to when the field is in a list.
    pub row_key: Option<String>,
    /// The value before.
    pub old: Option<LogValue>,
    /// The value after.
    pub new: Option<LogValue>,
}

/// Write one log entry with its typed changes (nothing for an object in `unlogged_ids`).
///
/// # Errors
/// `Invalid` (a value does not fit) or the engine's error.
pub async fn log(
    c: &Conn<'_>,
    id: ObjectId,
    kind: Kind,
    actor: &PersonRef,
    now: i64,
    changes: &[FieldChange],
) -> Result<()> {
    let unlogged = c
        .query(
            "SELECT 1 FROM unlogged_ids WHERE object_id = ?",
            vec![id_value(id)],
        )
        .await?;
    if !unlogged.is_empty() {
        return Ok(());
    }
    let aid = id_text(new_id(now)?);
    let [pid, part, pabzug] = person_values(actor);
    let row = vec![
        text(&aid),
        text(kind.word()),
        id_value(id),
        pid,
        part,
        pabzug,
        int(now),
        Value::Null,
        int(0),
        int(0),
        int(1),
        text(SET),
    ];
    c.insert(&ACTIVITY_V2, row).await?;
    for (seq, ch) in changes.iter().enumerate() {
        let (ko, old) = encode(ch.old.as_ref())?;
        let (kn, new) = encode(ch.new.as_ref())?;
        let vk = kn
            .or(ko)
            .ok_or_else(|| super::codec::invalid("changes", "a change without any value"))?;
        let mut row = vec![
            text(&aid),
            int(i64::try_from(seq).unwrap_or(0)),
            text(&ch.field),
            opt_text(ch.row_key.as_deref()),
            text(vk),
        ];
        row.extend(old);
        row.extend(new);
        c.insert(&ACTIVITY_CHANGE, row).await?;
    }
    let sql = "INSERT INTO meta (key, value) VALUES ('activity_high', ?) ON CONFLICT (key) DO UPDATE SET value = excluded.value";
    c.exec(sql, vec![text(&aid)]).await.map(|_| ())
}

/// The changes of one activity, in order.
///
/// # Errors
/// `Unavailable` for a row that does not read back.
pub async fn changes_of(c: &Conn<'_>, activity: &str) -> Result<Vec<FieldChange>> {
    let names: Vec<&str> = ACTIVITY_CHANGE.cols.iter().map(|x| x.name).collect();
    let sql = format!(
        "SELECT {} FROM activity_change WHERE activity = ? ORDER BY seq",
        names.join(", ")
    );
    let mut out = Vec::new();
    for r in c.query(&sql, vec![text(activity)]).await? {
        let kind = get_text(&r, 4)?;
        out.push(FieldChange {
            field: get_text(&r, 2)?,
            row_key: get_opt_text(&r, 3)?,
            old: decode(&kind, &r, 5)?,
            new: decode(&kind, &r, 15)?,
        });
    }
    Ok(out)
}

/// Log purge (ADR 0029): remove the log of an object, keep the object, remember the id in `purged_objects`.
///
/// # Errors
/// `NotFound` when there is no such object.
pub async fn purge_log(c: &Conn<'_>, id: ObjectId) -> Result<()> {
    state(c, id).await?.ok_or(Error::NotFound(id))?;
    c.exec("DELETE FROM activity WHERE object = ?", vec![id_value(id)])
        .await?;
    let sql =
        "INSERT INTO purged_objects (object_id) VALUES (?) ON CONFLICT (object_id) DO NOTHING";
    c.exec(sql, vec![id_value(id)]).await.map(|_| ())
}

/// From now on the writes of this object are not logged (as v4 `unlogged_ids`).
///
/// # Errors
/// `NotFound` when there is no such object.
pub async fn mark_unlogged(c: &Conn<'_>, id: ObjectId) -> Result<()> {
    state(c, id).await?.ok_or(Error::NotFound(id))?;
    let sql = "INSERT INTO unlogged_ids (object_id) VALUES (?) ON CONFLICT (object_id) DO NOTHING";
    c.exec(sql, vec![id_value(id)]).await.map(|_| ())
}

/// Mark the entries of an object that name one list row as redacted and drop their changes: what a program does when
/// a part of the data is forgotten for good (ADR 0073 data minimisation). `field_like` is a SQL `LIKE` pattern.
///
/// # Errors
/// The engine's error.
pub async fn redact(c: &Conn<'_>, id: ObjectId, row_key: &str, field_like: &str) -> Result<()> {
    let hit = "SELECT activity FROM activity_change WHERE row_key = ? AND field LIKE ? AND activity IN (SELECT id FROM activity WHERE object = ?)";
    let params = vec![text(row_key), text(field_like), id_value(id)];
    c.exec(
        &format!("UPDATE activity SET redacted = 1, explained = 0 WHERE id IN ({hit})"),
        params.clone(),
    )
    .await?;
    c.exec(
        "DELETE FROM activity_change WHERE row_key = ? AND field LIKE ? AND activity IN (SELECT id FROM activity WHERE object = ?)",
        params,
    )
    .await
    .map(|_| ())
}
