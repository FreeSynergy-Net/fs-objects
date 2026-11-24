//! `write_by`: several writes as one undo unit, with origin, idempotent step and compare-and-set.
//!
//! Pattern: Unit of Work — all writes of one call are planned first (nothing is written when one of them conflicts),
//! then applied in ONE transaction; the first activity is the head of the unit, the others point to it.

use fs_types::BasicValue as _;
use turso::{Connection, Value};

use super::TursoStore;
use super::activities::last_activity_id;
use super::codec::decode_activity;
use super::guard::refuse_unlogged;
use super::objects::load_object;
use super::owner::own_urn;
use super::repository::plan_put;
use super::schema::ACTIVITY_COLS;
use super::sql::collect;
use super::store::{Change, Inner, Meta};
use super::tx::in_tx;
use super::validate::validate;
use super::write_clash::check_expect;
use crate::{Activity, ActivityKind, By, Error, Outcome, Recorded, Result, Write};

/// The head of the unit that holds `(context, step)`, with all its activities, if it is in the log.
pub(super) async fn unit_of_step(
    c: &Connection,
    context: &str,
    step: &str,
) -> Result<Option<Recorded>> {
    let rows = collect(
        c,
        &format!("SELECT {ACTIVITY_COLS} FROM activities WHERE context = ? AND step = ? LIMIT 1"),
        vec![Value::Text(context.into()), Value::Text(step.into())],
    )
    .await?;
    let Some(r) = rows.first() else {
        return Ok(None);
    };
    let head = decode_activity(r)?;
    let mut all = vec![head.clone()];
    let rest = collect(
        c,
        &format!("SELECT {ACTIVITY_COLS} FROM activities WHERE unit = ? ORDER BY id"),
        vec![Value::Text(head.id.to_string())],
    )
    .await?;
    for r in &rest {
        all.push(decode_activity(r)?);
    }
    Ok(Some(Recorded {
        head,
        all,
        replayed: true,
    }))
}

/// What the planning of one write came to: a change to apply, or a result that ends the call.
#[allow(clippy::large_enum_variant)] // internal, one value at a time
enum Planned {
    Change(Change),
    Stop(Box<Outcome>),
}

/// Plan one write against the stored state (nothing is written).
async fn plan_write(c: &Connection, by: &By, w: Write, birth: bool) -> Result<Planned> {
    match w {
        Write::Put { object, expect } => {
            let id = object.envelope.id;
            refuse_unlogged(c, id).await?;
            let before = load_object(c, id).await?;
            if let Some(o) = check_expect(c, id, before.as_ref(), expect).await? {
                return Ok(Planned::Stop(Box::new(o)));
            }
            if !birth && before.is_none() && by.owner.id.canonical() == own_urn(&object) {
                return Err(Error::Invalid(format!(
                    "{id}: self-ownership only at birth"
                )));
            }
            let current_owner = before.as_ref().and_then(|b| b.owner.clone());
            let expected_owner = current_owner.unwrap_or_else(|| by.owner.clone());
            if object.owner.as_ref().is_some_and(|o| *o != expected_owner) {
                return Err(Error::Invalid(format!(
                    "{id}: another owner is a separate command (change of owner)"
                )));
            }
            let (kind, after) = plan_put(object, before.as_ref())?;
            Ok(Planned::Change(Change {
                kind,
                object: id,
                target: None,
                before,
                after: Some(after),
            }))
        }
        Write::Delete { id, expect } => {
            refuse_unlogged(c, id).await?;
            let before = load_object(c, id).await?;
            if let Some(o) = check_expect(c, id, before.as_ref(), expect).await? {
                return Ok(Planned::Stop(Box::new(o)));
            }
            let before = before.ok_or(Error::NotFound(id))?;
            Ok(Planned::Change(Change {
                kind: ActivityKind::Delete,
                object: id,
                target: None,
                before: Some(before),
                after: None,
            }))
        }
    }
}

impl TursoStore {
    /// The rules that need no database: owner present, writes sensible, no id twice.
    pub(super) fn check_writes(&self, by: &By, writes: &[Write]) -> Result<()> {
        if writes.is_empty() {
            return Err(Error::Invalid("write_by without writes".into()));
        }
        if by.step.is_some() && by.context.is_none() {
            return Err(Error::Invalid("a step needs its run (context)".into()));
        }
        let mut seen = std::collections::HashSet::new();
        for w in writes {
            let id = match w {
                Write::Put { object, .. } => {
                    validate(object)?;
                    if !object.envelope.sensitive.is_empty()
                        && !self.encrypted
                        && !self.allow_sensitive_unencrypted
                    {
                        return Err(Error::EncryptionRequired(object.envelope.id));
                    }
                    object.envelope.id
                }
                Write::Delete { id, .. } => *id,
            };
            if !seen.insert(id) {
                return Err(Error::Invalid(format!("{id} is written twice in one unit")));
            }
        }
        Ok(())
    }

    /// Plan every write, then apply them: the first activity is the head of the unit. Caller holds the lock and a
    /// transaction.
    pub(super) async fn write_unit(
        &self,
        inner: &mut Inner,
        by: &By,
        writes: Vec<Write>,
        birth: bool,
    ) -> Result<Outcome> {
        if let (Some(ctx), Some(step)) = (&by.context, &by.step)
            && let Some(rec) = unit_of_step(&inner.conn, ctx, step).await?
        {
            return Ok(Outcome::Done(rec));
        }
        let mut plans = Vec::with_capacity(writes.len());
        for w in writes {
            match plan_write(&inner.conn, by, w, birth).await? {
                Planned::Change(ch) => plans.push(ch),
                Planned::Stop(o) => return Ok(*o),
            }
        }
        let mut all: Vec<Activity> = Vec::with_capacity(plans.len());
        let mut head_id = None;
        for (i, ch) in plans.into_iter().enumerate() {
            let meta = Meta {
                owner: Some(by.owner.clone()),
                origin: by.origin.clone(),
                context: by.context.clone(),
                step: if i == 0 { by.step.clone() } else { None },
                unit: head_id,
            };
            let act = self.apply_with(inner, &by.actor, ch, &meta).await?;
            if i == 0 {
                head_id = Some(act.id);
            }
            all.push(act);
        }
        Ok(Outcome::Done(Recorded {
            head: all[0].clone(),
            all,
            replayed: false,
        }))
    }

    /// After the transaction of a unit: publish what was written, or re-read the last activity id of a rolled-back
    /// attempt.
    pub(super) async fn finish_write(
        &self,
        inner: &mut Inner,
        res: Result<Outcome>,
    ) -> Result<Outcome> {
        match res {
            Ok(Outcome::Done(rec)) => {
                if !rec.replayed {
                    self.publish(&rec.all);
                }
                Ok(Outcome::Done(rec))
            }
            Ok(other) => Ok(other),
            Err(e) => {
                if let Ok(last) = last_activity_id(&inner.conn).await {
                    inner.last_activity = last;
                }
                Err(e)
            }
        }
    }

    /// See [`ObjectRepository::write_by`](crate::ObjectRepository::write_by).
    pub(super) async fn write_by_impl(&self, by: &By, writes: Vec<Write>) -> Result<Outcome> {
        self.check_writes(by, &writes)?;
        let mut inner = self.inner.lock().await;
        let inner = &mut *inner;
        require_owner(inner)?;
        let conn = inner.conn.clone();
        let res = in_tx(&conn, Box::pin(self.write_unit(inner, by, writes, false))).await;
        self.finish_write(inner, res).await
    }
}

/// Every write needs an owner: no empty one, no default one.
pub(super) fn require_owner(inner: &Inner) -> Result<()> {
    if inner.owner.is_none() {
        return Err(Error::Invalid(
            "store has no owner: open it with OpenOptions::with_owner".into(),
        ));
    }
    Ok(())
}
