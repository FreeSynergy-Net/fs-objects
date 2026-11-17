//! The flow around the conversion (review objects-v007, conditions 3 and 4): the fall-back from `Switched` does not
//! give up changes of the new file silently, and an aborted check leaves no half-built file behind.
#![cfg(feature = "v5")]

mod common_migrate;

use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use common_migrate::{Dir, NOW, good, opts, snapshot, v4_file};
use fs_objects::v5::migrate_v4::notes::NotesConverter;
use fs_objects::v5::migrate_v4::{Converted, Converter, Hold, MigrationPlan, Phase};
use fs_objects::v5::{Conn, Domain, Error, Store};
use fs_objects::StoredObject;

async fn switched(dir: &Dir) -> (std::path::PathBuf, MigrationPlan) {
    let path = v4_file(dir, good()).await;
    let mut plan = MigrationPlan::open(&path).unwrap();
    plan.check(&NotesConverter, opts(), NOW).await.unwrap();
    plan.confirm().unwrap();
    plan.switch().unwrap();
    (path, plan)
}

#[tokio::test]
async fn the_fall_back_from_switched_is_refused_when_the_new_file_changed() {
    let dir = Dir::new("fs-objects-flow-loss");
    let (path, mut plan) = switched(&dir).await;
    let v5 = plan.active_path().to_path_buf();
    // The program works in the new file after the switch.
    {
        let s = Store::open(v5.to_str().unwrap(), NOW).await.unwrap();
        s.read()
            .await
            .run("UPDATE objects SET name = 'changed after the switch'")
            .await
            .unwrap();
    }
    let err = plan.fall_back(false).unwrap_err().to_string();
    assert!(err.contains("would be lost") && err.contains("confirm_loss"), "{err}");
    assert_eq!(plan.phase(), Phase::Switched, "refused: nothing changed");
    assert_eq!(plan.active_path(), v5);
    // Only an explicit confirmation gives the changes up, and the state says so after a restart.
    plan.fall_back(true).unwrap();
    assert_eq!(plan.active_path(), path);
    let state = std::fs::read_to_string(dir.file("objects.db.migration-state")).unwrap();
    assert!(state.contains("loss=confirmed") && state.contains("phase=fellback"));
    assert!(v5.is_file(), "the new file is left for a person to delete");
}

#[tokio::test]
async fn the_fall_back_from_switched_is_free_when_nothing_changed() {
    let dir = Dir::new("fs-objects-flow-nochange");
    let (path, _plan) = switched(&dir).await;
    // A restart keeps the fingerprint of the moment of the switch.
    let mut again = MigrationPlan::open(&path).unwrap();
    again.fall_back(false).unwrap();
    assert_eq!(again.phase(), Phase::FellBack);
    // Before the switch there is nothing to lose either.
    let d2 = Dir::new("fs-objects-flow-early");
    let p2 = v4_file(&d2, good()).await;
    let mut early = MigrationPlan::open(&p2).unwrap();
    early.check(&NotesConverter, opts(), NOW).await.unwrap();
    early.fall_back(false).unwrap();
}

/// A converter that breaks in the middle of the run (the second object): fault injection.
struct Breaks(AtomicUsize);

#[async_trait]
impl Converter for Breaks {
    fn domain(&self) -> Domain {
        NotesConverter.domain()
    }
    fn handles(&self, t: &str) -> bool {
        NotesConverter.handles(t)
    }
    fn to_rows(&self, o: &StoredObject) -> Result<Converted, Hold> {
        NotesConverter.to_rows(o)
    }
    async fn export(
        &self,
        c: &Conn<'_>,
        id: fs_types::ObjectId,
    ) -> fs_objects::v5::Result<serde_json::Value> {
        if self.0.fetch_add(1, Ordering::SeqCst) == 1 {
            return Err(Error::Unavailable("the disk went away (injected)".into()));
        }
        NotesConverter.export(c, id).await
    }
    fn normalize(&self, c: &serde_json::Value) -> serde_json::Value {
        NotesConverter.normalize(c)
    }
}

#[tokio::test]
async fn an_aborted_check_removes_what_it_made_and_only_that() {
    let dir = Dir::new("fs-objects-flow-abort");
    let path = v4_file(&dir, good()).await;
    // A foreign backup that was there before must survive.
    let older = dir.file("objects.db.v4.bak");
    std::fs::write(&older, b"an older backup of a person").unwrap();
    let before = snapshot(&dir);
    let mut plan = MigrationPlan::open(&path).unwrap();
    let err = plan
        .check(&Breaks(AtomicUsize::new(0)), opts(), NOW)
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("injected") && err.contains("removed what this run had made"), "{err}");
    assert!(plan.abort_note().unwrap().contains("objects.db.v5"));
    assert_eq!(plan.phase(), Phase::Checking, "nothing was switched");
    assert_eq!(
        snapshot(&dir),
        before,
        "no half-built .v5, no stray backup, nothing of the person's touched"
    );
    // The run can be repeated from the start.
    let mut again = MigrationPlan::open(&path).unwrap();
    again.check(&NotesConverter, opts(), NOW).await.unwrap();
}
