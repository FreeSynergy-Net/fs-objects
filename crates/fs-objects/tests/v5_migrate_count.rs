//! Review umwandler-2, condition 6: the count is explicit (v4 = converted + held back), and objects that the v4 store
//! refuses to return (written unlogged) stop the run WITH ALL of them listed at once; nothing is left behind.
#![cfg(all(feature = "v5", unix))] // the migrator is unix only for now (Windows: a locked file cannot be read through another handle)

mod common_migrate;

use common_migrate::{Dir, NOW, good, note, opts, snapshot, v4_file};
use fs_objects::traits::UnloggedObjects as _;
use fs_objects::v5::migrate_v4::MigrationPlan;
use fs_objects::v5::migrate_v4::notes::NotesConverter;
use fs_objects::{Store as _, TursoStore};
use serde_json::json;

#[tokio::test]
async fn the_report_counts_v4_equal_converted_plus_held() {
    let dir = Dir::new("fs-objects-count-ok");
    let mut objects = good();
    objects.push(note("unknown", json!({"title":"u","color":"red"})));
    let path = v4_file(&dir, objects).await;
    let mut plan = MigrationPlan::open(&path).unwrap();
    let report = plan
        .check(&NotesConverter, opts(), NOW)
        .await
        .unwrap()
        .clone();
    assert_eq!(report.v4_total, 4);
    assert_eq!(
        report.converted_total() + report.held.len(),
        report.v4_total
    );
    let text = std::fs::read_to_string(plan.report_file().unwrap()).unwrap();
    assert!(
        text.contains("v4 objects: 4 = converted 3 + held back 1"),
        "{text}"
    );
}

#[tokio::test]
async fn every_unlogged_object_is_listed_at_once_and_nothing_is_switched() {
    let dir = Dir::new("fs-objects-count-unlogged");
    let path = v4_file(&dir, good()).await;
    let mut ids = Vec::new();
    {
        let s = TursoStore::open_with(&path, opts()).await.unwrap();
        for n in 0..3 {
            let o = note(&format!("ballot {n}"), json!({"title":"secret"}));
            ids.push(o.envelope.id.to_string());
            s.put_unlogged(o).await.unwrap();
        }
        assert_eq!(s.count(&fs_objects::Query::new()).await.unwrap(), 6);
    }
    let before = snapshot(&dir);
    let mut plan = MigrationPlan::open(&path).unwrap();
    let err = plan
        .check(&NotesConverter, opts(), NOW)
        .await
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("3 of 6 v4 objects cannot be migrated"),
        "{err}"
    );
    let converted_ids: Vec<_> = ids
        .iter()
        .map(|i| fs_objects::v5::codec::id_text(fs_objects::v5::id::from_v4(i.parse().unwrap())))
        .collect();
    for id in &converted_ids {
        assert!(
            err.contains(id.as_str()),
            "{id} (all of them in one run) not in {err}"
        );
    }
    assert!(
        err.contains("nothing is switched") && err.contains("(Note)"),
        "{err}"
    );
    assert_eq!(snapshot(&dir), before, "no half-built file, no backup");
}
