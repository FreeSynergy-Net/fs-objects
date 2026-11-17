//! The source file is only READ and only a v4 file is migrated (review objects-v007, conditions 1 and 5): the check
//! looks at the schema version before it writes anything, refuses a file that is not exactly v4, and refuses while
//! another handle still holds the file. The file does not change by a single byte, whatever the outcome.
#![cfg(feature = "v5")]

mod common_migrate;

use common_migrate::{Dir, NOW, good, opts, snapshot, v4_file};
use fs_objects::v5::migrate_v4::MigrationPlan;
use fs_objects::v5::migrate_v4::notes::NotesConverter;

async fn sql(path: &std::path::Path, text: &str) {
    let db = turso::Builder::new_local(path.to_str().unwrap())
        .build()
        .await
        .unwrap();
    let c = db.connect().unwrap();
    c.execute_batch(text).await.unwrap();
}

/// Run the check on `path`: it must fail with `expect` in the message, and the folder must be as it was.
async fn refused(dir: &Dir, path: &std::path::Path, expect: &str) {
    let before = snapshot(dir);
    let mut plan = MigrationPlan::open(path).unwrap();
    let err = plan
        .check(&NotesConverter, opts(), NOW)
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains(expect), "{expect:?} not in {err:?}");
    assert_eq!(
        snapshot(dir),
        before,
        "the folder changed although the check was refused: {expect}"
    );
}

#[tokio::test]
async fn only_a_v4_file_is_accepted_and_a_refused_file_stays_byte_for_byte() {
    // Version 3: a v4 file whose version row says 3.
    let d3 = Dir::new("fs-objects-src-v3");
    let p3 = v4_file(&d3, good()).await;
    sql(&p3, "UPDATE schema_version SET version = 3").await;
    refused(&d3, &p3, "schema version 3, expected 4").await;
    // Version 5 (newer than this crate).
    let d5 = Dir::new("fs-objects-src-v5");
    let p5 = v4_file(&d5, good()).await;
    sql(&p5, "UPDATE schema_version SET version = 5").await;
    refused(&d5, &p5, "schema version 5, expected 4").await;
    // Version 0: a database without any store table.
    let d0 = Dir::new("fs-objects-src-v0");
    let p0 = d0.file("objects.db");
    sql(&p0, "CREATE TABLE other (x INTEGER)").await;
    refused(&d0, &p0, "schema version 0, expected 4").await;
    // An empty file and a foreign file.
    let de = Dir::new("fs-objects-src-empty");
    let pe = de.file("objects.db");
    std::fs::write(&pe, b"").unwrap();
    refused(&de, &pe, "objects.db").await;
    let df = Dir::new("fs-objects-src-foreign");
    let pf = df.file("objects.db");
    std::fs::write(
        &pf,
        b"this is a letter to my aunt, not a database\n".repeat(200),
    )
    .unwrap();
    refused(&df, &pf, "objects.db").await;
    // A real v5 file (typed tables, no v4 version row).
    let dv = Dir::new("fs-objects-src-real-v5");
    let pv = dv.file("objects.db");
    drop(
        fs_objects::v5::Store::open(pv.to_str().unwrap(), NOW)
            .await
            .unwrap(),
    );
    refused(&dv, &pv, "expected 4").await;
}

#[tokio::test]
async fn a_good_v4_file_is_read_without_changing_a_byte() {
    let dir = Dir::new("fs-objects-src-v4");
    let path = v4_file(&dir, good()).await;
    let before = std::fs::read(&path).unwrap();
    let mut plan = MigrationPlan::open(&path).unwrap();
    plan.check(&NotesConverter, opts(), NOW).await.unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert_eq!(
        std::fs::read(plan.backup().unwrap()).unwrap(),
        before,
        "the backup is the file as it was"
    );
}

#[tokio::test]
async fn the_program_must_be_closed_a_held_lock_stops_the_check() {
    let dir = Dir::new("fs-objects-src-lock");
    let path = v4_file(&dir, good()).await;
    let program = std::fs::File::open(&path).unwrap();
    program.lock().unwrap();
    refused(&dir, &path, "locked by another handle").await;
    drop(program);
    let mut plan = MigrationPlan::open(&path).unwrap();
    plan.check(&NotesConverter, opts(), NOW)
        .await
        .expect("once the program is closed the check runs");
}
