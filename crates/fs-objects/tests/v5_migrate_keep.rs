//! Review umwandler-3, conditions A1 and A2: reading the store file must not drop a `fcntl` lock. A program that
//! starts while the check runs (A1), and a program that is open in the SAME process while `confirm`, `switch` or
//! `fall_back` read the file (A2), keep the engine's write lock: another process that tries to open the file with
//! a real `TursoStore` is refused. The "other process" is this test binary started again with `FS_TRY_OPEN` set.
//!
//! Pattern: Probe in a child process (the lock belongs to a process, so only another process can see it).
#![cfg(all(feature = "v5", unix))]

mod common_migrate;

use std::path::Path;
use std::process::Command;

use common_migrate::{Dir, NOW, good, opts, v4_file};
use fs_objects::TursoStore;
use fs_objects::v5::migrate_v4::notes::NotesConverter;
use fs_objects::v5::migrate_v4::{MigrationPlan, SourceLock, fingerprint};

/// The "program that starts": only runs when started by `other_process_opens`.
#[tokio::test]
async fn child_tries_to_open() {
    let Ok(path) = std::env::var("FS_TRY_OPEN") else {
        return;
    };
    let opened = if path.ends_with(".v5") {
        fs_objects::v5::Store::open(&path, NOW)
            .await
            .map(|_| ())
            .map_err(|e| e.to_string())
    } else {
        TursoStore::open_with(&path, opts())
            .await
            .map(|_| ())
            .map_err(|e| e.to_string())
    };
    match opened {
        Ok(()) => println!("\nFS-OPENED"),
        Err(e) => println!("\nFS-REFUSED {e}"),
    }
}

/// Whether another process can open the file with the engine (true = it got in: the lock is gone).
fn other_process_opens(path: &Path) -> bool {
    let out = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "child_tries_to_open",
            "--nocapture",
            "--test-threads=1",
        ])
        .env("FS_TRY_OPEN", path)
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(
        text.contains("FS-OPENED") || text.contains("FS-REFUSED"),
        "the child did not report: {text} {}",
        String::from_utf8_lossy(&out.stderr)
    );
    text.contains("FS-OPENED")
}

/// Control: the lock alone keeps a program that starts later out; a released lock does not.
#[tokio::test]
async fn control_the_lock_keeps_a_later_program_out_until_it_is_released() {
    let dir = Dir::new("keep-control");
    let path = v4_file(&dir, good()).await;
    let lock = SourceLock::take(&path).unwrap();
    assert!(
        !other_process_opens(&path),
        "control: the lock does not hold"
    );
    drop(lock);
    assert!(other_process_opens(&path), "the lock was not released");
}

/// A1: the fingerprint (and the backup) read the v4 file while the lock is held: a program that starts DURING the
/// check is still refused.
#[tokio::test]
async fn a1_the_lock_survives_the_fingerprint_read() {
    let dir = Dir::new("keep-a1");
    let path = v4_file(&dir, good()).await;
    let lock = SourceLock::take(&path).unwrap();
    let _ = fingerprint(&path).unwrap();
    let got_in = other_process_opens(&path);
    drop(lock);
    assert!(!got_in, "a program that started during the check got in");
}

/// A1, whole check: with a check running, a program starting in another process is refused until it ends. The
/// converter is slow only by being called; the check is held at a converter call through a channel-free probe:
/// here the probe runs after `check` took the lock and read the file, through a converter that starts the child.
#[tokio::test]
async fn a1_a_program_started_by_the_converter_during_check_is_refused() {
    use async_trait::async_trait;
    use fs_objects::StoredObject;
    use fs_objects::v5::migrate_v4::{Converted, Converter, Hold};
    use fs_objects::v5::{Conn, Domain, Result};
    use std::sync::atomic::{AtomicU8, Ordering};

    struct Probe(std::path::PathBuf, AtomicU8);
    #[async_trait]
    impl Converter for Probe {
        fn domain(&self) -> Domain {
            NotesConverter.domain()
        }
        fn handles(&self, t: &str) -> bool {
            NotesConverter.handles(t)
        }
        fn to_rows(&self, o: &StoredObject) -> std::result::Result<Converted, Hold> {
            if self.1.load(Ordering::SeqCst) == 0 {
                let v = if other_process_opens(&self.0) { 2 } else { 1 };
                self.1.store(v, Ordering::SeqCst);
            }
            NotesConverter.to_rows(o)
        }
        async fn export(&self, c: &Conn<'_>, id: fs_types::ObjectId) -> Result<serde_json::Value> {
            NotesConverter.export(c, id).await
        }
        fn normalize(&self, c: &serde_json::Value) -> serde_json::Value {
            NotesConverter.normalize(c)
        }
    }
    let dir = Dir::new("keep-a1-check");
    let path = v4_file(&dir, good()).await;
    let probe = Probe(path.clone(), AtomicU8::new(0));
    let mut plan = MigrationPlan::open(&path).unwrap();
    let _ = plan.check(&probe, opts(), NOW).await;
    assert_eq!(
        probe.1.load(Ordering::SeqCst),
        1,
        "a program started during the check (0 = never asked, 2 = got in)"
    );
}

/// A2: a program is open in THIS process; the fingerprint read must not take its lock away.
#[tokio::test]
async fn a2_a_fingerprint_read_leaves_the_open_programs_lock() {
    let dir = Dir::new("keep-a2");
    let path = v4_file(&dir, good()).await;
    let program = TursoStore::open_with(&path, opts()).await.unwrap();
    assert!(
        !other_process_opens(&path),
        "control: the open program blocks"
    );
    let _ = fingerprint(&path).unwrap();
    let got_in = other_process_opens(&path);
    drop(program);
    assert!(
        !got_in,
        "after the read another process got the engine's lock"
    );
}

/// A2 through `confirm` and `switch`: they read the v4 file while the program has it open; the lock stays.
#[tokio::test]
async fn a2_confirm_and_switch_leave_the_open_programs_lock() {
    let dir = Dir::new("keep-a2-steps");
    let path = v4_file(&dir, good()).await;
    let mut plan = MigrationPlan::open(&path).unwrap();
    plan.check(&NotesConverter, opts(), NOW).await.unwrap();
    let program = TursoStore::open_with(&path, opts()).await.unwrap();
    assert!(
        !other_process_opens(&path),
        "control: the open program blocks"
    );
    let _ = plan.confirm();
    let after_confirm = other_process_opens(&path);
    let _ = plan.switch();
    let after_switch = other_process_opens(&path);
    drop(program);
    assert!(!after_confirm, "confirm took the program's lock");
    assert!(!after_switch, "switch took the program's lock");
}

/// A2 through `fall_back`: it reads the new file while a program has THAT open (the child opens it with the v5 engine
/// layer, a file that ends in `.v5`).
#[tokio::test]
async fn a2_fall_back_leaves_the_open_programs_lock_on_the_new_file() {
    let dir = Dir::new("keep-a2-fall");
    let path = v4_file(&dir, good()).await;
    let mut plan = MigrationPlan::open(&path).unwrap();
    plan.check(&NotesConverter, opts(), NOW).await.unwrap();
    plan.confirm().unwrap();
    let v5 = plan.switch().unwrap().to_path_buf();
    assert!(
        other_process_opens(&v5),
        "control: nobody has the new file open"
    );
    let program = fs_objects::v5::Store::open(v5.to_str().unwrap(), NOW)
        .await
        .unwrap();
    assert!(
        !other_process_opens(&v5),
        "control: the open program blocks"
    );
    let _ = plan.fall_back(false);
    let got_in = other_process_opens(&v5);
    drop(program);
    assert!(
        !got_in,
        "fall_back took the lock of the program on the new file"
    );
}
