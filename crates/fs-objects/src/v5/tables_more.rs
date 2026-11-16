//! The tables of fs-objects v5 that every program uses, part 2: log bookkeeping, meta, original and origin.
//!
//! Pattern: Value Object (const table specifications).

use super::spec::{
    Col,
    Logical::{Blob, Int, Iri, Local, Ref, Text, Utc},
    Table,
};
use super::tables::{ACTIVITY, ACTIVITY_CHANGE, OBJECTS, SETTING};

/// Ids whose log was purged (ADR 0029, as v4); no FK: the object may live on or be gone.
pub const PURGED_OBJECTS: Table = id_table("purged_objects");
/// Ids whose writes are not logged (as v4).
pub const UNLOGGED_IDS: Table = id_table("unlogged_ids");
const ID_COLS: &[Col] = &[Col::req("object_id", Ref)];
const ID_KEY: &[&str] = &["PRIMARY KEY (object_id)"];
const fn id_table(name: &'static str) -> Table {
    Table {
        name,
        cols: ID_COLS,
        constraints: ID_KEY,
        indexes: &[],
    }
}

/// Scalar store values: `store_id`, `activity_high` (the last activity id, so ids stay monotonic).
pub const META: Table = Table {
    name: "meta",
    cols: &[Col::req("key", Text), Col::req("value", Text)],
    constraints: &["PRIMARY KEY (key)"],
    indexes: &[],
};

/// Channel names a program may use for `object_origin.channel` (a domain lists its own, general names, in
/// `Domain::channels`; a channel is no calendar thing: `carddav` for people, `caldav` for calendar are the program's).
pub const COMMON_CHANNELS: &[&str] = &["file", "import", "subscription", "sync", "manual", "hub"];
const FK_OBJECT: &str = "FOREIGN KEY (object) REFERENCES objects(id) ON DELETE CASCADE";
const HEX: &str = "CHECK (length(sha256) = 64 AND sha256 NOT GLOB '*[^0-9a-f]*')";

/// The received file as it came (ADR 0030, immutable after insert); the one BLOB of the store (ADR 0086 item 4).
pub const OBJECT_ORIGINAL: Table = Table {
    name: "object_original",
    cols: &[
        Col::req("object", Ref),
        Col::req("media_type", Text),
        Col::req("bytes", Blob),
        Col::req("sha256", Text),
        Col::req("received", Utc),
        Col::opt("keep_until", Utc),
    ],
    constraints: &["PRIMARY KEY (object)", FK_OBJECT, HEX],
    indexes: &[],
};

/// Where an imported object came from. "Unchanged since import" is computed: `objects.rev = rev_at_import`.
pub const OBJECT_ORIGIN: Table = Table {
    name: "object_origin",
    cols: &[
        Col::req("object", Ref),
        Col::req("channel", Text),
        Col::req("source", Iri),
        Col::req("media_type", Text),
        Col::req("foreign_uid", Text),
        Col::opt("foreign_recurrence_id", Local),
        Col::opt("foreign_etag", Text),
        Col::opt("prod_id", Text),
        Col::req("imported", Utc),
        Col::req("importer", Text),
        Col::req("rev_at_import", Int),
        Col::opt("original_iri", Iri),
        Col::opt("original_sha256", Text),
        Col::opt("original_media_type", Text),
    ],
    constraints: &[
        "PRIMARY KEY (object)",
        FK_OBJECT,
        "CHECK ((original_iri IS NULL) = (original_sha256 IS NULL) AND (original_iri IS NULL) = (original_media_type IS NULL))",
    ],
    indexes: &[],
};

/// All tables of this module, in creation order (parents first).
pub const ALL: [Table; 9] = [
    SETTING,
    OBJECTS,
    ACTIVITY,
    ACTIVITY_CHANGE,
    PURGED_OBJECTS,
    UNLOGGED_IDS,
    META,
    OBJECT_ORIGINAL,
    OBJECT_ORIGIN,
];
