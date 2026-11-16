//! R1: the registry of domains and columns (`fs_domain`, `fs_column`), and the registration of a domain.
//!
//! Pattern: Registry (a program reports its table family, schema version and migration steps; the store applies the
//! missing steps and writes the tables down). A domain's schema version is the number of its steps already applied.

use super::codec::{get_text, int, invalid, text};
use super::conn::Conn;
pub use super::envelope::Child;
use super::error::{Error, Result};
use super::id::new_id;
use super::migrate::Step;
use super::own::OWN_DOMAIN;
pub use super::own::{FS_COLUMN, FS_DOMAIN, FS_STEP};
use super::purge::Expiry;
use super::registry_rows::{check_prints, record_columns, same_tables, version};
use super::schema;
use super::spec::Table;
use super::store::Store;

/// The prefixes a program may not take (draft R1).
pub const RESERVED: [&str; 5] = ["object", "activity", "person", "fs", "setting"];
/// The reserved domain that holds the store's own tables.
pub const OWN: &str = "fs_";

/// Everything a program tells the store about its tables. `non_exhaustive`: build it with [`Domain::new`] and the
/// `with_*` methods, so a field added later does not break the programs.
#[derive(Clone, Copy)]
#[non_exhaustive]
pub struct Domain {
    /// The table family: every table name starts with `<prefix>_`.
    pub prefix: &'static str,
    /// One line about the domain (`fs_domain.description`).
    pub about: &'static str,
    /// The tables as they are after the last step (written to `fs_column`).
    pub tables: &'static [Table],
    /// The channel names `object_origin.channel` may hold for this program (general names, see `COMMON_CHANNELS`).
    pub channels: &'static [&'static str],
    /// Dependent objects whose envelopes go with the object at the final delete (e.g. the exceptions of a series).
    pub children: &'static [Child],
    /// Tables with deadlines the period run removes (`purge::sweep`).
    pub expiries: &'static [Expiry],
    /// The ordered steps; the schema version is their number, only forward, only appended.
    pub steps: &'static [Step],
}

impl Domain {
    /// A domain with its prefix, one line about it, its tables after the last step and its ordered steps; no channels,
    /// no children, no expiries (add them with the `with_*` methods).
    #[must_use]
    pub const fn new(
        prefix: &'static str,
        about: &'static str,
        tables: &'static [Table],
        steps: &'static [Step],
    ) -> Self {
        Self {
            prefix,
            about,
            tables,
            channels: &[],
            children: &[],
            expiries: &[],
            steps,
        }
    }

    /// The channel names `object_origin.channel` may hold for this program.
    #[must_use]
    pub const fn with_channels(mut self, channels: &'static [&'static str]) -> Self {
        self.channels = channels;
        self
    }

    /// The dependent objects whose envelopes go with the object at the final delete.
    #[must_use]
    pub const fn with_children(mut self, children: &'static [Child]) -> Self {
        self.children = children;
        self
    }

    /// The tables with deadlines the period run removes.
    #[must_use]
    pub const fn with_expiries(mut self, expiries: &'static [Expiry]) -> Self {
        self.expiries = expiries;
        self
    }
}

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
    let other = store
        .read()
        .await
        .query(
            "SELECT prefix FROM fs_domain WHERE prefix NOT IN (?, ?) ORDER BY prefix",
            vec![text(OWN), text(d.prefix)],
        )
        .await?;
    if let Some(row) = other.first() {
        return Err(Error::SecondDomain {
            existing: get_text(row, 0)?,
            new: d.prefix.to_owned(),
        });
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
        let r = apply(&c, d, i, step, now).await;
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

async fn apply(c: &Conn<'_>, d: &Domain, i: usize, step: &Step, now: i64) -> Result<()> {
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
    }
    Ok(())
}
