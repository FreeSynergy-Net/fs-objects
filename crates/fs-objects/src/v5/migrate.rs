//! One ordered list of migration steps per domain: SQL or code, forward only, each in its own transaction.
//!
//! Pattern: Command list (a step is data the registry runs; it never decides when).

use std::future::Future;
use std::pin::Pin;

use sha2::{Digest, Sha256};

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
    /// Code, for what SQL alone cannot do (moving data between tables); the name is part of the fingerprint.
    Code(&'static str, StepFn),
}

impl Step {
    /// What identifies the step (SHA-256, lower-case hex): the DDL of its tables, its SQL text, or its code name. The
    /// registry stores it per step, so a reordered, inserted or changed step is found (`Error::StepMismatch`).
    #[must_use]
    pub fn fingerprint(&self) -> String {
        let text = match self {
            Self::Create(tables) => tables.iter().fold(String::from("create"), |mut a, t| {
                a.push('\n');
                a.push_str(&t.ddl());
                t.indexes.iter().for_each(|i| {
                    a.push('\n');
                    a.push_str(i);
                });
                a
            }),
            Self::Sql(sql) => format!("sql\n{sql}"),
            Self::Code(name, _) => format!("code\n{name}"),
        };
        Sha256::digest(text.as_bytes())
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<Vec<_>>()
            .concat()
    }

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
            Self::Code(_, f) => f(c).await,
        }
    }
}
