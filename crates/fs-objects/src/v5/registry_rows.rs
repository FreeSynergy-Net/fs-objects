//! The reading and writing of the registry rows (`fs_domain`, `fs_step`, `fs_column`): version, step fingerprints, the
//! recorded shape of a domain. Split from `registry`.
//!
//! Pattern: Registry (queries over the registry's own tables).

use turso::Value;

use super::codec::{get_int, get_text, invalid, text};
use super::conn::Conn;
use super::error::{Error, Result};
use super::registry::{Domain, FS_COLUMN};

/// The schema version of a domain: the number of its applied steps (0 = not registered).
pub(super) async fn version(c: &Conn<'_>, prefix: &str) -> Result<usize> {
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
pub(super) async fn check_prints(c: &Conn<'_>, d: &Domain, have: usize) -> Result<()> {
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

/// Write the final shape of the domain to `fs_column`.
pub(super) async fn record_columns(c: &Conn<'_>, d: &Domain) -> Result<()> {
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
pub(super) async fn same_tables(c: &Conn<'_>, d: &Domain) -> Result<()> {
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
