//! The storage classes of a value as columns, and the shared steps of the writing functions. Split from `rows`.
//!
//! Pattern: Adapter (typed values to engine values and back).

use turso::Value;

use super::Prim;
use crate::v5::codec::{Row, damaged, get_int, get_text, int, text};
use crate::v5::conn::Conn;
use crate::v5::error::Result;

/// `(kind, v_int, v_text)` of a value; all three null for `None`.
pub(super) fn cols(p: Option<&Prim>) -> [Value; 3] {
    match p {
        None => [Value::Null, Value::Null, Value::Null],
        Some(Prim::Bool(b)) => [text("bool"), int(i64::from(*b)), Value::Null],
        Some(Prim::Int(i)) => [text("int"), int(*i), Value::Null],
        Some(Prim::Text(t)) => [text("text"), Value::Null, text(t)],
    }
}

/// The value in the three columns from `at` on; a null kind is `None`.
pub(super) fn prim_at(r: &Row, at: usize) -> Result<Option<Prim>> {
    if matches!(r.get(at), Some(Value::Null)) {
        return Ok(None);
    }
    Ok(Some(match get_text(r, at)?.as_str() {
        "bool" => Prim::Bool(get_int(r, at + 1)? != 0),
        "int" => Prim::Int(get_int(r, at + 1)?),
        "text" => Prim::Text(get_text(r, at + 2)?),
        _ => return Err(damaged("value kind")),
    }))
}

/// `rev = rev + 1`; the new revision.
pub(super) async fn bump(c: &Conn<'_>) -> Result<i64> {
    c.run("UPDATE settings_rev SET rev = rev + 1 WHERE slot = 0")
        .await?;
    revision(c).await
}

/// The revision now.
pub(super) async fn revision(c: &Conn<'_>) -> Result<i64> {
    let rows = c
        .query("SELECT rev FROM settings_rev WHERE slot = 0", Vec::new())
        .await?;
    rows.first()
        .map_or_else(|| Err(damaged("settings_rev")), |r| get_int(r, 0))
}

/// The id of this store (`meta.store_id`): the device of `settings_active`.
pub(super) async fn device(c: &Conn<'_>) -> Result<String> {
    let rows = c
        .query("SELECT value FROM meta WHERE key = 'store_id'", Vec::new())
        .await?;
    rows.first()
        .map_or_else(|| Err(damaged("meta.store_id")), |r| get_text(r, 0))
}
