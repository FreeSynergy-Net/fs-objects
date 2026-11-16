//! R1: the registry of domains and columns (`fs_domain`, `fs_column`), and the registration of a domain.
//!
//! Pattern: Registry (a program reports its table family, schema version and migration steps; the store applies the
//! missing steps and writes the tables down). A domain's schema version is the number of its steps already applied.

use turso::Value;

use super::codec::{get_int, get_text, int, invalid, text};
use super::conn::Conn;
use super::error::{Error, Result};
use super::id::new_id;
use super::migrate::Step;
use super::schema;
use super::spec::{
    Col,
    Logical::{Bool, Int, Text, Utc},
    Table,
};
use super::store::Store;
use super::tables_more::ALL as OBJECT_TABLES;

/// The registry's own tables.
pub const FS_DOMAIN: Table = Table {
    name: "fs_domain",
    cols: &[
        Col::req("prefix", Text),
        Col::req("version", Int),
        Col::req("description", Text),
        Col::req("applied", Utc),
    ],
    constraints: &["PRIMARY KEY (prefix)"],
    indexes: &[],
};
/// One row per column of every registered table.
pub const FS_COLUMN: Table = Table {
    name: "fs_column",
    cols: &[
        Col::req("domain", Text),
        Col::req("tbl", Text),
        Col::req("col", Text),
        Col::req("logical", Text),
        Col::req("nullable", Bool),
        Col::req("derived", Bool),
    ],
    constraints: &["PRIMARY KEY (domain, tbl, col)"],
    indexes: &[],
};

/// The prefixes a program may not take (draft R1).
pub const RESERVED: [&str; 5] = ["object", "activity", "person", "fs", "setting"];
/// The reserved domain that holds the store's own tables.
pub const OWN: &str = "fs_";

/// Everything a program tells the store about its tables.
#[derive(Clone, Copy)]
pub struct Domain {
    /// The table family: every table name starts with `<prefix>_`.
    pub prefix: &'static str,
    /// One line about the domain (`fs_domain.description`).
    pub about: &'static str,
    /// The tables as they are after the last step (written to `fs_column`).
    pub tables: &'static [Table],
    /// The channel names `object_origin.channel` may hold for this program (general names, see `COMMON_CHANNELS`).
    pub channels: &'static [&'static str],
    /// The ordered steps; the schema version is their number, only forward, only appended.
    pub steps: &'static [Step],
}

/// The fingerprint of every applied step (reordered, inserted or changed steps are found by it).
pub const FS_STEP: Table = Table {
    name: "fs_step",
    cols: &[
        Col::req("domain", Text),
        Col::req("seq", Int),
        Col::req("fingerprint", Text),
    ],
    constraints: &[
        "PRIMARY KEY (domain, seq)",
        "CHECK (length(fingerprint) = 64 AND fingerprint NOT GLOB '*[^0-9a-f]*')",
    ],
    indexes: &[],
};

const N_OWN: usize = OBJECT_TABLES.len() + 3;
const OWN_TABLES: [Table; N_OWN] = {
    let mut t = [FS_DOMAIN; N_OWN];
    let mut i = 0;
    while i < OBJECT_TABLES.len() {
        t[i] = OBJECT_TABLES[i];
        i += 1;
    }
    t[i] = FS_DOMAIN;
    t[i + 1] = FS_COLUMN;
    t[i + 2] = FS_STEP;
    t
};
const OWN_STEPS: [Step; 1] = [Step::Create(&OWN_TABLES)];
const OWN_DOMAIN: Domain = Domain {
    prefix: OWN,
    about: "tables of the store itself",
    tables: &OWN_TABLES,
    channels: &[],
    steps: &OWN_STEPS,
};

/// Create the registry and the store's own tables and register them under the reserved domain.
pub(super) async fn bootstrap(store: &Store, now: i64) -> Result<()> {
    let c = store.begin().await?;
    let r = async {
        for t in [FS_DOMAIN, FS_COLUMN, FS_STEP] {
            c.run(&t.ddl()).await?;
        }
        Ok(())
    }
    .await;
    c.finish(r).await?;
    run(store, &OWN_DOMAIN, now).await?;
    let c = store.begin().await?;
    let r = async {
        let id = super::codec::id_text(new_id(now)?);
        c.exec(
            "INSERT INTO meta (key, value) VALUES ('store_id', ?) ON CONFLICT (key) DO NOTHING",
            vec![text(&id)],
        )
        .await
        .map(|_| ())
    }
    .await;
    c.finish(r).await
}

/// Register a program's domain (see [`Store::register_domain`]).
pub(super) async fn register(store: &Store, d: &Domain, now: i64) -> Result<()> {
    let shape = !d.prefix.is_empty()
        && d.prefix
            .chars()
            .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_');
    if !shape || d.prefix.ends_with('_') || RESERVED.contains(&d.prefix) {
        return Err(invalid("prefix", "reserved or malformed"));
    }
    let lead = format!("{}_", d.prefix);
    if let Some(t) = d.tables.iter().find(|t| !t.name.starts_with(&lead)) {
        return Err(invalid(
            "tables",
            &format!("{} does not start with {lead}", t.name),
        ));
    }
    if d.steps.is_empty() {
        return Err(invalid("steps", "a domain needs at least one step"));
    }
    run(store, d, now).await
}

async fn run(store: &Store, d: &Domain, now: i64) -> Result<()> {
    let have = version(&store.read().await, d.prefix).await?;
    if have > d.steps.len() {
        return Err(Error::Schema(format!(
            "store schema of {} is version {have}, this program knows {}",
            d.prefix,
            d.steps.len()
        )));
    }
    check_prints(&store.read().await, d, have).await?;
    for (i, step) in d.steps.iter().enumerate().skip(have) {
        let c = store.begin().await?;
        let r = apply(&c, d, i, step, have == 0, now).await;
        c.finish(r).await?;
    }
    if have == d.steps.len() {
        // A program that went back to its last good version after a failed later step: the columns were never written.
        let c = store.begin().await?;
        let r = async {
            let n = c
                .query(
                    "SELECT 1 FROM fs_column WHERE domain = ? LIMIT 1",
                    vec![text(d.prefix)],
                )
                .await?;
            if n.is_empty() {
                record_columns(&c, d).await
            } else {
                Ok(())
            }
        }
        .await;
        c.finish(r).await?;
    }
    let c = store.read().await;
    same_tables(&c, d).await?;
    schema::check_structure(&c, d.tables).await
}

/// The schema version of a domain: the number of its applied steps (0 = not registered).
async fn version(c: &Conn<'_>, prefix: &str) -> Result<usize> {
    let rows = c
        .query(
            "SELECT version FROM fs_domain WHERE prefix = ?",
            vec![text(prefix)],
        )
        .await?;
    let v = rows
        .first()
        .map(|r| get_int(r, 0))
        .transpose()?
        .unwrap_or(0);
    Ok(usize::try_from(v).unwrap_or(usize::MAX))
}

/// The steps applied so far must be the ones the program reports now.
async fn check_prints(c: &Conn<'_>, d: &Domain, have: usize) -> Result<()> {
    let rows = c
        .query(
            "SELECT seq, fingerprint FROM fs_step WHERE domain = ? ORDER BY seq",
            vec![text(d.prefix)],
        )
        .await?;
    for (n, row) in rows.iter().enumerate().take(have) {
        let seq = usize::try_from(get_int(row, 0)?).unwrap_or(usize::MAX);
        let same = d
            .steps
            .get(n)
            .is_some_and(|s| s.fingerprint() == get_text(row, 1).unwrap_or_default());
        if seq != n + 1 || !same {
            return Err(Error::StepMismatch {
                domain: d.prefix.to_owned(),
                step: n + 1,
            });
        }
    }
    if rows.len() < have {
        return Err(Error::Schema(format!(
            "{}: fingerprints of applied steps are missing",
            d.prefix
        )));
    }
    Ok(())
}

async fn apply(
    c: &Conn<'_>,
    d: &Domain,
    i: usize,
    step: &Step,
    fresh: bool,
    now: i64,
) -> Result<()> {
    // Read again inside the transaction: another registration may have applied this step since we looked.
    let now_at = version(c, d.prefix).await?;
    if now_at > i {
        return check_prints(c, d, now_at).await;
    }
    if now_at < i {
        return Err(Error::Schema(format!(
            "{}: step {} is missing before step {}",
            d.prefix,
            now_at + 1,
            i + 1
        )));
    }
    step.apply(c).await.map_err(|e| match e {
        Error::Schema(_) => e,
        other => Error::Schema(format!("{} step {}: {other}", d.prefix, i + 1)),
    })?;
    let seq = int(i64::try_from(i + 1).unwrap_or(i64::MAX));
    c.exec(
        "INSERT INTO fs_domain (prefix, version, description, applied) VALUES (?, ?, ?, ?) \
         ON CONFLICT (prefix) DO UPDATE SET version = excluded.version, applied = excluded.applied",
        vec![text(d.prefix), seq.clone(), text(d.about), int(now)],
    )
    .await?;
    c.exec(
        "INSERT INTO fs_step (domain, seq, fingerprint) VALUES (?, ?, ?)",
        vec![text(d.prefix), seq, text(&step.fingerprint())],
    )
    .await?;
    if i + 1 == d.steps.len() {
        record_columns(c, d).await?;
        schema::check_structure(c, d.tables).await?;
        if fresh {
            schema::json_in_text(c, d.tables).await?;
        }
    }
    Ok(())
}

/// Write the final shape of the domain to `fs_column`.
async fn record_columns(c: &Conn<'_>, d: &Domain) -> Result<()> {
    c.exec(
        "DELETE FROM fs_column WHERE domain = ?",
        vec![text(d.prefix)],
    )
    .await?;
    for t in d.tables {
        for col in t.cols {
            let row = vec![
                text(d.prefix),
                text(t.name),
                text(col.name),
                text(col.logical.label()),
                Value::Integer(i64::from(col.null)),
                Value::Integer(i64::from(col.derived)),
            ];
            c.insert(&FS_COLUMN, row).await?;
        }
    }
    Ok(())
}

/// The recorded tables and columns must be the declared ones (a program that changed its tables without a new step is
/// refused).
async fn same_tables(c: &Conn<'_>, d: &Domain) -> Result<()> {
    let rows = c
        .query(
            "SELECT tbl, col, logical, nullable, derived FROM fs_column WHERE domain = ? ORDER BY tbl, col",
            vec![text(d.prefix)],
        )
        .await?;
    let mut known = Vec::new();
    for r in &rows {
        known.push((
            get_text(r, 0)?,
            get_text(r, 1)?,
            get_text(r, 2)?,
            get_int(r, 3)?,
            get_int(r, 4)?,
        ));
    }
    let mut declared: Vec<_> = d
        .tables
        .iter()
        .flat_map(|t| {
            t.cols.iter().map(|k| {
                (
                    t.name.to_owned(),
                    k.name.to_owned(),
                    k.logical.label().to_owned(),
                    i64::from(k.null),
                    i64::from(k.derived),
                )
            })
        })
        .collect();
    declared.sort();
    if known == declared {
        Ok(())
    } else {
        Err(invalid(
            "prefix",
            "already registered with other tables or columns; a change needs a new step",
        ))
    }
}
