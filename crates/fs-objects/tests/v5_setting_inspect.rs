//! Teil A, `Store::inspect`: K-case KA13 (looking at a file changes nothing: bytes, length, mtime, folder).
#![cfg(feature = "v5")]
#![allow(clippy::many_single_char_names)]

mod common_setting;

use std::fmt::Write as _;
use std::path::Path;

use common_setting::{DOMAIN6, at, dir, file};
use fs_objects::v5::Inspect::{Absent, Broken, Fits, Newer};
use fs_objects::v5::setting::SettingRows;
use fs_objects::v5::setting::tables::{ALL, DOMAIN, STEPS};
use fs_objects::v5::{Domain, Inspect, Store};
use sha2::{Digest, Sha256};

/// What a file is, byte for byte, and what its folder holds.
fn print(p: &Path) -> (String, u64, Option<std::time::SystemTime>, Vec<String>) {
    let (hash, len, mtime) = match std::fs::metadata(p) {
        Ok(m) if m.is_file() => (
            Sha256::digest(std::fs::read(p).unwrap())
                .iter()
                .fold(String::new(), |mut a, b| {
                    let _ = write!(a, "{b:02x}");
                    a
                }),
            m.len(),
            m.modified().ok(),
        ),
        Ok(m) => ("dir-or-special".into(), m.len(), None),
        Err(_) => ("absent".into(), 0, None),
    };
    let mut names: Vec<String> = std::fs::read_dir(p.parent().unwrap())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    (hash, len, mtime, names)
}

async fn looks(p: &Path, d: &Domain) -> Inspect {
    let before = print(p);
    let got = Store::inspect(p.to_str().unwrap(), d).await.unwrap();
    assert_eq!(
        print(p),
        before,
        "inspect changed the file or its folder: {got:?}"
    );
    got
}

async fn valid(p: &Path, d: &Domain) {
    let s = Store::open(p.to_str().unwrap(), 1).await.unwrap();
    s.register_domain(d, 1).await.unwrap();
    let _ = at(1);
}

/// KA13: absent, fits, garbage, half a file, a text file, a folder, a FIFO, newer, a foreign domain.
#[tokio::test]
async fn ka13_inspect_looks_without_touching() {
    let d = dir();
    let p = file(&d);
    assert_eq!(looks(&p, &DOMAIN).await, Absent);
    assert!(!p.exists(), "nothing was created");
    {
        let s = SettingRows::open(p.to_str().unwrap(), at(1)).await.unwrap();
        SettingRows::new(&s).await.unwrap().rev().await.unwrap();
    }
    assert_eq!(looks(&p, &DOMAIN).await, Fits { found: 5 });
    // Half of a valid file.
    let full = std::fs::read(&p).unwrap();
    let half = d.path().join("half.db");
    std::fs::write(&half, &full[..full.len() / 2]).unwrap();
    assert!(matches!(
        looks(&half, &DOMAIN).await,
        Broken {
            cause: "open" | "integrity" | "registry"
        }
    ));
    // 1 KiB of garbage, a text file, an empty file, a header only.
    let junk = d.path().join("junk.db");
    std::fs::write(&junk, vec![0xA5_u8; 1024]).unwrap();
    assert_eq!(looks(&junk, &DOMAIN).await, Broken { cause: "header" });
    let txt = d.path().join("text.db");
    std::fs::write(&txt, "this is not a database\n").unwrap();
    assert_eq!(looks(&txt, &DOMAIN).await, Broken { cause: "header" });
    let empty = d.path().join("empty.db");
    std::fs::write(&empty, "").unwrap();
    assert_eq!(looks(&empty, &DOMAIN).await, Broken { cause: "header" });
    let head = d.path().join("head.db");
    std::fs::write(&head, b"SQLite format 3\0").unwrap();
    assert!(matches!(looks(&head, &DOMAIN).await, Broken { .. }));
    // A folder, a FIFO.
    let sub = d.path().join("folder.db");
    std::fs::create_dir(&sub).unwrap();
    assert_eq!(
        looks(&sub, &DOMAIN).await,
        Broken {
            cause: "not-regular"
        }
    );
    #[cfg(unix)]
    {
        let fifo = d.path().join("fifo.db");
        assert!(
            std::process::Command::new("mkfifo")
                .arg(&fifo)
                .status()
                .unwrap()
                .success()
        );
        assert_eq!(
            looks(&fifo, &DOMAIN).await,
            Broken {
                cause: "not-regular"
            }
        );
    }
    // Newer: six steps against five known. Fits: the same file for a program that knows six.
    let newer = d.path().join("newer.db");
    valid(&newer, &DOMAIN6).await;
    assert_eq!(looks(&newer, &DOMAIN).await, Newer { found: 6, known: 5 });
    assert_eq!(looks(&newer, &DOMAIN6).await, Fits { found: 6 });
    // An older file fits a program that knows more; a changed step is `steps`.
    assert_eq!(looks(&p, &DOMAIN6).await, Fits { found: 5 });
    let other = Domain::new(
        "settings",
        "x",
        &ALL,
        &[STEPS[0], STEPS[2], STEPS[1], STEPS[3], STEPS[4]],
    );
    assert_eq!(looks(&p, &other).await, Broken { cause: "steps" });
    // A foreign domain in the file is not ours.
    let tasks = d.path().join("tasks.db");
    let s = Store::open(tasks.to_str().unwrap(), 1).await.unwrap();
    drop(s);
    assert_eq!(
        looks(&tasks, &DOMAIN).await,
        Fits { found: 0 },
        "a bare store: our domain is not registered yet"
    );
}

/// KA13 (read-only): a store file with no write permission, in a folder without write permission, is still looked at: the
/// engine is opened read-only (an open that wants to write would fail on it), and nothing changes.
#[cfg(unix)]
#[tokio::test]
async fn ka13_inspect_works_on_a_read_only_file_in_a_read_only_folder() {
    use std::os::unix::fs::PermissionsExt;
    let d = dir();
    let p = file(&d);
    {
        let s = SettingRows::open(p.to_str().unwrap(), at(1)).await.unwrap();
        SettingRows::new(&s).await.unwrap().rev().await.unwrap();
    }
    let set = |m: u32| {
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(m)).unwrap();
        std::fs::set_permissions(d.path(), std::fs::Permissions::from_mode(m | 0o100)).unwrap();
    };
    set(0o444);
    let got = looks(&p, &DOMAIN).await;
    let newer = looks(&p, &Domain::new("settings", "x", &ALL, &STEPS[..3])).await;
    set(0o644);
    std::fs::set_permissions(d.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(got, Fits { found: 5 });
    assert_eq!(newer, Newer { found: 5, known: 3 });
}

/// KA13 (foreign file): a valid SQLite file that is not a store (no registry) is `Broken` and stays byte-identical: looking at
/// it must not run the store bootstrap (`Store::open` would add the `fs_*` tables to it).
#[tokio::test]
async fn ka13_a_foreign_sqlite_file_is_not_bootstrapped() {
    let d = dir();
    let p = d.path().join("foreign.db");
    {
        let db = turso::Builder::new_local(p.to_str().unwrap())
            .build()
            .await
            .unwrap();
        let c = db.connect().unwrap();
        c.execute("CREATE TABLE notes (id INTEGER PRIMARY KEY, body TEXT)", ())
            .await
            .unwrap();
        c.execute("INSERT INTO notes (body) VALUES ('mine')", ())
            .await
            .unwrap();
    }
    assert_eq!(looks(&p, &DOMAIN).await, Broken { cause: "registry" });
}
