//! `revert_by`: take back a whole unit (all objects of one `write_by`) as one new unit.
//!
//! Pattern: Command (inverse of a recorded unit of work) — the unit is checked against the stored revisions first,
//! nothing is written when one object changed since; the inverse is written as one `Undo` unit with the caller's origin.

use turso::Value;

use super::TursoStore;
use super::activities::load_activity;
use super::codec::decode_activity;
use super::objects::load_object;
use super::schema::ACTIVITY_COLS;
use super::sql::collect;
use super::store::{Change, Inner, Meta};
use super::tx::in_tx;
use super::write_by::{require_owner, unit_of_step};
use super::write_clash::{clash_between, last_change};
use crate::{
    Activity, ActivityId, ActivityKind, By, ChangeSet, Error, Outcome, Recorded, Result,
    StoredObject,
};

/// True if the unit's head says its effect left the store (mail sent, file written outside): it cannot be taken
/// back by images. Nothing sets this yet (state-changing payloads are `ChangeSet`s); the branch keeps the contract whole.
fn effected_outside(head: &Activity) -> bool {
    head.payload.get("effected_outside") == Some(&serde_json::Value::Bool(true))
}

/// The unit of `change`: its head and all members, head first. `None` if the log has no such entry.
async fn load_unit(inner: &Inner, change: ActivityId) -> Result<Option<Vec<Activity>>> {
    let Some(mut head) = load_activity(&inner.conn, change).await? else {
        return Ok(None);
    };
    if let Some(u) = head.unit {
        let Some(h) = load_activity(&inner.conn, u).await? else {
            return Ok(None);
        };
        head = h;
    }
    let rest = collect(
        &inner.conn,
        &format!("SELECT {ACTIVITY_COLS} FROM activities WHERE unit = ? ORDER BY id"),
        vec![Value::Text(head.id.to_string())],
    )
    .await?;
    let mut all = vec![head];
    for r in &rest {
        all.push(decode_activity(r)?);
    }
    Ok(Some(all))
}

/// What the check of one member came to.
enum Checked {
    Ok(Box<Change>),
    Stop(Box<Outcome>),
}

/// Does the stored object still stand where this activity left it (compared by `rev`)?
fn stands(current: Option<&StoredObject>, act: &Activity, after: Option<&StoredObject>) -> bool {
    match (current, after) {
        (None, None) => true,
        (Some(c), Some(_)) if act.revision > 0 => c.rev == act.revision,
        (Some(c), Some(a)) => c.matches_snapshot(a),
        _ => false,
    }
}

async fn check_member(inner: &Inner, act: &Activity) -> Result<Checked> {
    let cs: ChangeSet = act.change_set()?;
    let current = load_object(&inner.conn, act.object).await?;
    if !stands(current.as_ref(), act, cs.after.as_ref()) {
        let Some(by) = last_change(&inner.conn, act.object).await? else {
            return Ok(Checked::Stop(Box::new(Outcome::Gone)));
        };
        let fields = clash_between(cs.after.as_ref(), current.as_ref())?;
        return Ok(Checked::Stop(Box::new(Outcome::Conflict {
            by: Box::new(by),
            fields,
        })));
    }
    let mut restored = cs.before;
    if let (Some(r), Some(c)) = (restored.as_mut(), current.as_ref())
        && r.original.is_none()
    {
        r.original.clone_from(&c.original);
    }
    Ok(Checked::Ok(Box::new(Change {
        kind: ActivityKind::Undo,
        object: act.object,
        target: Some(act.id),
        before: current,
        after: restored,
    })))
}

impl TursoStore {
    /// Plan and write the inverse of the unit. Caller holds the lock and a transaction.
    async fn revert_unit(&self, inner: &mut Inner, by: &By, change: ActivityId) -> Result<Outcome> {
        if let (Some(ctx), Some(step)) = (&by.context, &by.step)
            && let Some(rec) = unit_of_step(&inner.conn, ctx, step).await?
        {
            return Ok(Outcome::Done(rec));
        }
        let Some(unit) = load_unit(inner, change).await? else {
            return Ok(Outcome::Gone);
        };
        if unit.iter().any(Activity::is_redacted) {
            return Ok(Outcome::Gone);
        }
        if unit.iter().any(|a| !a.kind.changes_state()) {
            return Ok(Outcome::NotRevertible(format!(
                "{change} is not a change of an object"
            )));
        }
        if effected_outside(&unit[0]) {
            return Ok(Outcome::NotRevertible(format!(
                "{change} has an effect outside the store"
            )));
        }
        let mut plans = Vec::with_capacity(unit.len());
        for act in unit.iter().rev() {
            match check_member(inner, act).await? {
                Checked::Ok(ch) => plans.push(*ch),
                Checked::Stop(o) => return Ok(*o),
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

    /// See [`UndoRedo::revert_by`](crate::UndoRedo::revert_by).
    pub(super) async fn revert_by_impl(&self, change: ActivityId, by: &By) -> Result<Outcome> {
        if by.step.is_some() && by.context.is_none() {
            return Err(Error::Invalid("a step needs its run (context)".into()));
        }
        let mut inner = self.inner.lock().await;
        let inner = &mut *inner;
        require_owner(inner)?;
        let conn = inner.conn.clone();
        let res = in_tx(&conn, Box::pin(self.revert_unit(inner, by, change))).await;
        self.finish_write(inner, res).await
    }
}
