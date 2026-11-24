//! The owner of a store, kept in table `meta` (key `store_owner`), and `birth`.
//!
//! Pattern: Adapter — one `meta` row (like `identity`). The owner is set once: by `OpenOptions::with_owner` on the
//! first open, or by `birth` (the one write whose owner is the written object itself). Every later open must
//! bring the same person or none.

use fs_types::BasicValue as _;
use turso::{Connection, Value};

use super::TursoStore;
use super::rows::text;
use super::sql::{collect, exec};
use super::store::Inner;
use super::tx::in_tx;
use crate::{By, Error, Outcome, PersonRef, Result, StoredObject, Write, art_word, person_from};
use fs_types::PersonArt;

const KEY: &str = "store_owner";

fn encode(p: &PersonRef) -> String {
    format!("{} {}", art_word(p.art), p.id.canonical())
}

fn decode(s: &str) -> Result<PersonRef> {
    s.split_once(' ')
        .and_then(|(art, id)| person_from(id, art))
        .ok_or_else(|| Error::Backend(format!("meta.{KEY} is not a person: {s}")))
}

async fn stored(c: &Connection) -> Result<Option<PersonRef>> {
    let rows = collect(
        c,
        "SELECT value FROM meta WHERE key = ?",
        vec![Value::Text(KEY.into())],
    )
    .await?;
    rows.first().map(|r| decode(&text(&r[0])?)).transpose()
}

async fn store(c: &Connection, p: &PersonRef) -> Result<()> {
    exec(
        c,
        "INSERT INTO meta (key, value) VALUES (?, ?)",
        vec![Value::Text(KEY.into()), Value::Text(encode(p))],
    )
    .await?;
    Ok(())
}

/// The owner of an opened database: the stored one, which `wanted` must equal if given; a first `wanted` is stored
/// (not on a read-only handle). `Invalid("store owner mismatch")` if the two differ.
pub(super) async fn settle_owner(
    c: &Connection,
    wanted: Option<&PersonRef>,
    read_only: bool,
) -> Result<Option<PersonRef>> {
    let have = stored(c).await?;
    match (have, wanted) {
        (Some(h), Some(w)) if h != *w => Err(Error::Invalid(format!(
            "store owner mismatch: the store belongs to {}, opened for {}",
            h.id.canonical(),
            w.id.canonical()
        ))),
        (Some(h), _) => Ok(Some(h)),
        (None, Some(w)) if read_only => Ok(Some(w.clone())),
        (None, Some(w)) => {
            // Two first opens at once: the later one re-reads inside the writer lock.
            let w = w.clone();
            in_tx(c, async {
                match stored(c).await? {
                    Some(h) if h != w => Err(Error::Invalid("store owner mismatch".into())),
                    Some(_) => Ok(()),
                    None => store(c, &w).await,
                }
            })
            .await?;
            Ok(Some(w))
        }
        (None, None) => Ok(None),
    }
}

/// `urn:uuid:<id>` of an object: how a person's `id` names the same uuid.
pub(super) fn own_urn(o: &StoredObject) -> String {
    format!("urn:uuid:{}", o.envelope.id)
}

impl TursoStore {
    /// The rules of `birth` that need no database.
    fn check_birth(by: &By, writes: &[Write]) -> Result<()> {
        let [Write::Put { object, .. }] = writes else {
            return Err(Error::Invalid(
                "birth writes exactly one Put (the owner itself)".into(),
            ));
        };
        if by.owner.art != PersonArt::Person {
            return Err(Error::Invalid(
                "birth: only a person can own a store".into(),
            ));
        }
        if by.owner.id.canonical() != own_urn(object) {
            return Err(Error::Invalid(
                "birth: the owner must be the written object itself (its own uuid)".into(),
            ));
        }
        if object.owner.as_ref().is_some_and(|o| *o != by.owner) {
            return Err(Error::Invalid("birth: the object has another owner".into()));
        }
        Ok(())
    }

    /// See [`ObjectRepository::birth`](crate::ObjectRepository::birth).
    pub(super) async fn birth_impl(&self, by: &By, object: StoredObject) -> Result<Outcome> {
        let writes = vec![Write::Put {
            object,
            expect: None,
        }];
        Self::check_birth(by, &writes)?;
        self.check_writes(by, &writes)?;
        let mut inner = self.inner.lock().await;
        let inner = &mut *inner;
        if inner.owner.is_some() {
            return Err(Error::Invalid("store already owned".into()));
        }
        let conn = inner.conn.clone();
        let res = in_tx(&conn, Box::pin(self.birth_in_tx(inner, by, writes))).await;
        let out = self.finish_write(inner, res).await?;
        if matches!(out, Outcome::Done(_)) {
            inner.owner = Some(by.owner.clone());
        }
        Ok(out)
    }

    async fn birth_in_tx(&self, inner: &mut Inner, by: &By, writes: Vec<Write>) -> Result<Outcome> {
        // Another handle may have given the file an owner since this one opened it.
        if stored(&inner.conn).await?.is_some() {
            return Err(Error::Invalid("store already owned".into()));
        }
        store(&inner.conn, &by.owner).await?;
        self.write_unit(inner, by, writes, true).await
    }
}

/// The owner a caller-built object may carry when written by `put`, `put_and_log` or `put_unlogged`: none, or exactly
/// the one that stamping would give it (the existing owner, else the store owner). Anything else, and self-ownership
/// of a new object (only `birth` makes that), is `Invalid`; nothing is written. The stamp itself is set by
/// `apply_with` / `put_unlogged`, never taken from the caller.
pub(super) fn check_caller_owner(
    inner: &Inner,
    object: &StoredObject,
    before: Option<&StoredObject>,
) -> Result<()> {
    let id = object.envelope.id;
    let expected = before
        .and_then(|b| b.owner.clone())
        .or_else(|| inner.owner.clone());
    let selfish = |o: &Option<PersonRef>| {
        o.as_ref()
            .is_some_and(|o| o.id.canonical() == own_urn(object))
    };
    if before.is_none() && (selfish(&expected) || selfish(&object.owner)) {
        return Err(Error::Invalid(format!(
            "{id}: self-ownership only through birth"
        )));
    }
    if let Some(o) = &object.owner
        && Some(o) != expected.as_ref()
    {
        return Err(Error::Invalid(format!(
            "{id}: the owner is stamped by the store, another owner is a separate command"
        )));
    }
    Ok(())
}
