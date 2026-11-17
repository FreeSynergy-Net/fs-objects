//! `Store::inspect`: what a store file is, found WITHOUT changing it (PLAN section 5.4 (3): a broken or newer file stays
//! byte-identical, nothing is created, nothing is deleted).
//!
//! Pattern: Facade over a probe on a throw-away copy. How: `turso` 0.8.3 has no read-only open and `Store::open` writes
//! (`bootstrap`), so the file (and its `-wal`) is copied into a fresh folder beside it, the copy is looked at, and the folder is
//! removed again. The original is only read. Written by Claude (Anthropic) as a probe; see CHANGELOG.

use std::path::{Path, PathBuf};

use turso::{Builder, Connection, Value};

use super::codec::{Row, get_int, get_text, id_text, text};
use super::error::{Error, Result};
use super::id::new_id;
use super::registry::{Domain, OWN};
use super::store::Store;

/// What a file is, found WITHOUT changing it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Inspect {
    /// No file there: a first start may create it.
    Absent,
    /// Opens, registry readable, `domain` at a version this program knows (`found <= known`; 0 = not yet registered),
    /// fingerprints equal.
    Fits {
        /// The version the store has.
        found: usize,
    },
    /// More steps than this program knows.
    Newer {
        /// The version the store has.
        found: usize,
        /// The version this program knows.
        known: usize,
    },
    /// Not usable: `cause` = `not-regular` | `header` | `open` | `registry` | `steps` (fingerprint differs) | `integrity`.
    Broken {
        /// Why.
        cause: &'static str,
    },
}

const MAGIC: &[u8; 16] = b"SQLite format 3\0";

impl Store {
    /// Looks at `path` for `domain` and changes nothing: the file stays byte-identical, no file is left behind.
    ///
    /// A file that is empty, shorter than a header, or foreign (another domain, no registry) is `Broken`: it is not a store
    /// this program may open. A damaged copy that makes the engine fail or panic is `Broken { cause: "open" }`.
    ///
    /// # Errors
    /// Never for a broken file (that is `Broken`); `Unavailable` only when the folder cannot be written for the probe copy.
    pub async fn inspect(path: &str, domain: &Domain) -> Result<Inspect> {
        let p = Path::new(path);
        let Ok(meta) = std::fs::symlink_metadata(p) else {
            return Ok(Inspect::Absent);
        };
        let meta = if meta.file_type().is_symlink() {
            match std::fs::metadata(p) {
                Ok(m) => m,
                Err(_) => {
                    return Ok(Inspect::Broken {
                        cause: "not-regular",
                    });
                }
            }
        } else {
            meta
        };
        if !meta.is_file() {
            return Ok(Inspect::Broken {
                cause: "not-regular",
            });
        }
        if !header_ok(p) {
            return Ok(Inspect::Broken { cause: "header" });
        }
        let dir = scratch_dir(p)?;
        let outcome = probe(p, &dir, domain).await;
        let _ = std::fs::remove_dir_all(&dir);
        outcome
    }
}

fn header_ok(p: &Path) -> bool {
    use std::io::Read;
    let mut head = [0_u8; 16];
    std::fs::File::open(p)
        .and_then(|mut f| f.read_exact(&mut head))
        .is_ok()
        && &head == MAGIC
}

/// A fresh folder beside the file (same disk, never `/tmp`), named by a random id.
fn scratch_dir(p: &Path) -> Result<PathBuf> {
    let parent = p
        .parent()
        .filter(|d| !d.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let id = id_text(new_id(0)?);
    let dir = parent.join(format!(".inspect-{id}"));
    std::fs::create_dir(&dir).map_err(|e| Error::Unavailable(format!("probe folder: {e}")))?;
    Ok(dir)
}

async fn probe(p: &Path, dir: &Path, d: &Domain) -> Result<Inspect> {
    let copy = dir.join("probe.db");
    if std::fs::copy(p, &copy).is_err() {
        return Ok(Inspect::Broken { cause: "open" });
    }
    let mut wal = p.as_os_str().to_owned();
    wal.push("-wal");
    if Path::new(&wal).is_file() {
        let _ = std::fs::copy(&wal, dir.join("probe.db-wal"));
    }
    let name = copy.to_string_lossy().into_owned();
    let prefix = d.prefix;
    let fingerprints: Vec<String> = d
        .steps
        .iter()
        .map(super::migrate::Step::fingerprint)
        .collect();
    // A panic in the engine on a damaged copy must not take the caller down: isolate it in a task.
    let task = tokio::spawn(async move { look(&name, prefix, &fingerprints).await });
    Ok(task.await.unwrap_or(Inspect::Broken { cause: "open" }))
}

async fn rows(c: &Connection, sql: &str, params: Vec<Value>) -> Option<Vec<Row>> {
    let mut r = c.query(sql, turso::params_from_iter(params)).await.ok()?;
    let mut out = Vec::new();
    while let Some(row) = r.next().await.ok()? {
        out.push(
            (0..row.column_count())
                .map(|i| row.get_value(i))
                .collect::<std::result::Result<Row, _>>()
                .ok()?,
        );
    }
    Some(out)
}

async fn look(copy: &str, prefix: &str, prints: &[String]) -> Inspect {
    let broken = |cause| Inspect::Broken { cause };
    let Ok(db) = Builder::new_local(copy).build().await else {
        return broken("open");
    };
    let Ok(c) = db.connect() else {
        return broken("open");
    };
    let Some(check) = rows(&c, "PRAGMA integrity_check", Vec::new()).await else {
        return broken("open");
    };
    if check.len() != 1 || get_text(&check[0], 0).as_deref() != Ok("ok") {
        return broken("integrity");
    }
    let Some(domains) = rows(&c, "SELECT prefix, version FROM fs_domain", Vec::new()).await else {
        return broken("registry");
    };
    let known = prints.len();
    let mut found = 0;
    for r in &domains {
        let (Ok(pre), Ok(v)) = (get_text(r, 0), get_int(r, 1)) else {
            return broken("registry");
        };
        if pre == prefix {
            found = usize::try_from(v).unwrap_or(usize::MAX);
        } else if pre != OWN {
            return broken("registry");
        }
    }
    if found > known {
        return Inspect::Newer { found, known };
    }
    let Some(steps) = rows(
        &c,
        "SELECT seq, fingerprint FROM fs_step WHERE domain = ? ORDER BY seq",
        vec![text(prefix)],
    )
    .await
    else {
        return broken("registry");
    };
    if steps.len() < found {
        return broken("registry");
    }
    for (n, r) in steps.iter().enumerate().take(found) {
        let same = get_int(r, 0) == Ok(i64::try_from(n + 1).unwrap_or(-1))
            && prints
                .get(n)
                .is_some_and(|f| get_text(r, 1).as_deref() == Ok(f));
        if !same {
            return broken("steps");
        }
    }
    Inspect::Fits { found }
}
