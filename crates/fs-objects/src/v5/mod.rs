//! The v5 typed-table layer (`docs/v5-entwurf.md`): what every program needs before it has a table of its own.
//!
//! Pattern: Facade over a small registry: a program reports its table family ([`Domain`]: prefix, schema version as
//! the number of steps, ordered migration [`Step`]s) and gets a [`Store`] with a transaction guard ([`Conn`]),
//! `UUIDv7` ids ([`new_id`]), typed rows ([`Table`]) and the shared tables ([`tables`], [`tables_more`]). Step L1 of the
//! lift from `fs-calendar-local`; step L2 adds the envelope (revision, trash, final delete), the typed log, original and
//! origin, the file store port and the period run. Types come from `fs-types`
//! (draft rule R13); the v4 API of this crate is untouched.

#![allow(clippy::many_single_char_names)]

pub mod codec;
pub mod conn;
pub mod envelope;
pub mod envelope_trash;
pub mod error;
pub mod files;
pub mod hold;
pub mod id;
pub mod log;
pub mod log_codec;
pub mod migrate;
pub mod original;
pub mod own;
pub mod purge;
pub mod registry;
pub mod registry_rows;
pub mod schema;
pub mod spec;
pub mod store;
pub mod tables;
pub mod tables_more;
pub mod typed;

pub use conn::Conn;
pub use error::{Error, Result};
pub use id::{Clock, new_id};
pub use migrate::Step;
pub use purge::Expiry;
pub use registry::{Child, Domain};
pub use spec::{Col, Logical, Table};
pub use store::Store;
