//! `check::typed`: does a v5 store file really keep the typed rules of ADR 0086? It reads the REAL catalogue of the
//! engine (`sqlite_master`, `PRAGMA table_info`) and the data, not what a program declared. It returns findings (it
//! does not stop at the first), so a report can list them all.
//!
//! Rules: (1) every table is `STRICT`; (2) no text column holds JSON (heuristic: starts with `{` or `[` and parses; a column of the logical type
//! `free_text`, which a person writes into, is exempt: `[1,2]` there is a note, not JSON), and no column has the type
//! `ANY`;
//! (3) no `value_kind object` blob: a logged `object` value must be an object id; (4) time columns are `INTEGER`
//! and hold integers; (5) a `REF` column that is not the table's own `id` has a foreign key, except the person
//! snapshot (`*_abzug`) and the core columns in [`REF_WITHOUT_FK`], where the program's provider checks; a person
//! IRI is no `REF` and needs none; (6) no blob except the ones ADR 0086 item 4 names ([`BLOB_ALLOWED`]; the nonce
//! belongs to the cipher blob); (7) every table is registered in `fs_column`.
//!
//! Pattern: Specification (small independent rules over the catalogue; each adds findings).

use std::collections::HashSet;
use std::fmt;

use turso::Value;

use super::codec::{get_int, get_text, parse_id, text};
use super::conn::Conn;
use super::error::Result;
use super::store::Store;

/// The blob columns ADR 0086 item 4 allows: the received original, the cipher (its nonce is part of it, decision of
/// 2026-11-17) and the rebuildable collaboration buffer.
pub const BLOB_ALLOWED: &[(&str, &str)] = &[
    ("object_original", "bytes"),
    ("*", "cipher"),
    ("collab_buffer", "loro"),
];

/// `REF` columns of the shared tables without a foreign key on purpose (the object may live in another store, or
/// the log outlives it); the program's provider checks them and reports a missing target visibly.
pub const REF_WITHOUT_FK: &[(&str, &str)] = &[
    ("objects", "context"),
    ("objects", "in_reply_to"),
    ("objects", "describes"),
    ("activity", "target"),
    ("purged_objects", "object_id"),
    ("unlogged_ids", "object_id"),
];

/// Core columns that hold text a person wrote (the object name and its sort key, the logged old and new text of any field): they
/// are exempt from the JSON heuristic like a column of the logical type `free_text`. They keep the label `text` in
/// `fs_column`, because a recorded label is part of what a file was built with (changing it would refuse old files).
pub const FREE_TEXT_CORE: &[(&str, &str)] = &[
    ("objects", "name"),
    ("objects", "name_sort"),
    ("activity_change", "old_text"),
    ("activity_change", "new_text"),
];

/// Which rule a finding is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rule {
    /// A table is not `STRICT`.
    NotStrict,
    /// A text column holds JSON.
    JsonInText,
    /// A column has the type `ANY` (a `STRICT` table accepts it, but then it is not typed).
    AnyType,
    /// A logged `object` value is not an object id.
    ObjectBlob,
    /// A time column is not `INTEGER` or holds something else.
    TimeNotInteger,
    /// A `REF` column has no foreign key.
    MissingForeignKey,
    /// A blob outside the allowed ones.
    BlobNotAllowed,
    /// A table is not in the registry.
    Unregistered,
}

/// One finding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    /// The rule.
    pub rule: Rule,
    /// The table.
    pub table: String,
    /// The column, when the finding is about one.
    pub column: Option<String>,
}

impl fmt::Display for Finding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.column {
            Some(c) => write!(f, "{:?}: {}.{c}", self.rule, self.table),
            None => write!(f, "{:?}: {}", self.rule, self.table),
        }
    }
}

struct Column {
    name: String,
    ty: String,
    pk: bool,
}

fn found(out: &mut Vec<Finding>, rule: Rule, table: &str, column: Option<&str>) {
    out.push(Finding {
        rule,
        table: table.to_owned(),
        column: column.map(str::to_owned),
    });
}

/// Columns named in `FOREIGN KEY (a, b)` clauses of a `CREATE TABLE` text (the engine's `foreign_key_list` is not
/// relied on).
fn fk_columns(sql: &str) -> HashSet<String> {
    let mut out = HashSet::new();
    let upper = sql.to_uppercase();
    let mut from = 0;
    while let Some(i) = upper[from..].find("FOREIGN KEY") {
        let start = from + i + "FOREIGN KEY".len();
        from = start;
        if let Some(open) = sql[start..].find('(')
            && let Some(close) = sql[start + open..].find(')')
        {
            let inner = &sql[start + open + 1..start + open + close];
            out.extend(inner.split(',').map(|c| c.trim().to_owned()));
        }
    }
    out
}

async fn columns(c: &Conn<'_>, table: &str) -> Result<Vec<Column>> {
    let rows = c
        .query(&format!("PRAGMA table_info({table})"), Vec::new())
        .await?;
    rows.iter()
        .map(|r| {
            Ok(Column {
                name: get_text(r, 1)?,
                ty: get_text(r, 2)?.to_uppercase(),
                pk: get_int(r, 5)? > 0,
            })
        })
        .collect()
}

fn blob_ok(table: &str, col: &str) -> bool {
    BLOB_ALLOWED
        .iter()
        .any(|(t, c)| (*t == "*" || *t == table) && *c == col)
}

async fn json_cells(c: &Conn<'_>, table: &str, col: &str) -> Result<bool> {
    let sql = format!("SELECT {col} FROM {table} WHERE substr({col}, 1, 1) IN ('{{', '[')");
    for row in c.query(&sql, Vec::new()).await? {
        if let Some(Value::Text(v)) = row.first()
            && matches!(
                serde_json::from_str::<serde_json::Value>(v),
                Ok(serde_json::Value::Object(_) | serde_json::Value::Array(_))
            )
        {
            return Ok(true);
        }
    }
    Ok(false)
}

async fn registered(c: &Conn<'_>) -> Result<Vec<(String, String, String)>> {
    let rows = c
        .query("SELECT tbl, col, logical FROM fs_column", Vec::new())
        .await?;
    rows.iter()
        .map(|r| Ok((get_text(r, 0)?, get_text(r, 1)?, get_text(r, 2)?)))
        .collect()
}

/// Check the typed rules over a whole v5 store file.
///
/// # Errors
/// The engine's error (the findings are the normal result, not an error).
pub async fn typed(store: &Store) -> Result<Vec<Finding>> {
    let c = store.read().await;
    let mut out = Vec::new();
    let reg = registered(&c).await?;
    let tables = c
        .query(
            "SELECT name, sql FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name",
            Vec::new(),
        )
        .await?;
    for t in &tables {
        let (name, sql) = (get_text(t, 0)?, get_text(t, 1)?);
        if name.starts_with("__") {
            continue;
        }
        if !sql.trim_end().to_uppercase().ends_with("STRICT") {
            found(&mut out, Rule::NotStrict, &name, None);
        }
        if !reg.iter().any(|(tb, _, _)| *tb == name) {
            found(&mut out, Rule::Unregistered, &name, None);
        }
        let fks = fk_columns(&sql);
        for col in columns(&c, &name).await? {
            let logical = reg
                .iter()
                .find(|(tb, cl, _)| *tb == name && *cl == col.name)
                .map(|(_, _, l)| l.as_str());
            if col.ty == "BLOB" && !blob_ok(&name, &col.name) {
                found(&mut out, Rule::BlobNotAllowed, &name, Some(&col.name));
            }
            if col.ty == "ANY" {
                found(&mut out, Rule::AnyType, &name, Some(&col.name));
            }
            if col.ty == "TEXT"
                && logical != Some("free_text")
                && !FREE_TEXT_CORE.contains(&(name.as_str(), col.name.as_str()))
                && json_cells(&c, &name, &col.name).await?
            {
                found(&mut out, Rule::JsonInText, &name, Some(&col.name));
            }
            if matches!(logical, Some("utc" | "local" | "date")) {
                let sql = format!(
                    "SELECT 1 FROM {name} WHERE {0} IS NOT NULL AND typeof({0}) <> 'integer' LIMIT 1",
                    col.name
                );
                if col.ty != "INTEGER" || !c.query(&sql, Vec::new()).await?.is_empty() {
                    found(&mut out, Rule::TimeNotInteger, &name, Some(&col.name));
                }
            }
            let own_id = col.pk && col.name == "id";
            let exempt = own_id
                || col.name.ends_with("_abzug")
                || fks.contains(&col.name)
                || REF_WITHOUT_FK.contains(&(name.as_str(), col.name.as_str()));
            if logical == Some("ref") && !exempt {
                found(&mut out, Rule::MissingForeignKey, &name, Some(&col.name));
            }
        }
    }
    object_values(&c, &tables, &mut out).await?;
    Ok(out)
}

/// A logged `object` value is an object id of the own store, never a serialized object.
async fn object_values(c: &Conn<'_>, tables: &[Vec<Value>], out: &mut Vec<Finding>) -> Result<()> {
    if !tables
        .iter()
        .any(|t| matches!(t.first(), Some(Value::Text(n)) if n == "activity_change"))
    {
        return Ok(());
    }
    let rows = c
        .query(
            "SELECT old_text, new_text FROM activity_change WHERE value_kind = ?",
            vec![text("object")],
        )
        .await?;
    for (i, side) in ["old_text", "new_text"].into_iter().enumerate() {
        if rows
            .iter()
            .any(|r| matches!(r.get(i), Some(Value::Text(v)) if parse_id(v).is_none()))
        {
            found(out, Rule::ObjectBlob, "activity_change", Some(side));
        }
    }
    Ok(())
}
