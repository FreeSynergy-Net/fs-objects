//! Migration of a v4 store file (`objects.content` = JSON) to a v5 file (typed tables), as a scaffold (v0.0.7):
//! backup, a NEW v5 file, one [`Converter`] per program, a round trip per object, a written report and a switch
//! that needs the human's confirmation. Data is never converted automatically and never lost: what does not
//! round-trip is HELD BACK (listed with a reason) and stays in the v4 file, which is never changed or deleted here.
//!
//! Pattern: State machine ([`MigrationPlan`]: Checking, Ready, Confirmed, Switched, `FellBack`) around a Strategy
//! ([`Converter`], one per program). What is carried in v0.0.7: the envelope (time rounded from microseconds to
//! milliseconds and counted), the program's typed rows and one `Create` log entry per object with the converter's
//! field changes. What is not carried yet (held or reported, never silent): addressing, protection, the original,
//! the v4 activity history, ids of the context that are no object ids.
//!
//! The report is a document (text), not stored data; the state file is a few `key=value` lines.

mod backup;
mod build;
mod carry;
mod convert;
mod lock;
pub mod notes;
mod plan;
mod report;
mod state;

pub use backup::{backup_file, fingerprint, unique_sibling};
pub use convert::{Converted, Converter, Hold, TableRow};
pub use lock::SourceLock;
pub use plan::{MigrationPlan, Phase};
pub use report::{Held, Report};

fn io(what: &str, e: &std::io::Error) -> crate::v5::error::Error {
    crate::v5::error::Error::Unavailable(format!("{what}: {e}"))
}
