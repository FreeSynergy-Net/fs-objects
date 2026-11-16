//! The period run: what a deadline made final (ADR 0073). A domain reports which of its tables hold deadlines; the
//! store removes the due rows, calling the domain's hook per row first (the hook forgets what hangs on the row).
//!
//! Pattern: Template method (the scaffold walks the tables; the hook is the program's step).

use fs_types::PersonRef;

use super::codec::{Row, int};
use super::conn::Conn;
use super::envelope::{delete_final, due};
use super::error::Result;
use super::migrate::StepFuture;
use super::registry::Domain;

/// A hook for one expired row (the key columns in the order of `Expiry::key`, the time of the run, the actor of the
/// run); it runs before the row is deleted. A hook that changes rows of an object must carry the envelope along
/// (`envelope::touch`): never leave a changed title in `objects.name`.
pub type ExpiredFn = for<'a, 'c> fn(&'a Conn<'c>, &'a Row, i64, &'a PersonRef) -> StepFuture<'a>;

/// A table with a deadline column.
#[derive(Clone, Copy)]
pub struct Expiry {
    /// The table.
    pub table: &'static str,
    /// The column with the deadline (UTC milliseconds); rows with `until <= now` are due.
    pub until: &'static str,
    /// The key columns that identify a row.
    pub key: &'static [&'static str],
    /// What to do with a due row before it is deleted.
    pub on_expired: Option<ExpiredFn>,
}

/// An object whose trash period is over, with the dependents the domain reports, and the rows past their deadline.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Swept {
    /// Objects removed for good (end of the trash).
    pub objects: usize,
    /// Due rows per table, in the order of `Domain::expiries`.
    pub rows: Vec<(&'static str, usize)>,
}

/// Run the deadlines of a domain: first the expiry tables, then the final delete of every object whose trash period
/// is over. All in the caller's transaction.
///
/// # Errors
/// The engine's error or the error of a hook.
pub async fn sweep(c: &Conn<'_>, d: &Domain, now: i64, actor: &PersonRef) -> Result<Swept> {
    let mut out = Swept::default();
    for e in d.expiries {
        let cols = e.key.join(", ");
        let rows = c
            .query(
                &format!("SELECT {cols} FROM {} WHERE {} <= ?", e.table, e.until),
                vec![int(now)],
            )
            .await?;
        for row in &rows {
            if let Some(hook) = e.on_expired {
                hook(c, row, now, actor).await?;
            }
            let cond: Vec<String> = e.key.iter().map(|k| format!("{k} = ?")).collect();
            c.exec(
                &format!("DELETE FROM {} WHERE {}", e.table, cond.join(" AND ")),
                row.clone(),
            )
            .await?;
        }
        out.rows.push((e.table, rows.len()));
    }
    for id in due(c, now).await? {
        delete_final(c, d, id).await?;
        out.objects += 1;
    }
    Ok(out)
}
