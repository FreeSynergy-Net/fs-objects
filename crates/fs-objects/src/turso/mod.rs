//! Embedded Turso backend of the [`Store`](crate::Store) traits.
//!
//! Pattern: Adapter — `turso` (a foreign SQL engine) behind our `Store` traits; one module per
//! concern so each trait impl stays small. See `DESIGN.md` for the pattern map.

mod activities;
mod codec;
mod compaction;
mod encryption;
mod fault;
mod guard;
mod identity;
mod info;
mod log_impl;
mod migration;
mod objects;
mod open;
mod options;
mod originals;
mod owner;
mod payload;
mod purge;
mod query_impl;
mod query_sql;
mod repository;
mod retention;
mod revert_by;
mod rows;
mod schema;
mod sql;
mod store;
mod subscribe;
mod tx;
mod undo;
mod unlogged;
mod validate;
mod write_by;
mod write_clash;

pub use options::OpenOptions;
pub use store::TursoStore;
