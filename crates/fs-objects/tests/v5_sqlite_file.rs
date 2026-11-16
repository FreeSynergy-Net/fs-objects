//! General rule (DESIGN.md): after all steps a store file is readable by standard SQLite (`python3 -I`), with no
//! orphan autoindex: on a fresh file and on a file migrated from the v0.0.4 shape. The files live under the test target
//! (never `/tmp`, which is RAM) and are removed when the test ends, also when it fails.
#![cfg(feature = "v5")]

mod common_v5;

use std::path::{Path, PathBuf};

use common_v5::{actor, id};
use fs_objects::v5::Store;
use fs_objects::v5::envelope::insert;
use fs_objects::v5::log::{Kind, log};
use fs_objects::v5::tables::{ACTIVITY, ACTIVITY_CHANGE};

const T0: i64 = 1_794_823_200_000;

/// A folder under `CARGO_TARGET_TMPDIR`, emptied first and removed on drop (also while a panic unwinds).
struct Dir(PathBuf);

impl Dir {
    fn new(name: &str) -> Self {
        let d =
            Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        Self(d)
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Run `tests/sqlite_check.py` on the file in place. The finding that matters shows only with the `-wal` file beside
/// the database (turso writes there without a checkpoint): never a bare copy of the `.db`.
fn check_with_standard_sqlite(file: &Path) {
    let wal = PathBuf::from(format!("{}-wal", file.display()));
    assert!(
        wal.exists(),
        "no -wal file beside the database: the check would prove less than it says"
    );
    let script = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/sqlite_check.py");
    match std::process::Command::new("python3")
        .args(["-I", script])
        .arg(file)
        .output()
    {
        Ok(o) if o.status.success() => {}
        Ok(o) if String::from_utf8_lossy(&o.stderr).contains("No module named") => {
            eprintln!("SKIPPED: python3 has no sqlite3 module");
        }
        Ok(o) => panic!(
            "standard SQLite refuses the file: {}",
            String::from_utf8_lossy(&o.stderr)
        ),
        Err(e) => eprintln!("SKIPPED: python3 not available ({e})"),
    }
}

async fn write_a_row(s: &Store) {
    let c = s.begin().await.unwrap();
    let o = id(T0);
    insert(&c, o, "Note", "n", &actor(), T0).await.unwrap();
    log(&c, o, Kind::Join, &actor(), T0, &[]).await.unwrap();
    c.finish(Ok(())).await.unwrap();
}

#[tokio::test]
async fn standard_sqlite_reads_the_store_file() {
    let dir = Dir::new("fs-objects-kinds");
    let file = dir.0.join("store.db");
    {
        let s = Store::open(file.to_str().unwrap(), 0).await.unwrap();
        write_a_row(&s).await;
    }
    check_with_standard_sqlite(&file);
}

/// The file as v0.0.4 left it (steps 1 to 3 only: no `fs_word`, the `CHECK` list in `activity`), opened by the new code.
#[tokio::test]
async fn standard_sqlite_reads_a_file_migrated_from_the_v0_0_4_shape() {
    let dir = Dir::new("fs-objects-migrated");
    let file = dir.0.join("store.db");
    let path = file.to_str().unwrap();
    {
        let s = Store::open(path, 0).await.unwrap();
        let c = s.begin().await.unwrap();
        c.run("DROP TABLE activity_change").await.unwrap();
        c.run("DROP TABLE activity").await.unwrap();
        c.run(&ACTIVITY.ddl()).await.unwrap();
        c.run(&ACTIVITY_CHANGE.ddl()).await.unwrap();
        c.run("DROP TABLE fs_word").await.unwrap();
        c.run("DELETE FROM fs_step WHERE domain = 'fs_' AND seq > 3")
            .await
            .unwrap();
        c.run("UPDATE fs_domain SET version = 3 WHERE prefix = 'fs_'")
            .await
            .unwrap();
        c.finish(Ok(())).await.unwrap();
    }
    {
        let s = Store::open(path, 1).await.unwrap();
        write_a_row(&s).await;
        let word = s
            .read()
            .await
            .query("SELECT 1 FROM fs_word WHERE code = 'Move'", Vec::new())
            .await
            .unwrap();
        assert_eq!(word.len(), 1, "steps 4 to 7 ran on the old file");
    }
    check_with_standard_sqlite(&file);
}
