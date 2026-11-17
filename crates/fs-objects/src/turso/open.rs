//! Opening a [`TursoStore`]: file, in-memory, with options.
//!
//! Pattern: Factory — constructors that choose location, encryption and policy and return a
//! ready store (schema migrated, last activity id loaded).

use std::path::Path;
use std::sync::atomic::AtomicBool;

use tokio::sync::{Mutex, broadcast};

use super::TursoStore;
use super::activities::last_activity_id;
use super::encryption;
use super::identity::read_identity;
use super::migration::{detect_version, migrate};
use super::options::OpenOptions;
use super::schema::SCHEMA_VERSION;
use super::store::Inner;
use crate::{Error, Result};

impl TursoStore {
    /// Open (creating if needed) a store in the file at `path`.
    pub async fn open(path: impl AsRef<Path>) -> Result<Self> {
        Self::open_with(path, OpenOptions::default()).await
    }

    /// Open with options (encryption, sensitive-data policy).
    pub async fn open_with(path: impl AsRef<Path>, opts: OpenOptions) -> Result<Self> {
        let p = path
            .as_ref()
            .to_str()
            .ok_or_else(|| Error::Invalid("path is not UTF-8".into()))?;
        Self::open_raw(p, opts, false).await
    }

    /// Open a file ONLY for reading (the engine opens it without write access) and refuse it unless its schema
    /// version is exactly the current one. Nothing is migrated, no identity is made: the file is never written
    /// (the migration to v5 reads its v4 source through this, ADR 0086). The file must exist.
    ///
    /// # Errors
    /// `Backend` when the file is no store, is empty, has another schema version, or cannot be opened.
    pub async fn open_read_only(path: impl AsRef<Path>, opts: OpenOptions) -> Result<Self> {
        let p = path
            .as_ref()
            .to_str()
            .ok_or_else(|| Error::Invalid("path is not UTF-8".into()))?;
        Self::open_raw(p, opts, true).await
    }

    /// Volatile in-memory store (tests, caches).
    pub async fn open_in_memory() -> Result<Self> {
        let o = OpenOptions {
            allow_sensitive_unencrypted: true,
            ..OpenOptions::default()
        };
        Self::open_raw(":memory:", o, false).await
    }

    async fn open_raw(path: &str, opts: OpenOptions, read_only: bool) -> Result<Self> {
        let (b, encrypted) = encryption::configure(
            turso::Builder::new_local(path)
                .experimental_vacuum(true)
                .read_only(read_only),
            &opts,
        )?;
        let db = b.build().await?;
        let conn = db.connect()?;
        if read_only {
            let found = detect_version(&conn).await?;
            if found != SCHEMA_VERSION {
                return Err(Error::Backend(format!(
                    "the file has schema version {found}, expected {SCHEMA_VERSION}: it is not opened"
                )));
            }
        } else {
            migrate(&conn).await?;
        }
        let identity = read_identity(&conn).await?;
        let last_activity = last_activity_id(&conn).await?;
        let (tx, _) = broadcast::channel(opts.subscribe_capacity.max(1));
        Ok(Self {
            _db: db,
            inner: Mutex::new(Inner {
                conn,
                last_activity,
            }),
            tx,
            encrypted,
            allow_sensitive_unencrypted: opts.allow_sensitive_unencrypted,
            fail_before_activity: AtomicBool::new(false),
            fail_before_extra: AtomicBool::new(false),
            fail_in_purge: AtomicBool::new(false),
            on_disk: path != ":memory:",
            identity,
        })
    }
}
