//! The table family `settings_*` (ADR 0100, PLAN section 5.2): the values of a person, the profile index with tombstones, the
//! admin's instance defaults, their locks and the append-only instance log. One source for the DDL and for `fs_column`.
//!
//! Pattern: Value Object (const table specs), as the typed tables of the programs. Schema version = number of steps = 5.
//! Written by Claude (Anthropic) as a probe; see CHANGELOG.

use crate::v5::migrate::Step;
use crate::v5::registry::Domain;
use crate::v5::spec::{
    Col,
    Logical::{Enum, FreeText, Int, Iri, Ref, Text, Utc},
    Table,
};

/// The prefix. `registry.rs:21` reserves `setting` (singular, the store's own `setting` table): `settings` is free (exact compare).
pub const PREFIX: &str = "settings";
/// Closed technical word lists (ADR 0086 addendum decision 7: `CHECK IN` only for closed technical states).
pub const KIND: &[&str] = &["bool", "int", "text"];
/// `settings_value.scope`.
pub const SCOPE: &[&str] = &["general", "program"];
/// `settings_value.place`.
pub const PLACE: &[&str] = &["profile", "device"];
/// `settings_instance_log.action`.
pub const ACTION: &[&str] = &["set", "clear", "lock", "unlock"];

/// One counter; every write transaction does `rev = rev + 1` -> `Stamp`.
pub const REV: Table = Table {
    name: "settings_rev",
    cols: &[Col::req("slot", Int), Col::req("rev", Int)],
    constraints: &["PRIMARY KEY (slot)", "CHECK (slot = 0)", "CHECK (rev >= 0)"],
    indexes: &[],
};

/// Base (id 0, no name) + named profiles; a deleted profile stays as a tombstone (`deleted`, `name_key` NULL) so its id is never
/// issued again (`top` = MAX(id)). `name` is what the person wrote (FreeText); `name_key` = `ProfileName` comparison form.
pub const PROFILE: Table = Table {
    name: "settings_profile",
    cols: &[
        Col::req("id", Int),
        Col::opt("name", FreeText),
        Col::opt("name_key", Text),
        Col::req("created", Utc),
        Col::opt("deleted", Utc),
    ],
    constraints: &[
        "PRIMARY KEY (id)",
        "UNIQUE (name_key)",
        "CHECK (id >= 0)",
        "CHECK (id <> 0 OR (name IS NULL AND name_key IS NULL AND deleted IS NULL))",
        "CHECK (id = 0 OR name IS NOT NULL)",
        "CHECK ((deleted IS NULL) = (id = 0 OR name_key IS NOT NULL))",
    ],
    indexes: &[],
};

/// The active profile of this device (one person DB per device; `device` = `meta.store_id`, ready for the later sync).
pub const ACTIVE: Table = Table {
    name: "settings_active",
    cols: &[Col::req("device", Ref), Col::req("profile", Int)],
    constraints: &[
        "PRIMARY KEY (device)",
        "FOREIGN KEY (profile) REFERENCES settings_profile(id)",
    ],
    indexes: &[],
};

/// One value of the person. Layers: profile 0 + general = Person, profile 0 + program = Program, profile > 0 = Named*.
/// `changed_by` = actor IRI when not the person herself (an AI's own actor, later); `changed_via` = the program that wrote it.
pub const VALUE: Table = Table {
    name: "settings_value",
    cols: &[
        Col::req("profile", Int),
        Col::req("scope", Enum(SCOPE)),
        Col::req("program", Text),
        Col::req("place", Enum(PLACE)),
        Col::req("key", Text),
        Col::req("kind", Enum(KIND)),
        Col::opt("v_int", Int),
        Col::opt("v_text", FreeText),
        Col::req("changed", Utc),
        Col::opt("changed_by", Iri),
        Col::opt("changed_via", Text),
    ],
    constraints: &[
        "PRIMARY KEY (profile, scope, program, place, key)",
        "FOREIGN KEY (profile) REFERENCES settings_profile(id)",
        "CHECK ((scope = 'general') = (program = ''))",
        "CHECK ((kind = 'bool' AND v_int IN (0,1) AND v_text IS NULL) OR (kind = 'int' AND v_int IS NOT NULL AND v_text IS NULL) \
         OR (kind = 'text' AND v_text IS NOT NULL AND v_int IS NULL))",
    ],
    indexes: &[
        "CREATE INDEX IF NOT EXISTS settings_value_scope ON settings_value (scope, program, profile)",
    ],
};

/// The admin's defaults (instance DB only; empty in a person DB).
pub const INSTANCE_VALUE: Table = Table {
    name: "settings_instance_value",
    cols: &[
        Col::req("key", Text),
        Col::req("kind", Enum(KIND)),
        Col::opt("v_int", Int),
        Col::opt("v_text", FreeText),
        Col::req("set_at", Utc),
        Col::req("set_by", Iri),
    ],
    constraints: &[
        "PRIMARY KEY (key)",
        "CHECK ((kind = 'bool' AND v_int IN (0,1) AND v_text IS NULL) OR (kind = 'int' AND v_int IS NOT NULL AND v_text IS NULL) \
         OR (kind = 'text' AND v_text IS NOT NULL AND v_int IS NULL))",
    ],
    indexes: &[],
};

/// A lock never exists without its value (FK, as `lock.rs:168-170` today): clearing a locked value fails -> `locked`.
pub const INSTANCE_LOCK: Table = Table {
    name: "settings_instance_lock",
    cols: &[
        Col::req("key", Text),
        Col::req("reason", FreeText),
        Col::req("locked_by", Iri),
        Col::req("locked_at", Utc),
    ],
    constraints: &[
        "PRIMARY KEY (key)",
        "FOREIGN KEY (key) REFERENCES settings_instance_value(key)",
        "CHECK (length(trim(reason)) > 0)", // turso `trim` in CHECK: K-case KA9; the code checks anyway
    ],
    indexes: &[],
};

/// Append-only protocol of the instance layer (triggers in steps 2 and 3, pattern `own.rs:90`).
pub const INSTANCE_LOG: Table = Table {
    name: "settings_instance_log",
    cols: &[
        Col::req("id", Ref), // UUIDv7, `new_id(now)`
        Col::req("at", Utc),
        Col::req("by", Iri),
        Col::opt("os_user", Text),
        Col::req("action", Enum(ACTION)),
        Col::req("key", Text),
        Col::opt("reason", FreeText),
        Col::opt("old_kind", Enum(KIND)),
        Col::opt("old_int", Int),
        Col::opt("old_text", FreeText),
        Col::opt("new_kind", Enum(KIND)),
        Col::opt("new_int", Int),
        Col::opt("new_text", FreeText),
    ],
    constraints: &[
        "PRIMARY KEY (id)",
        "CHECK (action NOT IN ('lock','unlock') OR (reason IS NOT NULL AND length(trim(reason)) > 0))",
    ],
    indexes: &["CREATE INDEX IF NOT EXISTS settings_instance_log_at ON settings_instance_log (at)"],
};

/// The seven tables after the last step.
pub const ALL: [Table; 7] = [
    REV,
    PROFILE,
    ACTIVE,
    VALUE,
    INSTANCE_VALUE,
    INSTANCE_LOCK,
    INSTANCE_LOG,
];

/// Schema version = number of steps (`registry.rs:4`) = 5. Later columns: append `Step::Sql("ALTER TABLE ... ADD COLUMN ...")` and
/// update the `Table` above (`fs_column` = shape after the last step). Never RENAME (ADR 0086 addendum, turso bug).
pub const STEPS: [Step; 5] = [
    Step::Create(&ALL),
    Step::Sql(
        "CREATE TRIGGER settings_instance_log_no_update BEFORE UPDATE ON settings_instance_log \
         BEGIN SELECT RAISE(ABORT, 'the instance log is append-only'); END",
    ),
    Step::Sql(
        "CREATE TRIGGER settings_instance_log_no_delete BEFORE DELETE ON settings_instance_log \
         BEGIN SELECT RAISE(ABORT, 'the instance log is append-only'); END",
    ),
    Step::Sql("INSERT INTO settings_rev (slot, rev) VALUES (0, 0)"),
    Step::Sql(
        "INSERT INTO settings_profile (id, name, name_key, created, deleted) VALUES (0, NULL, NULL, 0, NULL)",
    ),
];

/// The domain `settings` (prefix, tables, five steps).
pub const DOMAIN: Domain = Domain::new(
    PREFIX,
    "FreeSynergy settings (ADR 0100): values, profiles, instance defaults, locks, log",
    &ALL,
    &STEPS,
);
