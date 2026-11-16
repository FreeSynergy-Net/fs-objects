//! The v5 store: one connection with `PRAGMA foreign_keys = ON`, one transaction per unit of work (R3, R4).
//!
//! Pattern: Adapter (the turso engine behind a mutex). [`Conn`] is the guard of one unit of work.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use tokio::sync::{Mutex, MutexGuard};
use turso::{Builder, Connection};

use super::codec::invalid;
use super::conn::Conn;
use super::error::{Error, Result};
use super::registry::{self, Domain};

/// The store.
pub struct Store {
    _db: turso::Database,
    conn: Mutex<Connection>,
    /// Set when a unit of work was dropped without `finish` (a cancelled future): the next access rolls back first.
    dirty: Arc<AtomicBool>,
}

/// An engine error as the layer's error: a refused constraint is `Invalid`, anything else `Unavailable`.
#[must_use]
pub fn engine(e: &turso::Error) -> Error {
    let msg = e.to_string();
    let low = msg.to_lowercase();
    if [
        "constraint failed",
        "constraint violation",
        "foreign key",
        "cannot store",
        "datatype mismatch",
    ]
    .iter()
    .any(|w| low.contains(w))
    {
        invalid("store", &msg)
    } else {
        Error::Unavailable(msg)
    }
}

impl Store {
    /// Open (creating if needed) the store at `path`; `:memory:` is volatile. Registers the store's own tables.
    ///
    /// # Errors
    /// `Unavailable` when the engine cannot open the file.
    pub async fn open(path: &str, now: i64) -> Result<Self> {
        let db = Builder::new_local(path)
            .build()
            .await
            .map_err(|e| engine(&e))?;
        let conn = db.connect().map_err(|e| engine(&e))?;
        conn.execute("PRAGMA foreign_keys = ON", ())
            .await
            .map_err(|e| engine(&e))?;
        let store = Self {
            _db: db,
            conn: Mutex::new(conn),
            dirty: Arc::new(AtomicBool::new(false)),
        };
        registry::bootstrap(&store, now).await?;
        Ok(store)
    }

    /// The connection; a transaction that a cancelled future left open is rolled back first (K1).
    async fn lock(&self) -> MutexGuard<'_, Connection> {
        let guard = self.conn.lock().await;
        if self.dirty.swap(false, Ordering::SeqCst) {
            let _ = guard.execute("ROLLBACK", ()).await;
        }
        guard
    }

    /// Lock the connection and open a transaction (`BEGIN IMMEDIATE`).
    ///
    /// # Errors
    /// `Unavailable` when the engine refuses.
    pub async fn begin(&self) -> Result<Conn<'_>> {
        let guard = self.lock().await;
        guard
            .execute("BEGIN IMMEDIATE", ())
            .await
            .map_err(|e| engine(&e))?;
        Ok(Conn {
            guard,
            in_tx: true,
            dirty: self.dirty.clone(),
        })
    }

    /// Lock the connection for reading (no transaction).
    pub async fn read(&self) -> Conn<'_> {
        Conn {
            guard: self.lock().await,
            in_tx: false,
            dirty: self.dirty.clone(),
        }
    }

    /// A program registers its domain: prefix, schema version and the ordered migration steps (R1). Steps not yet
    /// applied run forward, each in its own transaction.
    ///
    /// # Errors
    /// `Invalid` for a reserved or malformed prefix or a clash; `Schema` when a step fails or the store is newer.
    pub async fn register_domain(&self, domain: &Domain, now: i64) -> Result<()> {
        registry::register(self, domain, now).await
    }
}
