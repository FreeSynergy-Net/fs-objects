//! The words of an activity kind as a word table (ADR 0086 addendum 2026-11-16) and the log table that points at it.
//!
//! Pattern: Value Object (const table specifications) plus Command list (the steps that fill and rebuild them).
//!
//! Steps 1 to 3 of the store's domain stay as they were: the `CHECK` list [`super::tables::KINDS`] is part of the
//! fingerprint of step 1 and is never edited. From step 4 on, `activity.kind` is a foreign key into `fs_word`,
//! so a new kind is one more `INSERT` step and never a table rebuild.

use super::conn::Conn;
use super::migrate::StepFuture;
use super::spec::{
    Col,
    Logical::{Bool, Int, Iri, Ref, Text, Utc},
    Table,
};
use super::tables::{ACTIVITY, ACTIVITY_CHANGE, KINDS};

/// The word set of the kinds of an activity (`fs_word.kind`, and `activity.kind_set`).
pub const SET: &str = "activity_kind";

/// Every kind an activity can have (AS2 names): the eight of step 1, then `Add`, `Remove`, `Join`, `Leave`, `Move`.
pub const ACTIVITY_KINDS: [&str; 13] = [
    KINDS[0], KINDS[1], KINDS[2], KINDS[3], KINDS[4], KINDS[5], KINDS[6], KINDS[7], "Add",
    "Remove", "Join", "Leave", "Move",
];

/// The word table: `(kind, code)` is the key; `builtin = 1` words are set by an `INSERT` step and cannot be deleted.
pub const ACTIVITY_WORD: Table = Table {
    name: "fs_word",
    cols: &[
        Col::req("kind", Text),
        Col::req("code", Text),
        Col::req("builtin", Bool),
        Col::opt("label", Text),
        Col::opt("sort", Int),
        Col::req("active", Bool),
    ],
    constraints: &["PRIMARY KEY (kind, code)"],
    indexes: &[],
};

/// The log after step 7: as `ACTIVITY`, but `kind` has no `CHECK` list; `(kind_set, kind)` is a key into the word table.
pub const ACTIVITY_V2: Table = Table {
    name: "activity",
    cols: &[
        Col::req("id", Ref),
        Col::req("kind", Text),
        Col::req("object", Ref),
        Col::req("actor_id", Iri),
        Col::req("actor_art", super::spec::Logical::Enum(super::tables::ART)),
        Col::opt("actor_abzug", Ref),
        Col::req("at", Utc),
        Col::opt("target", Ref),
        Col::req("synced", Bool),
        Col::req("redacted", Bool),
        Col::req("explained", Bool),
        Col::req("kind_set", Text),
    ],
    constraints: &[
        "PRIMARY KEY (id)",
        "FOREIGN KEY (object) REFERENCES objects(id) ON DELETE CASCADE",
        "CHECK (kind_set = 'activity_kind')",
        "FOREIGN KEY (kind_set, kind) REFERENCES fs_word(kind, code)",
    ],
    indexes: &["CREATE INDEX IF NOT EXISTS activity_object ON activity (object, id)"],
};

/// Step 5: the eight kinds of step 1 as built-in words (`ON CONFLICT` makes a user word of the same name a built-in).
pub const WORDS_OLD: &str = "INSERT INTO fs_word (kind, code, builtin, active) VALUES \
    ('activity_kind','Create',1,1),('activity_kind','Update',1,1),('activity_kind','Delete',1,1),\
    ('activity_kind','Undo',1,1),('activity_kind','Invite',1,1),('activity_kind','Accept',1,1),\
    ('activity_kind','TentativeAccept',1,1),('activity_kind','Reject',1,1) \
    ON CONFLICT (kind, code) DO UPDATE SET builtin = 1";

/// Step 6: the five AS2 kinds `Add`, `Remove`, `Join`, `Leave`, `Move` (v0.0.5).
pub const WORDS_AS2: &str = "INSERT INTO fs_word (kind, code, builtin, active) VALUES \
    ('activity_kind','Add',1,1),('activity_kind','Remove',1,1),('activity_kind','Join',1,1),\
    ('activity_kind','Leave',1,1),('activity_kind','Move',1,1) \
    ON CONFLICT (kind, code) DO UPDATE SET builtin = 1";

/// A staging table with the columns of `t` and no constraints (named `fs_tmp_...`, never a prefix of a real table).
fn staging(name: &str, t: &Table) -> String {
    let cols: Vec<String> = t
        .cols
        .iter()
        .map(|k| format!("{} {}", k.name, k.logical.sql()))
        .collect();
    format!("CREATE TABLE {name} ({}) STRICT", cols.join(", "))
}

/// Step 7: rebuild `activity` (and `activity_change`, which hangs on it) without the `CHECK` list, keeping every row.
///
/// No `ALTER TABLE ... RENAME` (turso 0.8.3 renames the autoindex of other tables, standard SQLite then refuses the
/// file): the rows go to staging tables, the old tables are dropped (children first), the new ones are created under
/// their own names and filled from staging.
pub fn rebuild_activity<'a>(c: &'a Conn<'_>) -> StepFuture<'a> {
    Box::pin(async move {
        let cols: Vec<&str> = ACTIVITY.cols.iter().map(|k| k.name).collect();
        let list = cols.join(",");
        c.run(&staging("fs_tmp_activity", &ACTIVITY)).await?;
        c.run(&staging("fs_tmp_change", &ACTIVITY_CHANGE)).await?;
        c.run(&format!(
            "INSERT INTO fs_tmp_activity ({list}) SELECT {list} FROM activity"
        ))
        .await?;
        c.run("INSERT INTO fs_tmp_change SELECT * FROM activity_change")
            .await?;
        c.run("DROP TABLE activity_change").await?;
        c.run("DROP TABLE activity").await?;
        c.run(&ACTIVITY_V2.ddl()).await?;
        for ix in ACTIVITY_V2.indexes {
            c.run(ix).await?;
        }
        c.run(&format!(
            "INSERT INTO activity ({list},kind_set) SELECT {list},'{SET}' FROM fs_tmp_activity"
        ))
        .await?;
        c.run(&ACTIVITY_CHANGE.ddl()).await?;
        c.run("INSERT INTO activity_change SELECT * FROM fs_tmp_change")
            .await?;
        c.run("DROP TABLE fs_tmp_change").await?;
        c.run("DROP TABLE fs_tmp_activity").await
    })
}
