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
    /// The ordered steps; the schema version is their number, only forward.
    pub steps: &'static [Step],
}

const OWN_TABLES: [Table; 11] = {
    let mut t = [FS_DOMAIN; 11];
    let mut i = 0;
    while i < 9 {
        t[i] = OBJECT_TABLES[i];
        i += 1;
    }
    t[9] = FS_DOMAIN;
    t[10] = FS_COLUMN;
    t
};
const OWN_STEPS: [Step; 1] = [Step::Create(&OWN_TABLES)];
const OWN_DOMAIN: Domain = Domain {
    prefix: OWN,
    about: "tables of the store itself",
    tables: &OWN_TABLES,
    steps: &OWN_STEPS,
};

/// Create the registry and the store's own tables and register them under the reserved domain.
pub(super) async fn bootstrap(store: &Store, now: i64) -> Result<()> {
    let c = store.begin().await?;
    let r = async {
        c.run(&FS_DOMAIN.ddl()).await?;
        c.run(&FS_COLUMN.ddl()).await
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
    let have = {
        let c = store.read().await;
        let rows = c
            .query(
                "SELECT version FROM fs_domain WHERE prefix = ?",
                vec![text(d.prefix)],
            )
            .await?;
        rows.first()
            .map(|r| get_int(r, 0))
            .transpose()?
            .unwrap_or(0)
    };
    let have = usize::try_from(have).unwrap_or(usize::MAX);
    if have > d.steps.len() {
        return Err(Error::Schema(format!(
            "store schema of {} is version {have}, this program knows {}",
            d.prefix,
            d.steps.len()
        )));
    }
    for (i, step) in d.steps.iter().enumerate().skip(have) {
        let c = store.begin().await?;
        let r = apply(&c, d, i, step, now).await;
        c.finish(r).await?;
    }
    let c = store.read().await;
    same_tables(&c, d).await
}

async fn apply(c: &Conn<'_>, d: &Domain, i: usize, step: &Step, now: i64) -> Result<()> {
    step.apply(c).await.map_err(|e| match e {
        Error::Schema(_) => e,
        other => Error::Schema(format!("{} step {}: {other}", d.prefix, i + 1)),
    })?;
    let version = int(i64::try_from(i + 1).unwrap_or(i64::MAX));
    c.exec(
        "INSERT INTO fs_domain (prefix, version, description, applied) VALUES (?, ?, ?, ?) \
         ON CONFLICT (prefix) DO UPDATE SET version = excluded.version, applied = excluded.applied",
        vec![text(d.prefix), version, text(d.about), int(now)],
    )
    .await?;
    if i + 1 == d.steps.len() {
        record_columns(c, d).await?;
    }
    Ok(())
}

/// Write the final shape of the domain to `fs_column`, after checking that every table really exists.
async fn record_columns(c: &Conn<'_>, d: &Domain) -> Result<()> {
    c.exec(
        "DELETE FROM fs_column WHERE domain = ?",
        vec![text(d.prefix)],
    )
    .await?;
    for t in d.tables {
        let rows = c
            .query(
                "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?",
                vec![text(t.name)],
            )
            .await?;
        if rows.is_empty() {
            return Err(Error::Schema(format!(
                "table {} is declared but no step creates it",
                t.name
            )));
        }
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

/// The recorded tables must be the declared ones (a program that changed its tables without a new step is refused).
async fn same_tables(c: &Conn<'_>, d: &Domain) -> Result<()> {
    let rows = c
        .query(
            "SELECT DISTINCT tbl FROM fs_column WHERE domain = ? ORDER BY tbl",
            vec![text(d.prefix)],
        )
        .await?;
    let known = rows
        .iter()
        .map(|r| get_text(r, 0))
        .collect::<Result<Vec<_>>>()?;
    let mut names: Vec<&str> = d.tables.iter().map(|t| t.name).collect();
    names.sort_unstable();
    if known == names {
        Ok(())
    } else {
        Err(invalid(
            "prefix",
            "already registered with other tables; a change of tables needs a new step",
        ))
    }
}
