//! The tables of the store itself (registry, step fingerprints, shared object tables) as the reserved domain `fs_`.
//!
//! Pattern: Value Object (const specifications and the own domain built from them).

use super::migrate::Step;
use super::registry::Domain;
use super::spec::{
    Col,
    Logical::{Bool, Int, Text, Utc},
    Table,
};
use super::tables_more::{ALL as OBJECT_TABLES, OBJECT_HOLD};

/// The registry's own tables.
pub const FS_DOMAIN: Table = Table {
    name: "fs_domain",
    cols: &[
        Col::req("prefix", Text),
        Col::req("version", Int),
        Col::req("description", Text),
        Col::req("applied", Utc),
    ],
    constraints: &["PRIMARY KEY (prefix)"],
    indexes: &[],
};
/// One row per column of every registered table.
pub const FS_COLUMN: Table = Table {
    name: "fs_column",
    cols: &[
        Col::req("domain", Text),
        Col::req("tbl", Text),
        Col::req("col", Text),
        Col::req("logical", Text),
        Col::req("nullable", Bool),
        Col::req("derived", Bool),
    ],
    constraints: &["PRIMARY KEY (domain, tbl, col)"],
    indexes: &[],
};

/// The fingerprint of every applied step (reordered, inserted or changed steps are found by it).
pub const FS_STEP: Table = Table {
    name: "fs_step",
    cols: &[
        Col::req("domain", Text),
        Col::req("seq", Int),
        Col::req("fingerprint", Text),
    ],
    constraints: &[
        "PRIMARY KEY (domain, seq)",
        "CHECK (length(fingerprint) = 64 AND fingerprint NOT GLOB '*[^0-9a-f]*')",
    ],
    indexes: &[],
};

const N_OWN: usize = OBJECT_TABLES.len() + 3;
const OWN_TABLES: [Table; N_OWN] = {
    let mut t = [FS_DOMAIN; N_OWN];
    let mut i = 0;
    while i < OBJECT_TABLES.len() {
        t[i] = OBJECT_TABLES[i];
        i += 1;
    }
    t[i] = FS_DOMAIN;
    t[i + 1] = FS_COLUMN;
    t[i + 2] = FS_STEP;
    t
};
/// The tables after the last step: those of step 1 and the hold table of step 3 (step 1 stays as it was in v0.0.2,
/// because its fingerprint is stored in every store).
const OWN_ALL: [Table; N_OWN + 1] = {
    let mut t = [FS_DOMAIN; N_OWN + 1];
    let mut i = 0;
    while i < N_OWN {
        t[i] = OWN_TABLES[i];
        i += 1;
    }
    t[N_OWN] = OBJECT_HOLD;
    t
};
const OWN_STEPS: [Step; 3] = [
    Step::Create(&OWN_TABLES),
    // ADR 0030: the original is never rewritten (only the final delete of the object removes it, by cascade).
    Step::Sql(
        "CREATE TRIGGER object_original_immutable BEFORE UPDATE ON object_original \
         BEGIN SELECT RAISE(ABORT, 'the original is immutable'); END",
    ),
    // v0.0.3, appended: a hold keeps a trashed object past its period (ADR 0073 addendum 2026-11-16).
    Step::Create(&[OBJECT_HOLD]),
];
pub(super) const OWN_DOMAIN: Domain = Domain::new(
    super::registry::OWN,
    "tables of the store itself",
    &OWN_ALL,
    &OWN_STEPS,
);
