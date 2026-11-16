//! The v5 typed-table layer (`docs/v5-entwurf.md`): what every program needs before it has a table of its own.
//!
//! Pattern: Facade over a small registry: a program reports its table family ([`Domain`]: prefix, schema version as
//! the number of steps, ordered migration [`Step`]s) and gets a [`Store`] with a transaction guard ([`Conn`]),
//! `UUIDv7` ids ([`new_id`]), typed rows ([`Table`]) and the shared tables ([`tables`], [`tables_more`]). Step L1 of the
//! lift from `fs-calendar-local`; trash, revision, log, original and purge follow (L2). Types come from `fs-types`
//! (draft rule R13); the v4 API of this crate is untouched.

pub mod codec;
pub mod conn;
pub mod error;
pub mod id;
pub mod migrate;
pub mod registry;
pub mod spec;
pub mod store;
pub mod tables;
pub mod tables_more;
pub mod typed;

pub use conn::Conn;
pub use error::{Error, Result};
pub use id::{Clock, new_id};
pub use migrate::Step;
pub use registry::Domain;
pub use spec::{Col, Logical, Table};
pub use store::Store;
