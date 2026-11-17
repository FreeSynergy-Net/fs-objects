//! The per-program part of a migration: v4 object in, typed rows out, and the way back for the round trip.
//!
//! Pattern: Strategy (one [`Converter`] per program; the scaffold knows nothing of any domain).

use async_trait::async_trait;
use turso::Value;

use crate::StoredObject;
use crate::v5::log::FieldChange;
use crate::v5::{Conn, Domain, Result, Table};

/// One row for one table, values in column order.
#[derive(Debug, Clone)]
pub struct TableRow {
    /// The table.
    pub table: &'static Table,
    /// The values, one per column.
    pub values: Vec<Value>,
}

/// What a converter makes of one object.
#[derive(Debug, Clone, Default)]
pub struct Converted {
    /// The typed rows, parents before children (the envelope row exists already).
    pub rows: Vec<TableRow>,
    /// The field changes of the object's first log entry (what the object "was made of").
    pub changes: Vec<FieldChange>,
}

/// An object the converter cannot carry over without loss: the reason is shown in the report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hold(pub String);

/// The converter of one program's objects (v4 JSON content to typed rows and back).
#[async_trait]
pub trait Converter: Send + Sync {
    /// The program's table family; the new v5 file registers exactly this domain.
    fn domain(&self) -> Domain;

    /// Whether this converter knows the object type.
    fn handles(&self, object_type: &str) -> bool;

    /// The typed rows of an object. A field the typed tables cannot hold is a [`Hold`], never dropped.
    ///
    /// # Errors
    /// [`Hold`] with the reason.
    fn to_rows(&self, object: &StoredObject) -> std::result::Result<Converted, Hold>;

    /// The content as it is rebuilt from the rows that are really in the new file (the way back of the round trip).
    ///
    /// # Errors
    /// The engine's error or a damaged row.
    async fn export(&self, conn: &Conn<'_>, id: fs_types::ObjectId) -> Result<serde_json::Value>;

    /// Whether this converter carries the envelope's `start_time` and `end_time` in its typed rows (and gives them
    /// back through [`Converter::export_times`]). When it does not, an object that has one of them is HELD BACK:
    /// the times are never dropped silently. The default carries nothing.
    fn carries_times(&self) -> bool {
        false
    }

    /// The `start_time` and `end_time` (UTC milliseconds) as they are in the rows of the new file; only called when
    /// [`Converter::carries_times`] is true. The round trip compares them with the v4 envelope.
    ///
    /// # Errors
    /// The engine's error or a damaged row.
    async fn export_times(
        &self,
        _conn: &Conn<'_>,
        _id: fs_types::ObjectId,
    ) -> Result<(Option<i64>, Option<i64>)> {
        Ok((None, None))
    }

    /// The media type (`content_type`) this converter's content has for an object type, which the way back
    /// reproduces. A v4 object with another `content_type` is HELD BACK (never rewritten silently). Default
    /// `application/json`.
    fn content_type(&self, _object_type: &str) -> String {
        "application/json".to_owned()
    }

    /// The documented normalization applied to the v4 content before it is compared with the export (for example
    /// an instant written with an offset is rewritten as UTC). The default changes nothing.
    fn normalize(&self, content: &serde_json::Value) -> serde_json::Value {
        content.clone()
    }
}
