//! Activity-log rows: load, insert, and the compact step view used by undo.
//!
//! Pattern: Repository (internal) — the only module that knows the `activities` table layout.

use fs_types::BasicValue as _;
use turso::{Connection, Value};

use super::codec::decode_activity;
use super::rows::{int, opt_text, text};
use super::schema::ACTIVITY_COLS;
use super::sql::{collect, exec};
use crate::history::Step;
use crate::{Activity, ActivityId, Result, art_word};

/// Load one activity by id.
pub(super) async fn load_activity(c: &Connection, id: ActivityId) -> Result<Option<Activity>> {
    let rows = collect(
        c,
        &format!("SELECT {ACTIVITY_COLS} FROM activities WHERE id = ?"),
        vec![Value::Text(id.to_string())],
    )
    .await?;
    rows.first().map(|r| decode_activity(r)).transpose()
}

/// Append an activity row.
pub(super) async fn insert_activity(c: &Connection, a: &Activity) -> Result<()> {
    exec(
        c,
        &format!(
            "INSERT INTO activities ({ACTIVITY_COLS}) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)"
        ),
        vec![
            Value::Text(a.id.to_string()),
            Value::Text(a.kind.as_str().into()),
            Value::Text(a.object.to_string()),
            Value::Text(a.actor.clone()),
            Value::Integer(a.at.timestamp_micros()),
            a.target.map_or(Value::Null, |t| Value::Text(t.to_string())),
            Value::Text(a.payload.to_string()),
            Value::Integer(i64::from(a.synced)),
            Value::Text(
                a.owner
                    .as_ref()
                    .map_or_else(String::new, |p| p.id.canonical()),
            ),
            Value::Text(
                a.owner
                    .as_ref()
                    .map_or_else(String::new, |p| art_word(p.art).into()),
            ),
            Value::Text(a.origin.program.clone()),
            Value::Text(a.origin.instance.clone()),
            Value::Text(a.origin.device.clone()),
            a.context.clone().map_or(Value::Null, Value::Text),
            a.step.clone().map_or(Value::Null, Value::Text),
            a.unit.map_or(Value::Null, |u| Value::Text(u.to_string())),
            Value::Integer(a.revision as i64),
        ],
    )
    .await?;
    Ok(())
}

/// The state-changing steps of one actor in log order (input of `history::compute`).
pub(super) async fn load_steps(c: &Connection, actor: &str) -> Result<Vec<Step>> {
    let rows = collect(
        c,
        "SELECT id, kind, target, redacted FROM activities \
         WHERE actor = ? AND kind IN ('Create','Update','Delete','Undo') ORDER BY id",
        vec![Value::Text(actor.into())],
    )
    .await?;
    rows.iter()
        .map(|r| {
            Ok(Step {
                id: text(&r[0])?.parse()?,
                kind: text(&r[1])?.parse()?,
                target: opt_text(&r[2])?.map(|s| s.parse()).transpose()?,
                redacted: int(&r[3])? != 0,
            })
        })
        .collect()
}

/// Highest activity id ever handed out: the log's maximum or the remembered high-water mark, whichever
/// is higher (purge and compaction can remove the newest entry; the hub has already accepted its id).
pub(super) async fn last_activity_id(c: &Connection) -> Result<Option<ActivityId>> {
    let mut best: Option<ActivityId> = None;
    for sql in [
        "SELECT MAX(id) FROM activities",
        "SELECT value FROM meta WHERE key = 'activity_high'",
    ] {
        let rows = collect(c, sql, vec![]).await?;
        if let Some(Value::Text(s)) = rows.first().and_then(|r| r.first()) {
            let id: ActivityId = s.parse()?;
            best = best.max(Some(id));
        }
    }
    Ok(best)
}

/// Remember the current highest activity id before entries are removed, so a reopen cannot hand it out again.
pub(super) async fn remember_high_water(c: &Connection) -> Result<()> {
    if let Some(h) = last_activity_id(c).await? {
        exec(c, "DELETE FROM meta WHERE key = 'activity_high'", vec![]).await?;
        exec(
            c,
            "INSERT INTO meta (key, value) VALUES ('activity_high', ?)",
            vec![Value::Text(h.to_string())],
        )
        .await?;
    }
    Ok(())
}

/// Before compaction removes log rows: remember the highest revision of every object that no longer exists (meta key
/// `rev:<id>`), so [`last_revision`] still knows it when the id is written again (no ABA after compaction).
pub(super) async fn remember_revisions(c: &Connection) -> Result<()> {
    exec(
        c,
        "INSERT OR REPLACE INTO meta (key, value) \
         SELECT 'rev:' || object_id, CAST(MAX(revision) AS TEXT) FROM activities \
         WHERE revision > 0 AND object_id NOT IN (SELECT id FROM objects) GROUP BY object_id",
        vec![],
    )
    .await?;
    Ok(())
}

/// Mark an inserted activity as written without images (see `purge`).
pub(super) async fn mark_redacted(c: &Connection, id: ActivityId) -> Result<()> {
    exec(
        c,
        "UPDATE activities SET redacted = 1 WHERE id = ?",
        vec![Value::Text(id.to_string())],
    )
    .await?;
    Ok(())
}

/// The highest revision the log ever recorded for `object` (0 if none): a recreated object continues from here, so
/// an old `expect` can never match a later incarnation (no ABA).
pub(super) async fn last_revision(c: &Connection, object: crate::ObjectId) -> Result<u64> {
    let rows = collect(
        c,
        "SELECT COALESCE(MAX(revision), 0) FROM activities WHERE object_id = ?",
        vec![Value::Text(object.to_string())],
    )
    .await?;
    let logged = match rows.first() {
        Some(r) => int(&r[0])? as u64,
        None => 0,
    };
    let marks = collect(
        c,
        "SELECT value FROM meta WHERE key = ?",
        vec![Value::Text(format!("rev:{object}"))],
    )
    .await?;
    let marked = marks
        .first()
        .and_then(|r| text(&r[0]).ok())
        .and_then(|t| t.parse::<u64>().ok())
        .unwrap_or(0);
    Ok(logged.max(marked))
}
