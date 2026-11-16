//! One ordered list of migration steps per domain: SQL or code, forward only, each in its own transaction.
//!
//! Pattern: Command list (a step is data the registry runs; it never decides when).

use std::future::Future;
use std::pin::Pin;

use super::conn::Conn;
use super::error::Result;
use super::spec::Table;

/// The future of a code step.
pub type StepFuture<'a> = Pin<Box<dyn Future<Output = Result<()>> + Send + 'a>>;

/// A code step: runs inside the transaction the registry opened for it.
pub type StepFn = for<'a, 'c> fn(&'a Conn<'c>) -> StepFuture<'a>;

/// One migration step.
#[derive(Clone, Copy)]
pub enum Step {
    /// Create these tables and their indexes (`CREATE ... IF NOT EXISTS`, every table `STRICT`).
    Create(&'static [Table]),
    /// One SQL statement (`ALTER TABLE`, a data fix).
    Sql(&'static str),
    /// Code, for what SQL alone cannot do (moving data between tables).
    Code(StepFn),
}

impl Step {
    /// Run the step on the registry's connection.
    ///
    /// # Errors
    /// The engine's error or the code's.
    pub async fn apply(&self, c: &Conn<'_>) -> Result<()> {
        match self {
            Self::Create(tables) => {
                for t in *tables {
                    c.run(&t.ddl()).await?;
                    for ix in t.indexes {
                        c.run(ix).await?;
                    }
                }
                Ok(())
            }
            Self::Sql(sql) => c.run(sql).await,
            Self::Code(f) => f(c).await,
        }
    }
}
