//! Runtime checks of ADR 0086 against the REAL tables of the engine (`sqlite_master`, `PRAGMA table_info`), not
//! against what a program declared: `STRICT`, the declared columns in order with the declared storage class, no blob
//! except `object_original.bytes`, and (a heuristic, draft R14) no JSON text in a text column.
//!
//! Pattern: Specification (small checks over the catalogue; the registry runs them in the step's transaction).

use turso::Value;

use super::codec::{get_int, get_text, invalid, text};
use super::conn::Conn;
use super::error::Result;
use super::spec::{Logical, Table};

/// The one place where a blob is allowed (ADR 0086 item 4).
fn blob_allowed(t: &Table, col: &str) -> bool {
    t.name == "object_original" && col == "bytes"
}

/// Check the real structure of the declared tables.
///
/// # Errors
/// `Invalid` naming `table` or `table.column`.
pub async fn check_structure(c: &Conn<'_>, tables: &[Table]) -> Result<()> {
    for t in tables {
        let sql = c
            .query(
                "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = ?",
                vec![text(t.name)],
            )
            .await?;
        let Some(sql) = sql.first().map(|r| get_text(r, 0)).transpose()? else {
            return Err(invalid(t.name, "no such table"));
        };
        if !sql.trim_end().to_uppercase().ends_with("STRICT") {
            return Err(invalid(t.name, "not a STRICT table"));
        }
        let real = c
            .query(&format!("PRAGMA table_info({})", t.name), Vec::new())
            .await?;
        if real.len() != t.cols.len() {
            return Err(invalid(
                t.name,
                "the columns of the table differ from the declared ones",
            ));
        }
        for (row, col) in real.iter().zip(t.cols) {
            let (name, ty) = (get_text(row, 1)?, get_text(row, 2)?.to_uppercase());
            let field = format!("{}.{}", t.name, col.name);
            if name != col.name || ty != col.logical.sql() {
                return Err(invalid(
                    &field,
                    "name or storage class differs from the declaration",
                ));
            }
            let pk = get_int(row, 5)? > 0;
            if !col.null && get_int(row, 3)? != 1 && !pk {
                return Err(invalid(
                    &field,
                    "declared required, but the table allows empty",
                ));
            }
            if ty == "BLOB" && !blob_allowed(t, col.name) {
                return Err(invalid(&field, "a blob outside object_original.bytes"));
            }
            if ty.contains("JSON") {
                return Err(invalid(&field, "a JSON column"));
            }
        }
    }
    Ok(())
}

/// Look for JSON in the text columns of the tables (an object or array that parses): what a migration step must not
/// leave behind. A heuristic (draft R14): it looks at the first rows that start with `{` or `[`.
///
/// # Errors
/// `Invalid` naming `table.column`.
pub async fn json_in_text(c: &Conn<'_>, tables: &[Table]) -> Result<()> {
    for t in tables {
        for col in t.cols.iter().filter(|c| c.logical == Logical::Text) {
            let sql = format!(
                "SELECT {0} FROM {1} WHERE substr({0}, 1, 1) IN ('{{', '[') LIMIT 50",
                col.name, t.name
            );
            for row in c.query(&sql, Vec::new()).await? {
                if let Some(Value::Text(v)) = row.first() {
                    let json = serde_json::from_str::<serde_json::Value>(v);
                    if matches!(
                        json,
                        Ok(serde_json::Value::Object(_) | serde_json::Value::Array(_))
                    ) {
                        return Err(invalid(
                            &format!("{}.{}", t.name, col.name),
                            "JSON in a text column",
                        ));
                    }
                }
            }
        }
    }
    Ok(())
}
