//! Review umwandler-2, conditions 1, 3 and 4: the v4 file is compared again at `confirm` and `switch`; the state file
//! is written atomically and a damaged one is an error; a v4 file that changes during the run, a converter that
//! brings in an original the v4 object did not have, and a damaged backup are found.
#![cfg(feature = "v5")]

mod common_migrate;

use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use common_migrate::{Dir, ME, NOW, good, note, opts, snapshot, v4_file};
use fs_objects::StoredObject;
use fs_objects::v5::codec::{id_value, int, text};
use fs_objects::v5::migrate_v4::notes::NotesConverter;
use fs_objects::v5::migrate_v4::{Converted, Converter, Hold, MigrationPlan, Phase, TableRow};
use fs_objects::v5::tables_more::OBJECT_ORIGINAL;
use fs_objects::v5::{Conn, Domain};
use fs_objects::{Store as _, TursoStore};
use serde_json::json;

/// The program writes into the (still active) v4 file after the check.
async fn program_writes(path: &std::path::Path) {
    let s = TursoStore::open_with(path, opts()).await.unwrap();
    s.put(note("written after the check", json!({"title":"x"})), ME)
        .await
        .unwrap();
}

#[tokio::test]
async fn confirm_refuses_when_v4_changed_after_the_check() {
    let dir = Dir::new("fs-objects-guard-confirm");
    let path = v4_file(&dir, good()).await;
    let mut plan = MigrationPlan::open(&path).unwrap();
    plan.check(&NotesConverter, opts(), NOW).await.unwrap();
    program_writes(&path).await;
    let err = plan.confirm().unwrap_err().to_string();
    assert!(
        err.contains("changed since the check") && err.contains("new check"),
        "{err}"
    );
    assert_eq!(plan.phase(), Phase::Checking, "a new check is needed");
    assert_eq!(plan.active_path(), path);
    // The stale new file is never overwritten; once a person removed it, a new check carries the new object.
    let again = plan
        .check(&NotesConverter, opts(), NOW)
        .await
        .unwrap_err()
        .to_string();
    assert!(again.contains("already exists"), "{again}");
    for side in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}.v5{side}", path.display()));
    }
    let report = plan
        .check(&NotesConverter, opts(), NOW)
        .await
        .unwrap()
        .clone();
    assert_eq!(
        report.converted_total(),
        4,
        "the object written after the first check is in the new file now"
    );
    plan.confirm().unwrap();
    plan.switch().unwrap();
}

#[tokio::test]
async fn switch_refuses_when_v4_changed_between_confirm_and_switch() {
    let dir = Dir::new("fs-objects-guard-switch");
    let path = v4_file(&dir, good()).await;
    let mut plan = MigrationPlan::open(&path).unwrap();
    plan.check(&NotesConverter, opts(), NOW).await.unwrap();
    plan.confirm().unwrap();
    program_writes(&path).await;
    let err = plan.switch().map(|_| ()).unwrap_err().to_string();
    assert!(err.contains("changed since the check"), "{err}");
    assert_eq!(plan.phase(), Phase::Checking);
    assert_eq!(
        plan.active_path(),
        path,
        "v4 stays the active file: nothing was lost"
    );
}

#[tokio::test]
async fn a_state_file_without_the_fingerprint_does_not_confirm() {
    let dir = Dir::new("fs-objects-guard-nofp");
    let path = v4_file(&dir, good()).await;
    let mut plan = MigrationPlan::open(&path).unwrap();
    plan.check(&NotesConverter, opts(), NOW).await.unwrap();
    let state = dir.file("objects.db.migration-state");
    let text = std::fs::read_to_string(&state).unwrap();
    let kept = text
        .lines()
        .filter(|l| !l.starts_with("v4fingerprint="))
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    std::fs::write(&state, kept).unwrap();
    let mut again = MigrationPlan::open(&path).unwrap();
    assert!(
        again.confirm().is_err(),
        "an unknown v4 state counts as changed"
    );
}

#[tokio::test]
async fn an_empty_or_cut_state_file_is_an_error_not_a_phase() {
    let dir = Dir::new("fs-objects-guard-state");
    let path = v4_file(&dir, good()).await;
    let mut plan = MigrationPlan::open(&path).unwrap();
    plan.check(&NotesConverter, opts(), NOW).await.unwrap();
    plan.confirm().unwrap();
    plan.switch().unwrap();
    let state = dir.file("objects.db.migration-state");
    let full = std::fs::read_to_string(&state).unwrap();
    assert!(full.ends_with("end=state\n"), "{full}");
    assert!(
        !dir.file("objects.db.migration-state.tmp").exists(),
        "the temp file was renamed away"
    );
    for damaged in [
        String::new(),
        "phase=switched\n".to_owned(),
        full.replace("end=state\n", ""),
        "\n\n".to_owned(),
    ] {
        std::fs::write(&state, &damaged).unwrap();
        let err = MigrationPlan::open(&path).unwrap_err().to_string();
        assert!(err.contains("damaged"), "{damaged:?} gave {err}");
    }
    std::fs::write(&state, &full).unwrap();
    let ok = MigrationPlan::open(&path).unwrap();
    assert_eq!(ok.phase(), Phase::Switched);
}

#[tokio::test]
async fn the_state_file_is_written_through_a_temp_file_and_a_failed_write_keeps_the_old_one() {
    let dir = Dir::new("fs-objects-guard-atomic");
    let path = v4_file(&dir, good()).await;
    let mut plan = MigrationPlan::open(&path).unwrap();
    plan.check(&NotesConverter, opts(), NOW).await.unwrap();
    let state = dir.file("objects.db.migration-state");
    let old = std::fs::read(&state).unwrap();
    // The temp file's name is taken by a folder: the write must fail and must not touch the state file.
    std::fs::create_dir(dir.file("objects.db.migration-state.tmp")).unwrap();
    assert!(
        plan.confirm().is_err(),
        "a write that cannot use its temp file must fail, not write in place"
    );
    assert_eq!(
        std::fs::read(&state).unwrap(),
        old,
        "the old state file is untouched"
    );
    assert_eq!(plan.phase(), Phase::Ready);
}

/// Appends bytes to the v4 file while the check runs (the second object).
struct Meddles(AtomicUsize, std::path::PathBuf);

#[async_trait]
impl Converter for Meddles {
    fn domain(&self) -> Domain {
        NotesConverter.domain()
    }
    fn handles(&self, t: &str) -> bool {
        NotesConverter.handles(t)
    }
    fn to_rows(&self, o: &StoredObject) -> Result<Converted, Hold> {
        if self.0.fetch_add(1, Ordering::SeqCst) == 1 {
            use std::io::Write;
            let mut f = std::fs::OpenOptions::new()
                .append(true)
                .open(&self.1)
                .unwrap();
            f.write_all(b"tail").unwrap();
        }
        NotesConverter.to_rows(o)
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
async fn a_v4_file_that_changes_during_the_run_aborts_the_check() {
    let dir = Dir::new("fs-objects-guard-during");
    let path = v4_file(&dir, good()).await;
    let before = snapshot(&dir);
    let mut plan = MigrationPlan::open(&path).unwrap();
    let conv = Meddles(AtomicUsize::new(0), path.clone());
    let err = plan
        .check(&conv, opts(), NOW)
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("changed while the check ran"), "{err}");
    assert_eq!(plan.phase(), Phase::Checking);
    let mut after = snapshot(&dir);
    let mut want = before;
    after.remove("objects.db");
    want.remove("objects.db");
    assert_eq!(after, want, "the half-built file and the backup are gone");
}

/// Brings in an original of its own for an object that has none in v4.
struct Invents;

#[async_trait]
impl Converter for Invents {
    fn domain(&self) -> Domain {
        NotesConverter.domain()
    }
    fn handles(&self, t: &str) -> bool {
        NotesConverter.handles(t)
    }
    fn to_rows(&self, o: &StoredObject) -> Result<Converted, Hold> {
        let mut out = NotesConverter.to_rows(o)?;
        out.rows.push(TableRow {
            table: &OBJECT_ORIGINAL,
            values: vec![
                id_value(fs_objects::v5::id::from_v4(o.envelope.id)),
                text("text/plain"),
                turso::Value::Blob(b"not in v4".to_vec()),
                text(&"0".repeat(64)),
                int(NOW),
                turso::Value::Null,
            ],
        });
        Ok(out)
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
async fn an_original_that_v4_does_not_have_is_found_by_the_round_trip() {
    let dir = Dir::new("fs-objects-guard-original");
    let path = v4_file(&dir, vec![note("a", json!({"title":"a"}))]).await;
    let mut plan = MigrationPlan::open(&path).unwrap();
    let report = plan.check(&Invents, opts(), NOW).await.unwrap().clone();
    assert_eq!(report.converted_total(), 0);
    assert_eq!(report.held.len(), 1);
    assert!(
        report.held[0].reason.contains("roundtrip.extras"),
        "{:?}",
        report.held
    );
}
