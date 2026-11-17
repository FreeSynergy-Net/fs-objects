//! Typed row functions over the `settings_*` family (ADR 0100). fs-objects knows nothing of fs-settings (PLAN section 3.1 (e)):
//! the program maps these plain types 1:1 to its own.
//!
//! Pattern: Repository over a [`Store`] (one transaction per write: value, `settings_rev + 1`, log row), plus Value Objects for
//! the rows. Times are `fs_types::Utc` (ms). Errors: `Error::Invalid` with `table.column` and a reason code, never a sentence.
//! Written by Claude (Anthropic) as a probe; see CHANGELOG.

mod batch;
mod prim;
mod rows;
mod rows_index;
mod rows_instance;
mod rows_lock;
pub mod tables;

use fs_types::{ObjectId, Reference, Utc};

use super::Store;

pub use batch::SettingOp;
pub use tables::{DOMAIN, PREFIX};

/// A stored value: the three storage classes (no edge to fs-settings `Raw`; the server maps 1:1).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Prim {
    /// 0 or 1.
    Bool(bool),
    /// A whole number.
    Int(i64),
    /// Text.
    Text(String),
}

/// The scope of a person value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScopeRow<'a> {
    /// General (every program).
    General,
    /// A program name; the caller checked it (`ProgramName`); here only: not empty (CHECK).
    Program(&'a str),
}

/// Where a person value lives.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlaceRow {
    /// With the profile (travels with the person).
    Profile,
    /// On this device only.
    Device,
}

/// Who writes a person value: `None`/`None` = the person herself through no named program.
#[derive(Clone, Copy, Debug, Default)]
pub struct Writer<'a> {
    /// An actor IRI when it is not the person herself.
    pub by: Option<&'a Reference>,
    /// The program that wrote it.
    pub via: Option<&'a str>,
}

/// Who writes an instance row: actor IRI (mandatory) and the OS user the SERVICE found (never the caller's word).
#[derive(Clone, Copy, Debug)]
pub struct Actor<'a> {
    /// The actor.
    pub by: &'a Reference,
    /// The OS user.
    pub os_user: Option<&'a str>,
}

/// One row of the profile index.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProfileRow {
    /// The id (0 = base).
    pub id: i64,
    /// `None` for the base.
    pub name: Option<String>,
    /// When it was made.
    pub created: Utc,
    /// When it was deleted (a tombstone), else `None`.
    pub deleted: Option<Utc>,
}

/// The index as stored: all rows (tombstones too), the active id of this device, `top` = MAX(id).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IndexRows {
    /// All rows, by id.
    pub profiles: Vec<ProfileRow>,
    /// The active id of this device.
    pub active: i64,
    /// The highest id ever issued.
    pub top: i64,
}

/// One change of the index, applied inside the transaction of `SettingRows::change_index`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IndexChange {
    /// New named profile with exactly this id (must be > `top`; the caller computed it from `top`).
    Create {
        /// The id.
        id: i64,
        /// The name as written.
        name: String,
        /// The comparison form.
        name_key: String,
    },
    /// New name for a live profile.
    Rename {
        /// The id.
        id: i64,
        /// The name as written.
        name: String,
        /// The comparison form.
        name_key: String,
    },
    /// Tombstone (`deleted = at`, `name_key = NULL`) and ALL its values go (every scope); active -> base if it was.
    Delete {
        /// The id.
        id: i64,
    },
    /// Make this profile the active one of this device.
    Switch {
        /// The id.
        id: i64,
    },
}

/// One lock of an instance default.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LockRow {
    /// The key.
    pub key: String,
    /// The locked value.
    pub value: Prim,
    /// Why.
    pub reason: String,
    /// Who locked.
    pub by: Reference,
    /// When.
    pub at: Utc,
}

/// The instance layer as stored.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstanceRows {
    /// The defaults, by key.
    pub values: Vec<(String, Prim)>,
    /// The locks, by key.
    pub locks: Vec<LockRow>,
}

/// One row of the append-only instance log.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LogRow {
    /// The id (`UUIDv7`).
    pub id: ObjectId,
    /// When.
    pub at: Utc,
    /// Who.
    pub by: Reference,
    /// The OS user the service found.
    pub os_user: Option<String>,
    /// `set`, `clear`, `lock` or `unlock`.
    pub action: &'static str,
    /// The key.
    pub key: String,
    /// Why (mandatory for lock and unlock).
    pub reason: Option<String>,
    /// The value before.
    pub old: Option<Prim>,
    /// The value after.
    pub new: Option<Prim>,
}

/// Instance writes are idempotent: the same value again = `Same` (no new log row, no new rev).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Something changed; the new `rev`.
    Changed {
        /// The revision now.
        rev: i64,
    },
    /// Nothing changed; the unchanged `rev`.
    Same {
        /// The revision (unchanged).
        rev: i64,
    },
}

/// The typed row functions: a borrow of a store on which [`DOMAIN`] is registered (checked once in `new`).
pub struct SettingRows<'s> {
    pub(super) store: &'s Store,
}
