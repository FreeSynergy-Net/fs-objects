//! The period run: what a deadline made final (ADR 0073). A domain reports which of its tables hold deadlines; the
//! store removes the due rows, calling the domain's hook per row first (the hook forgets what hangs on the row).
//!
//! Pattern: Template method (the scaffold walks the tables; the hook is the program's step).

use std::future::Future;
use std::pin::Pin;

use fs_types::PersonRef;

use super::codec::{Row, int};
use super::conn::Conn;
use super::envelope::{delete_final, due};
use super::error::Result;
use super::registry::Domain;

/// What a hook really did: a kind of its own naming and how many rows besides the due row it changed or removed
/// (`0` when it ended early). The run adds them up per kind in `Swept::changed`.
pub type Done = (&'static str, usize);

/// The future of a hook.
pub type HookFuture<'a> = Pin<Box<dyn Future<Output = Result<Done>> + Send + 'a>>;

/// A hook for one expired row (the key columns in the order of `Expiry::key`, the time of the run, the actor of the
/// run); it runs before the row is deleted. A hook that changes rows of an object must carry the envelope along
/// (`envelope::touch`): never leave a changed title in `objects.name`.
pub type ExpiredFn = for<'a, 'c> fn(&'a Conn<'c>, &'a Row, i64, &'a PersonRef) -> HookFuture<'a>;

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
    /// What the hooks reported as really changed or removed, summed per kind (in the order the kinds first came up).
    pub changed: Vec<(&'static str, usize)>,
    /// Due rows per table, in the order of `Domain::expiries`.
    pub rows: Vec<(&'static str, usize)>,
}

impl Swept {
    fn add(&mut self, kind: &'static str, n: usize) {
        match self.changed.iter_mut().find(|(k, _)| *k == kind) {
            Some((_, sum)) => *sum += n,
            None => self.changed.push((kind, n)),
        }
    }

    /// The rows the hooks changed or removed of this kind (0 when none reported it).
    #[must_use]
    pub fn changed_of(&self, kind: &str) -> usize {
        self.changed
            .iter()
            .find(|(k, _)| *k == kind)
            .map_or(0, |(_, n)| *n)
    }
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
                let (kind, n) = hook(c, row, now, actor).await?;
                out.add(kind, n);
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
