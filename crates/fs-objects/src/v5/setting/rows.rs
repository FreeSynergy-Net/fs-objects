//! Open, revision, person values. Split from `mod`; the index is in `rows_index`, the instance layer in `rows_instance`.
//!
//! Pattern: Repository (one transaction per writing call: value, `rev + 1`).

use fs_types::{BasicValue, Utc};

use super::prim::{bump, cols, prim_at, revision};
use super::{DOMAIN, PREFIX, PlaceRow, Prim, ScopeRow, SettingRows, Writer};
use crate::v5::Store;
use crate::v5::codec::{int, invalid, opt_text, text};
use crate::v5::conn::Conn;
use crate::v5::error::Result;
use crate::v5::registry_rows::version;
use crate::v5::spec::Table;

use super::tables::{STEPS, VALUE};

pub(super) fn scope_words(s: ScopeRow<'_>) -> (&'static str, &str) {
    match s {
        ScopeRow::General => ("general", ""),
        ScopeRow::Program(p) => ("program", p),
    }
}

pub(super) fn place_word(p: PlaceRow) -> &'static str {
    match p {
        PlaceRow::Profile => "profile",
        PlaceRow::Device => "device",
    }
}

/// The profile exists and is not a tombstone.
pub(super) async fn live(c: &Conn<'_>, profile: i64, field: &str) -> Result<()> {
    let rows = c
        .query(
            "SELECT deleted FROM settings_profile WHERE id = ?",
            vec![int(profile)],
        )
        .await?;
    match rows.first() {
        Some(r) if matches!(r.first(), Some(turso::Value::Null)) => Ok(()),
        _ => Err(invalid(field, "unknown")),
    }
}

impl<'s> SettingRows<'s> {
    /// Opens (creating if absent) and registers [`DOMAIN`]. A program uses `Store::inspect` FIRST when the file exists.
    ///
    /// # Errors
    /// The engine's, or `SecondDomain` / `Newer` / `StepMismatch` from the registry.
    pub async fn open(path: &str, now: Utc) -> Result<Store> {
        let store = Store::open(path, now.millis()).await?;
        store.register_domain(&DOMAIN, now.millis()).await?;
        Ok(store)
    }

    /// Borrows `store`.
    ///
    /// # Errors
    /// `Invalid` (`settings`, `not-registered`) when [`DOMAIN`] is not registered at its last step on `store`.
    pub async fn new(store: &'s Store) -> Result<Self> {
        if version(&store.read().await, PREFIX).await? != STEPS.len() {
            return Err(invalid(PREFIX, "not-registered"));
        }
        Ok(Self { store })
    }

    /// The revision (the `Stamp` of the file): rises with every write.
    ///
    /// # Errors
    /// The engine's.
    pub async fn rev(&self) -> Result<i64> {
        revision(&self.store.read().await).await
    }

    /// The values of `profile` in `scope` and `place`, by key.
    ///
    /// # Errors
    /// The engine's.
    pub async fn values(
        &self,
        profile: i64,
        scope: ScopeRow<'_>,
        place: PlaceRow,
    ) -> Result<Vec<(String, Prim)>> {
        let (s, program) = scope_words(scope);
        let rows = self
            .store
            .read()
            .await
            .query(
                "SELECT key, kind, v_int, v_text FROM settings_value \
                 WHERE profile = ? AND scope = ? AND program = ? AND place = ? ORDER BY key",
                vec![
                    int(profile),
                    text(s),
                    text(program),
                    text(place_word(place)),
                ],
            )
            .await?;
        let mut out = Vec::new();
        for r in &rows {
            let key = crate::v5::codec::get_text(r, 0)?;
            if let Some(p) = prim_at(r, 1)? {
                out.push((key, p));
            }
        }
        Ok(out)
    }

    /// `v = None` removes. Every call is one write (`rev + 1`).
    ///
    /// # Errors
    /// `Invalid` `settings_value.profile` `unknown` (unknown or deleted profile); `settings_value.key` `locked` when the key is
    /// locked in THIS file's instance tables (instance DB / single seat); the person service checks the instance service's
    /// locks itself.
    #[allow(clippy::too_many_arguments)]
    pub async fn put(
        &self,
        profile: i64,
        scope: ScopeRow<'_>,
        place: PlaceRow,
        key: &str,
        v: Option<&Prim>,
        w: Writer<'_>,
        at: Utc,
    ) -> Result<i64> {
        let (s, program) = scope_words(scope);
        let c = self.store.begin().await?;
        let r = async {
            live(&c, profile, "settings_value.profile").await?;
            let locked = c
                .query(
                    "SELECT 1 FROM settings_instance_lock WHERE key = ?",
                    vec![text(key)],
                )
                .await?;
            if !locked.is_empty() {
                return Err(invalid("settings_value.key", "locked"));
            }
            let pk = vec![int(profile), text(s), text(program), text(place_word(place)), text(key)];
            match v {
                None => {
                    c.exec(
                        "DELETE FROM settings_value WHERE profile = ? AND scope = ? AND program = ? AND place = ? AND key = ?",
                        pk,
                    )
                    .await?;
                }
                Some(p) => {
                    let [k, i, t] = cols(Some(p));
                    let mut row = pk;
                    row.extend([k, i, t, int(at.millis())]);
                    row.push(opt_text(w.by.map(BasicValue::canonical).as_deref()));
                    row.push(opt_text(w.via));
                    upsert_value(&c, &VALUE, row).await?;
                }
            }
            bump(&c).await
        }
        .await;
        c.finish(r).await
    }

    /// Removes every value of `profile` in `scope` (both places).
    ///
    /// # Errors
    /// `Invalid` `settings_value.profile` `unknown`; the engine's.
    pub async fn clear_profile(&self, profile: i64, scope: ScopeRow<'_>, _at: Utc) -> Result<i64> {
        let (s, program) = scope_words(scope);
        let c = self.store.begin().await?;
        let r = async {
            live(&c, profile, "settings_value.profile").await?;
            c.exec(
                "DELETE FROM settings_value WHERE profile = ? AND scope = ? AND program = ?",
                vec![int(profile), text(s), text(program)],
            )
            .await?;
            bump(&c).await
        }
        .await;
        c.finish(r).await
    }
}

async fn upsert_value(c: &Conn<'_>, t: &Table, row: Vec<turso::Value>) -> Result<()> {
    c.upsert(t, &["profile", "scope", "program", "place", "key"], row)
        .await
}
