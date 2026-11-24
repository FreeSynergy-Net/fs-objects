//! Compare-and-set for `write_by`: the `expect` check, the last change of an object and the paths that clash.
//!
//! Pattern: none (pure functions over the log) — kept apart from the unit of work so each file stays small.

use turso::{Connection, Value};

use super::codec::decode_activity;
use super::schema::ACTIVITY_COLS;
use super::sql::collect;
use crate::{Activity, FieldClash, ObjectId, Outcome, Result, StoredObject};

const STATE_KINDS: &str = "kind IN ('Create','Update','Delete','Undo')";

/// The last state-changing activity of an object.
async fn last_change(c: &Connection, id: ObjectId) -> Result<Option<Activity>> {
    let rows = collect(
        c,
        &format!(
            "SELECT {ACTIVITY_COLS} FROM activities WHERE object_id = ? AND {STATE_KINDS} \
             ORDER BY id DESC LIMIT 1"
        ),
        vec![Value::Text(id.to_string())],
    )
    .await?;
    rows.first().map(|r| decode_activity(r)).transpose()
}

/// The image of an object as comparable JSON (without what changes with every write).
fn comparable(o: &StoredObject) -> Result<serde_json::Value> {
    let mut v = serde_json::to_value(o)?;
    if let Some(m) = v.as_object_mut() {
        m.remove("owner");
        m.remove("rev");
        m.remove("original");
        if let Some(e) = m.get_mut("envelope").and_then(|e| e.as_object_mut()) {
            e.remove("updated");
        }
    }
    Ok(v)
}

fn diff_paths(a: &serde_json::Value, b: &serde_json::Value, path: &str, out: &mut Vec<FieldClash>) {
    if let (Some(x), Some(y)) = (a.as_object(), b.as_object()) {
        let mut keys: Vec<&String> = x.keys().chain(y.keys()).collect();
        keys.sort();
        keys.dedup();
        for k in keys {
            let sub = if path.is_empty() {
                k.clone()
            } else {
                format!("{path}.{k}")
            };
            let null = serde_json::Value::Null;
            diff_paths(
                x.get(k).unwrap_or(&null),
                y.get(k).unwrap_or(&null),
                &sub,
                out,
            );
        }
    } else if a != b {
        out.push(FieldClash { path: path.into() });
    }
}

/// The paths whose value changed since the caller saw revision `expect`: the old values are the `before` image of
/// the activity with `revision == expect + 1`. If it is gone (compacted, redacted), the list stays empty.
async fn clash_fields(
    c: &Connection,
    id: ObjectId,
    expect: u64,
    current: Option<&StoredObject>,
) -> Result<Vec<FieldClash>> {
    let Some(current) = current else {
        return Ok(vec![]);
    };
    let rows = collect(
        c,
        &format!(
            "SELECT {ACTIVITY_COLS} FROM activities WHERE object_id = ? AND revision = ? AND {STATE_KINDS} \
             ORDER BY id LIMIT 1"
        ),
        vec![Value::Text(id.to_string()), Value::Integer((expect + 1) as i64)],
    )
    .await?;
    let Some(act) = rows.first().map(|r| decode_activity(r)).transpose()? else {
        return Ok(vec![]);
    };
    let Ok(cs) = act.change_set() else {
        return Ok(vec![]);
    };
    let Some(old) = cs.before else {
        return Ok(vec![]);
    };
    let mut out = Vec::new();
    diff_paths(&comparable(&old)?, &comparable(current)?, "", &mut out);
    Ok(out)
}

/// `Some(outcome)` if `expect` does not match the stored revision.
pub(super) async fn check_expect(
    c: &Connection,
    id: ObjectId,
    current: Option<&StoredObject>,
    expect: Option<u64>,
) -> Result<Option<Outcome>> {
    let Some(e) = expect else {
        return Ok(None);
    };
    if current.map(|o| o.rev) == Some(e) {
        return Ok(None);
    }
    let Some(by) = last_change(c, id).await? else {
        return Ok(Some(Outcome::Gone));
    };
    let fields = clash_fields(c, id, e, current).await?;
    Ok(Some(Outcome::Conflict {
        by: Box::new(by),
        fields,
    }))
}
