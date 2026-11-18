//! Pattern: Strategy (a concrete [`Converter`]; a real program writes its own).
//!
//! The example converter for a small test domain `notes` (title, body, pinned, tags, optional due time). It shows
//! what a program's converter does; it is not a real program.
//!
//! v4 content: `{"title": text, "body": text, "pinned": bool, "tags": [text], "due": RFC 3339 text (optional),
//! "day": YYYY-MM-DD (optional)}`.
//! Documented normalization: `due` (an instant) is rewritten as UTC with milliseconds (`...Z`); the instant stays the
//! same. `day` (an all-day value) is a DATE and stays one: "a date stays a date" - it is never turned into an
//! instant and never shifted by a zone; a `day` that is not a plain date (for example with a time) is held back.

use async_trait::async_trait;
use chrono::{DateTime, SecondsFormat};
use fs_types::{BasicValue, Date, ObjectId, Utc as FsUtc};
use serde_json::{Value as Json, json};

use super::convert::{Converted, Converter, Hold, TableRow};
use crate::StoredObject;
use crate::v5::codec::{
    date_to_int, get_int, get_opt_int, get_text, id_value, int, int_to_date, opt_int, text,
};
use crate::v5::id::from_v4;
use crate::v5::log::FieldChange;
use crate::v5::log_codec::LogValue;
use crate::v5::spec::Logical::{Bool, Date as DateCol, FreeText, Int, Ref, Utc};
use crate::v5::{Col, Conn, Domain, Result, Step, Table};

/// One note.
pub const NOTE: Table = Table {
    name: "notes_note",
    cols: &[
        Col::req("object", Ref),
        Col::req("title", FreeText),
        Col::req("body", FreeText),
        Col::req("pinned", Bool),
        Col::opt("due", Utc),
        Col::opt("day", DateCol),
    ],
    constraints: &[
        "PRIMARY KEY (object)",
        "FOREIGN KEY (object) REFERENCES objects(id) ON DELETE CASCADE",
    ],
    indexes: &[],
};

/// The tags of a note, in order.
pub const TAG: Table = Table {
    name: "notes_tag",
    cols: &[
        Col::req("object", Ref),
        Col::req("seq", Int),
        Col::req("tag", FreeText),
    ],
    constraints: &[
        "PRIMARY KEY (object, seq)",
        "FOREIGN KEY (object) REFERENCES notes_note(object) ON DELETE CASCADE",
    ],
    indexes: &[],
};

const STEPS: [Step; 1] = [Step::Create(&[NOTE, TAG])];

/// The object type of a note.
pub const NOTE_TYPE: &str = "Note";

/// The converter of the `notes` test domain.
#[derive(Debug, Clone, Copy, Default)]
pub struct NotesConverter;

fn due_ms(s: &str) -> std::result::Result<i64, Hold> {
    DateTime::parse_from_rfc3339(s)
        .map(|d| d.timestamp_millis())
        .map_err(|_| Hold(format!("due is not an RFC 3339 instant: {s}")))
}

fn due_text(ms: i64) -> Option<String> {
    DateTime::from_timestamp_millis(ms).map(|d| d.to_rfc3339_opts(SecondsFormat::Millis, true))
}

#[async_trait]
impl Converter for NotesConverter {
    fn domain(&self) -> Domain {
        Domain::new("notes", "Notes (migration example)", &[NOTE, TAG], &STEPS)
    }

    fn handles(&self, t: &str) -> bool {
        t == NOTE_TYPE
    }

    fn to_rows(&self, o: &StoredObject) -> std::result::Result<Converted, Hold> {
        let Json::Object(m) = &o.content else {
            return Err(Hold("content is not an object".into()));
        };
        if let Some(k) = m
            .keys()
            .find(|k| !["title", "body", "pinned", "tags", "due", "day"].contains(&k.as_str()))
        {
            return Err(Hold(format!(
                "unknown field {k}: no typed column, no original"
            )));
        }
        let s = |k: &str| m.get(k).and_then(Json::as_str).map(str::to_owned);
        let (title, body) = (
            s("title").ok_or_else(|| Hold("title missing".into()))?,
            s("body").unwrap_or_default(),
        );
        let pinned = m.get("pinned").and_then(Json::as_bool).unwrap_or(false);
        let due = s("due").map(|d| due_ms(&d)).transpose()?;
        let day = s("day")
            .map(|d| {
                Date::parse(&d)
                    .map_err(|_| Hold(format!("day is not a plain date YYYY-MM-DD: {d}")))
            })
            .transpose()?;
        let id = id_value(from_v4(o.envelope.id));
        let mut rows = vec![TableRow {
            table: &NOTE,
            values: vec![
                id.clone(),
                text(&title),
                text(&body),
                int(i64::from(pinned)),
                opt_int(due),
                opt_int(day.as_ref().map(date_to_int)),
            ],
        }];
        let mut changes = vec![
            change("title", LogValue::Text(title)),
            change("body", LogValue::Text(body)),
            change("pinned", LogValue::Bool(pinned)),
        ];
        if let Some(ms) = due {
            changes.push(change("due", LogValue::Utc(FsUtc::from_millis(ms))));
        }
        if let Some(d) = day {
            changes.push(change("day", LogValue::Date(d)));
        }
        for (i, t) in m
            .get("tags")
            .and_then(Json::as_array)
            .into_iter()
            .flatten()
            .enumerate()
        {
            let t = t.as_str().ok_or_else(|| Hold("a tag is not text".into()))?;
            rows.push(TableRow {
                table: &TAG,
                values: vec![id.clone(), int(i64::try_from(i).unwrap_or(0)), text(t)],
            });
        }
        Ok(Converted { rows, changes })
    }

    async fn export(&self, c: &Conn<'_>, id: ObjectId) -> Result<Json> {
        let key = vec![id_value(id)];
        let note = c
            .query(
                "SELECT title, body, pinned, due, day FROM notes_note WHERE object = ?",
                key.clone(),
            )
            .await?;
        let Some(r) = note.first() else {
            return Err(crate::v5::codec::invalid(
                "notes_note",
                "no row for the object",
            ));
        };
        let tags = c
            .query(
                "SELECT tag FROM notes_tag WHERE object = ? ORDER BY seq",
                key,
            )
            .await?;
        let tags = tags
            .iter()
            .map(|t| get_text(t, 0).map(Json::String))
            .collect::<Result<Vec<_>>>()?;
        let mut out = json!({"title": get_text(r, 0)?, "body": get_text(r, 1)?, "pinned": get_int(r, 2)? != 0, "tags": tags});
        if let Some(due) = get_opt_int(r, 3)?.and_then(due_text) {
            out["due"] = Json::String(due);
        }
        if let Some(day) = get_opt_int(r, 4)? {
            out["day"] = Json::String(int_to_date(day)?.canonical());
        }
        Ok(out)
    }

    fn normalize(&self, content: &Json) -> Json {
        let mut out = content.clone();
        if let Some(Json::String(d)) = out.get("due").cloned()
            && let Some(n) = due_ms(&d).ok().and_then(due_text)
        {
            out["due"] = Json::String(n);
        }
        if let Json::Object(m) = &mut out {
            m.entry("pinned").or_insert(Json::Bool(false));
            m.entry("body").or_insert(Json::String(String::new()));
            m.entry("tags").or_insert(Json::Array(Vec::new()));
        }
        out
    }
}

fn change(field: &str, v: LogValue) -> FieldChange {
    FieldChange {
        field: field.to_owned(),
        row_key: None,
        old: None,
        new: Some(v),
    }
}
