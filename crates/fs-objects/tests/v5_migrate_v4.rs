//! Migration v4 to v5 (scaffold): backup, a new file, the round trip per object, held objects, the report and the
//! switch that waits for a person.
#![cfg(feature = "v5")]

use async_trait::async_trait;
use fs_objects::v5::migrate_v4::notes::{NOTE_TYPE, NotesConverter};
use fs_objects::v5::migrate_v4::{Converted, Converter, Hold, MigrationPlan, Phase};
use fs_objects::v5::{Conn, Domain, Store, check};
use fs_objects::{Envelope, OpenOptions, Store as _, StoredObject, TursoStore};
use serde_json::json;
use std::path::{Path, PathBuf};

const NOW: i64 = 1_794_823_200_000;
const ME: &str = "https://example.org/users/me";

fn note(title: &str, content: serde_json::Value) -> StoredObject {
    let mut e = Envelope::new(NOTE_TYPE, title);
    e.attributed_to = Some(ME.into());
    StoredObject::new(e, "application/json", content)
}

async fn v4_file(dir: &Path, objects: Vec<StoredObject>) -> PathBuf {
    let path = dir.join("objects.db");
    let s = TursoStore::open_with(&path, opts()).await.unwrap();
    for o in objects {
        s.put(o, ME).await.unwrap();
    }
    drop(s);
    path
}

fn opts() -> OpenOptions {
    OpenOptions {
        allow_sensitive_unencrypted: true,
        ..OpenOptions::default()
    }
}

fn good() -> Vec<StoredObject> {
    vec![
        note(
            "one",
            json!({"title":"one","body":"b","pinned":true,"tags":["x","y"],"due":"2026-03-01T10:00:00+01:00"}),
        ),
        note("two", json!({"title":"two"})),
        note("three", json!({"title":"three","body":"c","tags":["z"]})),
    ]
}

async fn rows(s: &Store, sql: &str) -> usize {
    s.read().await.query(sql, Vec::new()).await.unwrap().len()
}

#[tokio::test]
async fn every_object_round_trips_and_the_report_says_so() {
    let dir = tempfile::tempdir().unwrap();
    let path = v4_file(dir.path(), good()).await;
    let before = std::fs::read(&path).unwrap();
    let mut plan = MigrationPlan::open(&path).unwrap();
    let report = plan
        .check(&NotesConverter, opts(), NOW)
        .await
        .unwrap()
        .clone();
    assert_eq!(report.converted_total(), 3);
    assert_eq!(report.held.len(), 0);
    assert!(
        report.rounded > 0,
        "the v4 instants have microseconds: the rounding is counted"
    );
    // The backup is the v4 file as it was before the run.
    assert_eq!(std::fs::read(plan.backup().unwrap()).unwrap(), before);
    assert!(
        plan.backup()
            .unwrap()
            .to_string_lossy()
            .ends_with(".v4.bak")
    );
    // The report file is a document with the numbers.
    let text = std::fs::read_to_string(plan.report_file().unwrap()).unwrap();
    assert!(text.contains("converted: 3") && text.contains("held back: 0"));
    // The new file holds the typed rows and keeps every rule of ADR 0086.
    let v5 = Store::open(&report.target, NOW).await.unwrap();
    assert_eq!(rows(&v5, "SELECT 1 FROM objects").await, 3);
    assert_eq!(rows(&v5, "SELECT 1 FROM notes_note").await, 3);
    assert_eq!(rows(&v5, "SELECT 1 FROM notes_tag").await, 3);
    assert_eq!(rows(&v5, "SELECT 1 FROM activity").await, 3);
    assert_eq!(check::typed(&v5).await.unwrap().len(), 0);
}

#[tokio::test]
async fn what_does_not_fit_is_held_back_never_lost() {
    let dir = tempfile::tempdir().unwrap();
    let mut addressed = note("addressed", json!({"title":"a"}));
    addressed.envelope.to = vec![ME.into()];
    let mut ctx = note("ctx", json!({"title":"c"}));
    ctx.envelope.context = Some("contacts".into());
    let mut other = note("other", json!({"title":"o"}));
    other.envelope.object_type = "Task".into();
    let objects = vec![
        note("ok", json!({"title":"ok"})),
        note("unknown", json!({"title":"u","color":"red"})),
        addressed,
        ctx,
        other,
    ];
    let path = v4_file(dir.path(), objects).await;
    let mut plan = MigrationPlan::open(&path).unwrap();
    let report = plan
        .check(&NotesConverter, opts(), NOW)
        .await
        .unwrap()
        .clone();
    assert_eq!(report.converted_total(), 1);
    assert_eq!(report.held.len(), 4);
    let why: String = report.held.iter().map(|h| h.reason.clone()).collect();
    for w in [
        "unknown field color",
        "addressing",
        "not an object id",
        "no converter for the type Task",
    ] {
        assert!(why.contains(w), "{w} not in {why}");
    }
    // They are still in the v4 file.
    let v4 = TursoStore::open_with(&path, opts()).await.unwrap();
    assert_eq!(v4.count(&fs_objects::Query::new()).await.unwrap(), 5);
    let v5 = Store::open(&report.target, NOW).await.unwrap();
    assert_eq!(
        rows(&v5, "SELECT 1 FROM objects").await,
        1,
        "a held object leaves no row behind"
    );
}

/// A converter that loses the tags on the way in: the round trip must find it.
struct Lossy;

#[async_trait]
impl Converter for Lossy {
    fn domain(&self) -> Domain {
        NotesConverter.domain()
    }
    fn handles(&self, t: &str) -> bool {
        NotesConverter.handles(t)
    }
    fn to_rows(&self, o: &StoredObject) -> Result<Converted, Hold> {
        let mut c = NotesConverter.to_rows(o)?;
        c.rows.truncate(1);
        Ok(c)
    }
    async fn export(
        &self,
        c: &Conn<'_>,
        id: fs_types::ObjectId,
    ) -> fs_objects::v5::Result<serde_json::Value> {
        NotesConverter.export(c, id).await
    }
    fn normalize(&self, c: &serde_json::Value) -> serde_json::Value {
        NotesConverter.normalize(c)
    }
}

#[tokio::test]
async fn a_lossy_converter_is_caught_by_the_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let objects = vec![
        note("t", json!({"title":"t","tags":["a"]})),
        note("n", json!({"title":"n"})),
    ];
    let path = v4_file(dir.path(), objects).await;
    let mut plan = MigrationPlan::open(&path).unwrap();
    let report = plan.check(&Lossy, opts(), NOW).await.unwrap().clone();
    assert_eq!(
        report.held.len(),
        1,
        "the note with tags lost them and must be held"
    );
    assert!(report.held[0].reason.contains("roundtrip.content"));
    assert_eq!(report.converted_total(), 1);
}

#[tokio::test]
async fn a_taken_backup_name_is_never_overwritten() {
    let dir = tempfile::tempdir().unwrap();
    let path = v4_file(dir.path(), good()).await;
    let taken = dir.path().join("objects.db.v4.bak");
    std::fs::write(&taken, b"an older backup").unwrap();
    let mut plan = MigrationPlan::open(&path).unwrap();
    plan.check(&NotesConverter, opts(), NOW).await.unwrap();
    assert_eq!(std::fs::read(&taken).unwrap(), b"an older backup");
    assert!(
        plan.backup()
            .unwrap()
            .to_string_lossy()
            .ends_with(".v4.bak.1")
    );
}

#[tokio::test]
async fn the_switch_waits_for_the_human() {
    let dir = tempfile::tempdir().unwrap();
    let path = v4_file(dir.path(), good()).await;
    let mut plan = MigrationPlan::open(&path).unwrap();
    assert_eq!(plan.phase(), Phase::Checking);
    assert!(plan.confirm().is_err() && plan.switch().is_err());
    plan.check(&NotesConverter, opts(), NOW).await.unwrap();
    assert_eq!(plan.phase(), Phase::Ready);
    assert!(plan.switch().is_err(), "no switch without a confirmation");
    assert_eq!(plan.active_path(), path);
    plan.confirm().unwrap();
    assert_eq!(plan.active_path(), path, "confirmed is not switched");
    let new = plan.switch().unwrap().to_path_buf();
    assert_ne!(new, path);
    // A restart keeps the state.
    let again = MigrationPlan::open(&path).unwrap();
    assert_eq!(again.phase(), Phase::Switched);
    assert_eq!(again.active_path(), new);
    let mut again = again;
    again.fall_back().unwrap();
    assert_eq!(
        again.active_path(),
        path,
        "the fall-back returns to the v4 file"
    );
    assert!(path.is_file() && again.backup().unwrap().is_file());
}

#[tokio::test]
async fn an_existing_v5_file_is_never_overwritten() {
    let dir = tempfile::tempdir().unwrap();
    let path = v4_file(dir.path(), good()).await;
    let v5 = dir.path().join("objects.db.v5");
    std::fs::write(&v5, b"mine").unwrap();
    let mut plan = MigrationPlan::open(&path).unwrap();
    assert!(plan.check(&NotesConverter, opts(), NOW).await.is_err());
    assert_eq!(std::fs::read(&v5).unwrap(), b"mine");
    assert_eq!(plan.phase(), Phase::Checking);
}
