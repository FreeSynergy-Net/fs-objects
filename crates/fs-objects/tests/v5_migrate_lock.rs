//! Review umwandler-2, condition 2: a REAL program (an open `TursoStore`, which holds the engine's own POSIX lock)
//! stops the check. The program runs in a child process (the POSIX lock belongs to a process): this test binary
//! starts itself with `FS_LOCK_CHILD` set, the child opens the store and waits on stdin.
#![cfg(feature = "v5")]

mod common_migrate;

use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};

use common_migrate::{Dir, NOW, good, opts, v4_file};
use fs_objects::TursoStore;
use fs_objects::v5::migrate_v4::MigrationPlan;
use fs_objects::v5::migrate_v4::notes::NotesConverter;

/// The "program": only runs when started by the test below.
#[tokio::test]
async fn child_program_holds_the_store() {
    let Ok(path) = std::env::var("FS_LOCK_CHILD") else {
        return;
    };
    let _store = TursoStore::open_with(&path, opts()).await.unwrap();
    println!("\nFS-CHILD-READY");
    let mut line = String::new();
    let _ = std::io::stdin().read_line(&mut line);
}

#[tokio::test]
async fn a_running_program_stops_the_check_and_a_closed_one_does_not() {
    let dir = Dir::new("fs-objects-lock-real");
    let path = v4_file(&dir, good()).await;
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "child_program_holds_the_store",
            "--nocapture",
            "--test-threads=1",
        ])
        .env("FS_LOCK_CHILD", &path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut out = BufReader::new(child.stdout.take().unwrap());
    let mut line = String::new();
    while !line.contains("FS-CHILD-READY") {
        line.clear();
        assert!(
            out.read_line(&mut line).unwrap() > 0,
            "the child program did not start"
        );
    }
    let mut plan = MigrationPlan::open(&path).unwrap();
    let r = plan.check(&NotesConverter, opts(), NOW).await;
    let err = r.map(|_| ()).unwrap_err().to_string();
    assert!(err.contains("locked by another handle"), "{err}");
    assert!(!dir.file("objects.db.v5").exists() && !dir.file("objects.db.v4.bak").exists());
    // The program is closed (stdin ends, the child ends): now the check runs.
    drop(child.stdin.take());
    assert!(child.wait().unwrap().success());
    let mut plan = MigrationPlan::open(&path).unwrap();
    plan.check(&NotesConverter, opts(), NOW)
        .await
        .expect("the program is closed");
}
