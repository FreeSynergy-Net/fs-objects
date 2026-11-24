//! Helpers of the migration tests: a folder under the test target (never `/tmp`, which is RAM), synthetic v4 files,
//! a byte-exact view of a folder, and the standard-SQLite check of the migrated file.
#![allow(dead_code)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use fs_objects::v5::migrate_v4::notes::NOTE_TYPE;
use fs_objects::{Envelope, OpenOptions, Store as _, StoredObject, TursoStore};
use serde_json::json;
use sha2::{Digest, Sha256};

pub const NOW: i64 = 1_794_823_200_000;
pub const ME: &str = "https://example.org/users/me";

/// A folder under `CARGO_TARGET_TMPDIR`, emptied first and removed on drop (also while a panic unwinds).
pub struct Dir(pub PathBuf);

impl Dir {
    pub fn new(name: &str) -> Self {
        let d =
            Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        Self(d)
    }

    pub fn file(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

pub fn opts() -> OpenOptions {
    OpenOptions {
        owner: Some(fs_objects::testing::owner()),
        allow_sensitive_unencrypted: true,
        ..OpenOptions::default()
    }
}

pub fn note(title: &str, content: serde_json::Value) -> StoredObject {
    let mut e = Envelope::new(NOTE_TYPE, title);
    e.attributed_to = Some(ME.into());
    StoredObject::new(e, "application/json", content)
}

pub fn good() -> Vec<StoredObject> {
    vec![
        note(
            "one",
            json!({"title":"one","body":"b","pinned":true,"tags":["x","y"],"due":"2026-03-01T10:00:00+01:00"}),
        ),
        note("two", json!({"title":"two"})),
        note("three", json!({"title":"three","body":"c","tags":["z"]})),
    ]
}

pub async fn v4_file(dir: &Dir, objects: Vec<StoredObject>) -> PathBuf {
    let path = dir.file("objects.db");
    let s = TursoStore::open_with(&path, opts()).await.unwrap();
    for o in objects {
        s.put(o, ME).await.unwrap();
    }
    drop(s);
    path
}

/// Name to SHA-256 of every file in the folder: nothing may appear, vanish or change.
pub fn snapshot(dir: &Dir) -> BTreeMap<String, Vec<u8>> {
    std::fs::read_dir(&dir.0)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.is_file())
        .map(|p| {
            let bytes = std::fs::read(&p).unwrap();
            (
                p.file_name().unwrap().to_string_lossy().into_owned(),
                Sha256::digest(&bytes).to_vec(),
            )
        })
        .collect()
}

/// `tests/sqlite_check.py` on the file in place, with its `-wal` file beside it (never a bare copy).
pub fn check_with_standard_sqlite(file: &Path) {
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
