//! The tables of fs-objects v5 that every program uses (draft section 3), part 1: setting, envelope, activity, changes.
//!
//! Pattern: Value Object (const table specifications).

use super::spec::{
    Col,
    Logical::{Bool, Enum, Int, Iri, Real, Ref, Text, Utc},
    Table,
};

/// The kinds of a person reference (AS2 names).
pub const ART: &[&str] = &["profile", "person", "group", "organization"];
/// The kinds of an activity.
pub const KINDS: &[&str] = &[
    "Create",
    "Update",
    "Delete",
    "Undo",
    "Invite",
    "Accept",
    "TentativeAccept",
    "Reject",
];
/// The kinds of a logged value.
pub const VALUE_KINDS: &[&str] = &[
    "int", "real", "text", "bool", "utc", "local", "date", "zone", "object", "iri", "person",
    "attendee",
];
/// The kinds of an attendee in a logged value.
pub const WHO_KINDS: &[&str] = &["actor", "resource", "location"];

/// Scalar program settings (`trash_days`).
pub const SETTING: Table = Table {
    name: "setting",
    cols: &[Col::req("key", Text), Col::req("int_value", Int)],
    constraints: &[
        "PRIMARY KEY (key)",
        "CHECK (key <> 'trash_days' OR int_value > 0)",
    ],
    indexes: &[],
};

/// The envelope of every object. `rev` is the counter for compare-and-set; trash is `deleted` + `purge_after`.
pub const OBJECTS: Table = Table {
    name: "objects",
    cols: &[
        Col::req("id", Ref),
        Col::opt("iri", Iri),
        Col::req("type", Text),
        Col::req("name", Text),
        Col::req("name_sort", Text),
        Col::opt("attributed_to_id", Iri),
        Col::opt("attributed_to_art", Enum(ART)),
        Col::opt("attributed_to_abzug", Ref),
        Col::req("published", Utc),
        Col::req("updated", Utc),
        Col::opt("context", Ref),
        Col::opt("in_reply_to", Ref),
        Col::opt("describes", Ref),
        Col::req("rev", Int),
        Col::opt("deleted", Utc),
        Col::opt("purge_after", Utc),
    ],
    constraints: &[
        "PRIMARY KEY (id)",
        "CHECK ((deleted IS NULL) = (purge_after IS NULL))",
        "CHECK ((attributed_to_id IS NULL) = (attributed_to_art IS NULL))",
        "CHECK (rev >= 1)",
    ],
    indexes: &[],
};

/// The log: one row per action; the changed fields are `activity_change` rows.
pub const ACTIVITY: Table = Table {
    name: "activity",
    cols: &[
        Col::req("id", Ref),
        Col::req("kind", Enum(KINDS)),
        Col::req("object", Ref),
        Col::req("actor_id", Iri),
        Col::req("actor_art", Enum(ART)),
        Col::opt("actor_abzug", Ref),
        Col::req("at", Utc),
        Col::opt("target", Ref),
        Col::req("synced", Bool),
        Col::req("redacted", Bool),
        Col::req("explained", Bool),
    ],
    constraints: &[
        "PRIMARY KEY (id)",
        "FOREIGN KEY (object) REFERENCES objects(id) ON DELETE CASCADE",
    ],
    indexes: &["CREATE INDEX IF NOT EXISTS activity_object ON activity (object, id)"],
};

macro_rules! side {
    ($p:literal) => {
        [
            Col::opt(concat!($p, "_int"), Int),
            Col::opt(concat!($p, "_real"), Real),
            Col::opt(concat!($p, "_text"), Text),
            Col::opt(concat!($p, "_person_id"), Iri),
            Col::opt(concat!($p, "_person_art"), Enum(ART)),
            Col::opt(concat!($p, "_person_abzug"), Ref),
            Col::opt(concat!($p, "_who_kind"), Enum(WHO_KINDS)),
            Col::opt(concat!($p, "_who_id"), Iri),
            Col::opt(concat!($p, "_who_art"), Enum(ART)),
            Col::opt(concat!($p, "_who_abzug"), Ref),
        ]
    };
}

const OLD: [Col; 10] = side!("old");
const NEW: [Col; 10] = side!("new");
const HEAD: [Col; 4] = [
    Col::req("activity", Ref),
    Col::req("seq", Int),
    Col::req("field", Text),
    Col::opt("row_key", Text),
];
const KIND: Col = Col::req("value_kind", Enum(VALUE_KINDS));

const fn join() -> [Col; 25] {
    let mut out = [KIND; 25];
    let mut i = 0;
    while i < 4 {
        out[i] = HEAD[i];
        i += 1;
    }
    out[4] = KIND;
    let mut j = 0;
    while j < 10 {
        out[5 + j] = OLD[j];
        out[15 + j] = NEW[j];
        j += 1;
    }
    out
}
const CHANGE_COLS: [Col; 25] = join();

macro_rules! kind_check {
    ($p:literal) => {
        concat!(
            "CHECK ((",
            $p,
            "_int IS NULL OR value_kind IN ('int','bool','utc','local','date'))",
            " AND (",
            $p,
            "_real IS NULL OR value_kind = 'real')",
            " AND (",
            $p,
            "_text IS NULL OR value_kind IN ('text','zone','object','iri'))",
            " AND ((",
            $p,
            "_person_id IS NULL AND ",
            $p,
            "_person_art IS NULL AND ",
            $p,
            "_person_abzug IS NULL) OR value_kind = 'person')",
            " AND ((",
            $p,
            "_who_kind IS NULL AND ",
            $p,
            "_who_id IS NULL AND ",
            $p,
            "_who_art IS NULL AND ",
            $p,
            "_who_abzug IS NULL) OR value_kind = 'attendee')",
            " AND (",
            $p,
            "_who_art IS NULL OR ",
            $p,
            "_who_kind = 'actor'))"
        )
    };
}

/// One changed field: old and new value, typed (no JSON, no blob; ADR 0086 item 2). No FK on the value columns.
pub const ACTIVITY_CHANGE: Table = Table {
    name: "activity_change",
    cols: &CHANGE_COLS,
    constraints: &[
        "PRIMARY KEY (activity, seq)",
        "FOREIGN KEY (activity) REFERENCES activity(id) ON DELETE CASCADE",
        kind_check!("old"),
        kind_check!("new"),
    ],
    indexes: &[],
};
