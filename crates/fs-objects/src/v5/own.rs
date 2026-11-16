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
use super::tables_more::ALL as OBJECT_TABLES;

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
const OWN_STEPS: [Step; 1] = [Step::Create(&OWN_TABLES)];
pub(super) const OWN_DOMAIN: Domain = Domain {
    prefix: super::registry::OWN,
    about: "tables of the store itself",
    tables: &OWN_TABLES,
    channels: &[],
    steps: &OWN_STEPS,
};
