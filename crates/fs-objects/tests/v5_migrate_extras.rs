//! What the migration carries besides the converter's rows (review objects-v007, conditions 2 and 6): the envelope
//! times and the media type are carried by the converter or the object is HELD BACK with the reason, never dropped;
//! the original is carried and compared; free text that looks like JSON is no finding; the migrated file passes the
//! standard-SQLite check.
#![cfg(all(feature = "v5", unix))] // the migrator is unix only for now (Windows: a locked file cannot be read through another handle)

mod common_migrate;

use async_trait::async_trait;
use chrono::DateTime;
use common_migrate::{Dir, NOW, check_with_standard_sqlite, note, opts, v4_file};
use fs_objects::v5::check::typed;
use fs_objects::v5::codec::{get_int, get_opt_int, id_value, int, opt_int};
use fs_objects::v5::id::from_v4;
use fs_objects::v5::migrate_v4::notes::{NOTE, NotesConverter, TAG};
use fs_objects::v5::migrate_v4::{Converted, Converter, Hold, MigrationPlan, TableRow};
use fs_objects::v5::spec::Logical::{Ref, Utc};
use fs_objects::v5::{Col, Conn, Domain, Result, Step, Store, Table};
use fs_objects::{Blob, StoredObject};
use serde_json::json;

fn at(ms: i64) -> chrono::DateTime<chrono::Utc> {
    DateTime::from_timestamp_millis(ms).unwrap()
}

const TIMES: Table = Table {
    name: "notes_times",
    cols: &[
        Col::req("object", Ref),
        Col::opt("start_ms", Utc),
        Col::opt("end_ms", Utc),
    ],
    constraints: &[
        "PRIMARY KEY (object)",
        "FOREIGN KEY (object) REFERENCES notes_note(object) ON DELETE CASCADE",
    ],
    indexes: &[],
};
const STEPS: [Step; 1] = [Step::Create(&[NOTE, TAG, TIMES])];

/// A converter that carries the envelope times in a table of its own.
struct Timed;

#[async_trait]
impl Converter for Timed {
    fn domain(&self) -> Domain {
        Domain::new("notes", "Notes with times", &[NOTE, TAG, TIMES], &STEPS)
    }
    fn handles(&self, t: &str) -> bool {
        NotesConverter.handles(t)
    }
    fn to_rows(&self, o: &StoredObject) -> std::result::Result<Converted, Hold> {
        let mut c = NotesConverter.to_rows(o)?;
        let e = &o.envelope;
        c.rows.push(TableRow {
            table: &TIMES,
            values: vec![
                id_value(from_v4(e.id)),
                opt_int(e.start_time.map(|t| t.timestamp_millis())),
                opt_int(e.end_time.map(|t| t.timestamp_millis())),
            ],
        });
        Ok(c)
    }
    async fn export(&self, c: &Conn<'_>, id: fs_types::ObjectId) -> Result<serde_json::Value> {
        NotesConverter.export(c, id).await
    }
    fn normalize(&self, c: &serde_json::Value) -> serde_json::Value {
        NotesConverter.normalize(c)
    }
    fn carries_times(&self) -> bool {
        true
    }
    async fn export_times(
        &self,
        c: &Conn<'_>,
        id: fs_types::ObjectId,
    ) -> Result<(Option<i64>, Option<i64>)> {
        let r = c
            .query(
                "SELECT start_ms, end_ms FROM notes_times WHERE object = ?",
                vec![id_value(id)],
            )
            .await?;
        let r = r.first().cloned().unwrap_or_default();
        Ok((get_opt_int(&r, 0)?, get_opt_int(&r, 1)?))
    }
}

/// Says it carries the times but writes none: the round trip must find it.
struct Liar;

#[async_trait]
impl Converter for Liar {
    fn domain(&self) -> Domain {
        NotesConverter.domain()
    }
    fn handles(&self, t: &str) -> bool {
        NotesConverter.handles(t)
    }
    fn to_rows(&self, o: &StoredObject) -> std::result::Result<Converted, Hold> {
        NotesConverter.to_rows(o)
    }
    async fn export(&self, c: &Conn<'_>, id: fs_types::ObjectId) -> Result<serde_json::Value> {
        NotesConverter.export(c, id).await
    }
    fn normalize(&self, c: &serde_json::Value) -> serde_json::Value {
        NotesConverter.normalize(c)
    }
    fn carries_times(&self) -> bool {
        true
    }
}

fn timed(title: &str, start: Option<i64>, end: Option<i64>) -> StoredObject {
    let mut o = note(title, json!({"title": title}));
    o.envelope.start_time = start.map(at);
    o.envelope.end_time = end.map(at);
    o
}

#[tokio::test]
async fn times_and_media_type_are_carried_by_the_converter_or_held_never_dropped() {
    let dir = Dir::new("fs-objects-extras-times");
    let mut vcard = note("card", json!({"title":"card"}));
    vcard.content_type = "text/vcard".into();
    let objects = vec![
        timed("plain", None, None),
        timed("start", Some(1_700_000_000_123), None),
        timed("end", None, Some(1_700_000_999_000)),
        vcard,
    ];
    let path = v4_file(&dir, objects).await;
    let mut plan = MigrationPlan::open(&path).unwrap();
    let report = plan
        .check(&NotesConverter, opts(), NOW)
        .await
        .unwrap()
        .clone();
    assert_eq!(
        report.converted_total(),
        1,
        "only the object without times and with the right type"
    );
    assert_eq!(report.held.len(), 3);
    let why = |name: &str| {
        report
            .held
            .iter()
            .filter(|h| h.reason.contains(name))
            .count()
    };
    assert_eq!(why("start_time/end_time are set"), 2);
    assert_eq!(why("content_type text/vcard"), 1);
}

#[tokio::test]
async fn a_converter_that_carries_the_times_is_compared_by_the_round_trip() {
    let dir = Dir::new("fs-objects-extras-timed");
    let path = v4_file(
        &dir,
        vec![timed("s", Some(1_700_000_000_123), Some(1_700_000_999_000))],
    )
    .await;
    let mut plan = MigrationPlan::open(&path).unwrap();
    let r = plan.check(&Timed, opts(), NOW).await.unwrap().clone();
    assert_eq!((r.converted_total(), r.held.len()), (1, 0));
    let v5 = Store::open(&r.target, NOW).await.unwrap();
    let row = v5
        .read()
        .await
        .query("SELECT start_ms, end_ms FROM notes_times", Vec::new())
        .await
        .unwrap();
    assert_eq!(
        (get_int(&row[0], 0).unwrap(), get_int(&row[0], 1).unwrap()),
        (1_700_000_000_123, 1_700_000_999_000)
    );
    // The liar says it carries them and does not: held with the round-trip reason.
    let d2 = Dir::new("fs-objects-extras-liar");
    let p2 = v4_file(&d2, vec![timed("s", Some(1_700_000_000_123), None)]).await;
    let mut plan = MigrationPlan::open(&p2).unwrap();
    let r = plan.check(&Liar, opts(), NOW).await.unwrap().clone();
    assert_eq!((r.converted_total(), r.held.len()), (0, 1));
    assert!(
        r.held[0].reason.contains("roundtrip.extras"),
        "{:?}",
        r.held
    );
    let _ = int(0);
}

#[tokio::test]
async fn the_original_is_carried_byte_for_byte_and_compared() {
    let dir = Dir::new("fs-objects-extras-original");
    let bytes: Vec<u8> = (0..=255).collect();
    let o = note("with original", json!({"title":"with original"})).with_original(Blob {
        media_type: "text/vcard".into(),
        bytes: bytes.clone(),
    });
    let path = v4_file(&dir, vec![o, note("without", json!({"title":"without"}))]).await;
    let mut plan = MigrationPlan::open(&path).unwrap();
    let r = plan
        .check(&NotesConverter, opts(), NOW)
        .await
        .unwrap()
        .clone();
    assert_eq!((r.converted_total(), r.held.len()), (2, 0), "{:?}", r.held);
    let v5 = Store::open(&r.target, NOW).await.unwrap();
    let rows = v5
        .read()
        .await
        .query(
            "SELECT media_type, bytes, sha256 FROM object_original",
            Vec::new(),
        )
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0][1], turso::Value::Blob(bytes.clone()));
    assert_eq!(
        rows[0][2],
        turso::Value::Text(fs_objects::v5::original::sha256_hex(&bytes))
    );
    assert_eq!(typed(&v5).await.unwrap().len(), 0);
}

#[tokio::test]
async fn free_text_that_looks_like_json_is_no_finding_and_the_file_passes_standard_sqlite() {
    let dir = Dir::new("fs-objects-extras-freetext");
    let objects = vec![
        note(
            "[1,2]",
            json!({"title":"{}","body":"[1,2]","tags":["{\"a\":1}","[3]"]}),
        ),
        note("{}", json!({"title":"[true]","body":"{\"x\": [1]}"})),
    ];
    let path = v4_file(&dir, objects).await;
    let mut plan = MigrationPlan::open(&path).unwrap();
    let r = plan
        .check(&NotesConverter, opts(), NOW)
        .await
        .unwrap()
        .clone();
    assert_eq!((r.converted_total(), r.held.len()), (2, 0), "{:?}", r.held);
    let v5 = Store::open(&r.target, NOW).await.unwrap();
    let found = typed(&v5).await.unwrap();
    assert!(found.is_empty(), "free text is no JSON: {found:?}");
    drop(v5);
    check_with_standard_sqlite(std::path::Path::new(&r.target));
}
