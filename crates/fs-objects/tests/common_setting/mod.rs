//! Helpers for the `settings_*` tests (Probe A): files under the test target (never `/tmp`), fixed actors and times.
#![allow(dead_code)]

use std::path::PathBuf;

use fs_objects::v5::setting::tables::{
    ACTIVE, INSTANCE_LOCK, INSTANCE_LOG, INSTANCE_VALUE, PROFILE, REV, STEPS, VALUE,
};
use fs_objects::v5::setting::{Actor, SettingRows};
use fs_objects::v5::spec::Logical::{Enum, FreeText, Int, Iri, Text, Utc as UtcCol};
use fs_objects::v5::{Col, Domain, Step, Store, Table};
use fs_types::{BasicValue, Reference, Utc};
use tempfile::TempDir;

/// An instant `ms` after the epoch.
pub fn at(ms: i64) -> Utc {
    Utc::from_millis(ms)
}

/// The admin actor.
pub fn admin() -> Reference {
    Reference::parse("https://example.org/actor/admin").unwrap()
}

/// An actor with an OS user.
pub fn actor(by: &Reference) -> Actor<'_> {
    Actor {
        by,
        os_user: Some("root"),
    }
}

/// A folder under the test target.
pub fn dir() -> TempDir {
    tempfile::Builder::new()
        .prefix("settings-")
        .tempdir_in(env!("CARGO_TARGET_TMPDIR"))
        .unwrap()
}

/// The path of a store file in `d`.
pub fn file(d: &TempDir) -> PathBuf {
    d.path().join("settings.db")
}

/// An open store with the domain registered, in memory.
pub async fn mem() -> Store {
    SettingRows::open(":memory:", at(1)).await.unwrap()
}

/// Does the result fail as `Invalid` with this field and reason?
pub fn invalid_is<T: std::fmt::Debug>(
    r: &Result<T, fs_objects::v5::Error>,
    field: &str,
    reason: &str,
) -> bool {
    matches!(r, Err(fs_objects::v5::Error::Invalid(p)) if p.iter().any(|x| x.field == field && x.reason == reason))
}

/// The five steps plus one more: a column appended to `settings_value`.
pub const NOTE_STEP: Step = Step::Sql("ALTER TABLE settings_value ADD COLUMN note TEXT");
pub const STEPS6: [Step; 6] = [STEPS[0], STEPS[1], STEPS[2], STEPS[3], STEPS[4], NOTE_STEP];
const VALUE_COLS6: [Col; 12] = [
    Col::req("profile", Int),
    Col::req("scope", Enum(fs_objects::v5::setting::tables::SCOPE)),
    Col::req("program", Text),
    Col::req("place", Enum(fs_objects::v5::setting::tables::PLACE)),
    Col::req("key", Text),
    Col::req("kind", Enum(fs_objects::v5::setting::tables::KIND)),
    Col::opt("v_int", Int),
    Col::opt("v_text", FreeText),
    Col::req("changed", UtcCol),
    Col::opt("changed_by", Iri),
    Col::opt("changed_via", Text),
    Col::opt("note", FreeText),
];
const VALUE6: Table = Table {
    cols: &VALUE_COLS6,
    ..VALUE
};
const ALL6: [Table; 7] = [
    REV,
    PROFILE,
    ACTIVE,
    VALUE6,
    INSTANCE_VALUE,
    INSTANCE_LOCK,
    INSTANCE_LOG,
];
pub const DOMAIN6: Domain = Domain::new("settings", "six steps", &ALL6, &STEPS6);
