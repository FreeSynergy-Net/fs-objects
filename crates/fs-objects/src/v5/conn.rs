//! The connection for one unit of work, with the drop guard.
//!
//! Pattern: Adapter (the turso engine behind typed helpers); `Conn` is a guard that owns the connection for one unit
//! of work. `finish` commits or rolls back; a guard dropped without `finish` leaves the transaction open and marks the
//! store, so the next access rolls back (K1).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use tokio::sync::MutexGuard;
use turso::{Connection, Value};

use super::codec::Row;
use super::error::Result;
use super::spec::Table;
use super::store::engine;
use super::typed::check_row;

/// The connection for one unit of work.
pub struct Conn<'a> {
    pub(super) guard: MutexGuard<'a, Connection>,
    pub(super) in_tx: bool,
    pub(super) dirty: Arc<AtomicBool>,
}

impl Drop for Conn<'_> {
    fn drop(&mut self) {
        if self.in_tx {
            self.dirty.store(true, Ordering::SeqCst);
        }
    }
}

impl Conn<'_> {
    /// Rows of a query.
    ///
    /// # Errors
    /// The engine's error.
    pub async fn query(&self, sql: &str, params: Vec<Value>) -> Result<Vec<Row>> {
        let mut rows = self
            .guard
            .query(sql, turso::params_from_iter(params))
            .await
            .map_err(|e| engine(&e))?;
        let mut out = Vec::new();
        while let Some(row) = rows.next().await.map_err(|e| engine(&e))? {
            let mut r = Vec::with_capacity(row.column_count());
            for i in 0..row.column_count() {
                r.push(row.get_value(i).map_err(|e| engine(&e))?);
            }
            out.push(r);
        }
        Ok(out)
    }

    /// Execute a statement; the number of rows it changed.
    ///
    /// # Errors
    /// The engine's error.
    pub async fn exec(&self, sql: &str, params: Vec<Value>) -> Result<u64> {
        self.guard
            .execute(sql, turso::params_from_iter(params))
            .await
            .map_err(|e| engine(&e))
    }

    /// Execute a statement without parameters.
    ///
    /// # Errors
    /// The engine's error.
    pub async fn run(&self, sql: &str) -> Result<()> {
        self.exec(sql, Vec::new()).await.map(|_| ())
    }

    /// Insert one row after the checks of R2 (`values` in column order).
    ///
    /// # Errors
    /// `Invalid` naming `table.column`.
    pub async fn insert(&self, t: &Table, values: Vec<Value>) -> Result<()> {
        check_row(t, &values)?;
        let names: Vec<&str> = t.cols.iter().map(|c| c.name).collect();
        let marks = vec!["?"; names.len()].join(",");
        self.exec(
            &format!(
                "INSERT INTO {} ({}) VALUES ({marks})",
                t.name,
                names.join(",")
            ),
            values,
        )
        .await
        .map(|_| ())
    }

    /// Insert or update on the primary key `key` (never `REPLACE`: that would cascade, draft R4).
    ///
    /// # Errors
    /// `Invalid` naming `table.column`.
    pub async fn upsert(&self, t: &Table, key: &[&str], values: Vec<Value>) -> Result<()> {
        check_row(t, &values)?;
        let names: Vec<&str> = t.cols.iter().map(|c| c.name).collect();
        let marks = vec!["?"; names.len()].join(",");
        let set: Vec<String> = names
            .iter()
            .filter(|n| !key.contains(n))
            .map(|n| format!("{n} = excluded.{n}"))
            .collect();
        let sql = format!(
            "INSERT INTO {} ({}) VALUES ({marks}) ON CONFLICT ({}) DO UPDATE SET {}",
            t.name,
            names.join(","),
            key.join(","),
            set.join(", ")
        );
        self.exec(&sql, values).await.map(|_| ())
    }

    /// End the unit of work: commit on `Ok`, roll back on `Err` (nothing is written anywhere).
    ///
    /// `finish` suspends once before the COMMIT (one scheduler turn per unit of work). That is a deliberate hook in the
    /// production path: it makes the cancel point real, so that `tests/transaction.rs` can drop the future exactly
    /// there and prove that the next access rolls back (K1).
    ///
    /// # Errors
    /// The error of `r`, or of the commit.
    pub async fn finish<T>(mut self, r: Result<T>) -> Result<T> {
        if !self.in_tx {
            return r;
        }
        // `in_tx` stays true until the engine has answered: a future cancelled before that point is dropped with
        // `in_tx` set, so `Drop` marks the connection and the next access rolls back (K1).
        Suspend(false).await;
        let out = match r {
            Ok(v) => match self.guard.execute("COMMIT", ()).await {
                Ok(_) => Ok(v),
                Err(e) => {
                    let _ = self.guard.execute("ROLLBACK", ()).await;
                    Err(engine(&e))
                }
            },
            Err(e) => {
                let _ = self.guard.execute("ROLLBACK", ()).await;
                Err(e)
            }
        };
        self.in_tx = false;
        out
    }
}

/// Gives the scheduler one turn (a real suspension point, so that a cancel there can be tested).
struct Suspend(bool);

impl std::future::Future for Suspend {
    type Output = ();
    fn poll(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<()> {
        if self.0 {
            return std::task::Poll::Ready(());
        }
        self.0 = true;
        cx.waker().wake_by_ref();
        std::task::Poll::Pending
    }
}
