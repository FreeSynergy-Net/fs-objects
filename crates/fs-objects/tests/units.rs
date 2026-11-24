//! Units of change (S3-3): `revert_by`, `changes_by`, the standard bodies of the new trait methods.

use async_trait::async_trait;
use fs_objects::testing::{options, owner};
use fs_objects::*;
use serde_json::json;

mod common;
use common::*;

fn by(program: &str) -> By {
    By::new(ME, owner()).origin(Origin::new(program, "i1", "laptop"))
}

fn put(o: StoredObject, expect: Option<u64>) -> Write {
    Write::Put { object: o, expect }
}

fn done(o: Outcome) -> Recorded {
    match o {
        Outcome::Done(r) => r,
        other => panic!("expected Done, got {other:?}"),
    }
}

async fn owned() -> TursoStore {
    TursoStore::open_with(":memory:", options()).await.unwrap()
}

fn with(mut o: StoredObject, n: &str) -> StoredObject {
    o.content = json!({ "n": n });
    o
}

/// (2) A `write_by` of two objects is one unit; `revert_by` takes both back as one new unit.
#[tokio::test]
async fn revert_takes_a_whole_unit_back() {
    let s = owned().await;
    let (a, b) = (obj("Note", "a"), obj("Note", "b"));
    let (ia, ib) = (a.envelope.id, b.envelope.id);
    done(
        s.write_by(&by("t"), vec![put(a, None), put(b, None)])
            .await
            .unwrap(),
    );
    let (a1, b1) = (
        s.get(ia).await.unwrap().unwrap(),
        s.get(ib).await.unwrap().unwrap(),
    );
    let unit = done(
        s.write_by(
            &by("t"),
            vec![
                put(with(a1.clone(), "a2"), Some(1)),
                put(with(b1.clone(), "b2"), Some(1)),
            ],
        )
        .await
        .unwrap(),
    );
    let back = done(s.revert_by(unit.head.id, &by("t")).await.unwrap());
    assert_eq!(back.all.len(), 2);
    assert_eq!(back.head.unit, None);
    assert_eq!(back.all[1].unit, Some(back.head.id));
    assert!(
        back.all
            .iter()
            .all(|x| x.kind == ActivityKind::Undo && !x.synced)
    );
    let (a2, b2) = (
        s.get(ia).await.unwrap().unwrap(),
        s.get(ib).await.unwrap().unwrap(),
    );
    assert_eq!((a2.content, b2.content), (a1.content, b1.content));
    assert_eq!((a2.rev, b2.rev), (3, 3));
    // naming a member finds the same unit: the unit is already taken back, so it conflicts now
    let again = s.revert_by(unit.all[1].id, &by("t")).await.unwrap();
    assert!(matches!(again, Outcome::Conflict { .. }));
}

/// A later change of one object stops the revert of the whole unit; nothing is written.
#[tokio::test]
async fn revert_conflicts_when_one_object_changed_since() {
    let s = owned().await;
    let (a, b) = (obj("Note", "a"), obj("Note", "b"));
    let (ia, ib) = (a.envelope.id, b.envelope.id);
    let unit = done(
        s.write_by(&by("t"), vec![put(a, None), put(b, None)])
            .await
            .unwrap(),
    );
    let a1 = s.get(ia).await.unwrap().unwrap();
    done(
        s.write_by(&by("other"), vec![put(with(a1, "changed"), Some(1))])
            .await
            .unwrap(),
    );
    let n = s
        .activities(&ActivityFilter::default())
        .await
        .unwrap()
        .len();
    let out = s.revert_by(unit.head.id, &by("t")).await.unwrap();
    let Outcome::Conflict { by: last, fields } = out else {
        panic!("expected a conflict");
    };
    assert_eq!(last.origin.program, "other");
    assert_eq!(
        fields.iter().map(|f| f.path.as_str()).collect::<Vec<_>>(),
        vec!["content.n"]
    );
    assert_eq!(
        s.activities(&ActivityFilter::default())
            .await
            .unwrap()
            .len(),
        n
    );
    assert!(s.get(ib).await.unwrap().is_some(), "b was not touched");
}

/// The same step twice is the same undo unit; an unknown activity is `Gone`; a log-only entry is not revertible.
#[tokio::test]
async fn revert_is_idempotent_by_step_and_knows_gone() {
    let s = owned().await;
    let a = obj("Note", "a");
    let unit = done(
        s.write_by(&by("t"), vec![put(a.clone(), None)])
            .await
            .unwrap(),
    );
    let b = by("t").step("run", "undo-1");
    let first = done(s.revert_by(unit.head.id, &b).await.unwrap());
    assert!(!first.replayed);
    let second = done(s.revert_by(unit.head.id, &b).await.unwrap());
    assert!(second.replayed);
    assert_eq!(second.head.id, first.head.id);
    assert!(matches!(
        s.revert_by(ActivityId::new(), &by("t")).await.unwrap(),
        Outcome::Gone
    ));
    let note = s
        .log(ActivityKind::Read, a.envelope.id, ME, json!({}))
        .await
        .unwrap();
    assert!(matches!(
        s.revert_by(note.id, &by("t")).await.unwrap(),
        Outcome::NotRevertible(_)
    ));
}

/// `changes_by` lists heads only, filtered by program, run, objects and cursor.
#[tokio::test]
async fn changes_by_lists_heads_with_filters() {
    let s = owned().await;
    let (one, two, three) = (obj("Note", "a"), obj("Note", "b"), obj("Note", "c"));
    let (ia, ic) = (one.envelope.id, three.envelope.id);
    let u1 = done(
        s.write_by(
            &by("tasks").step("r1", "s1"),
            vec![put(one, None), put(two, None)],
        )
        .await
        .unwrap(),
    );
    let u2 = done(
        s.write_by(&by("calendar"), vec![put(three, None)])
            .await
            .unwrap(),
    );
    let f = ChangeFilter::default();
    let all = s.changes_by(&f).await.unwrap();
    assert_eq!(
        all.iter().map(|x| x.id).collect::<Vec<_>>(),
        vec![u1.head.id, u2.head.id]
    );
    let mut f = ChangeFilter::default();
    f.program = Some("tasks".into());
    assert_eq!(s.changes_by(&f).await.unwrap().len(), 1);
    let mut f = ChangeFilter::default();
    f.run = Some("r1".into());
    assert_eq!(s.changes_by(&f).await.unwrap()[0].id, u1.head.id);
    // the second object of a unit finds the unit's head
    let mut f = ChangeFilter::default();
    f.objects = Some(vec![u1.all[1].object]);
    assert_eq!(s.changes_by(&f).await.unwrap()[0].id, u1.head.id);
    f.objects = Some(vec![ic]);
    assert_eq!(s.changes_by(&f).await.unwrap()[0].id, u2.head.id);
    f.objects = Some(vec![ia, ic]);
    f.after = Some(u1.head.id);
    f.limit = Some(1);
    assert_eq!(s.changes_by(&f).await.unwrap()[0].id, u2.head.id);
    f.objects = Some(vec![]);
    assert_eq!(s.changes_by(&f).await.unwrap().len(), 0);
}

/// A store that only has the old methods.
struct Foreign;

#[async_trait]
impl traits::ObjectRepository for Foreign {
    async fn put(&self, _: StoredObject, _: &str) -> Result<Activity> {
        unimplemented!()
    }
    async fn put_and_log(
        &self,
        _: StoredObject,
        _: &str,
        _: Vec<LogEntry>,
    ) -> Result<Vec<Activity>> {
        unimplemented!()
    }
    async fn get(&self, _: ObjectId) -> Result<Option<StoredObject>> {
        unimplemented!()
    }
    async fn delete(&self, _: ObjectId, _: &str) -> Result<Activity> {
        unimplemented!()
    }
}
#[async_trait]
impl traits::UndoRedo for Foreign {
    async fn undo(&self, _: &str, _: usize) -> Result<Vec<Activity>> {
        unimplemented!()
    }
    async fn redo(&self, _: &str, _: usize) -> Result<Vec<Activity>> {
        unimplemented!()
    }
    async fn undo_state(&self, _: &str) -> Result<UndoState> {
        unimplemented!()
    }
}
#[async_trait]
impl traits::ActivityLog for Foreign {
    async fn log(
        &self,
        _: ActivityKind,
        _: ObjectId,
        _: &str,
        _: serde_json::Value,
    ) -> Result<Activity> {
        unimplemented!()
    }
    async fn activities(&self, _: &ActivityFilter) -> Result<Vec<Activity>> {
        unimplemented!()
    }
}

/// (8) The standard bodies of a foreign backend refuse; they never pretend an origin was recorded.
#[tokio::test]
async fn the_standard_bodies_refuse() {
    let f = Foreign;
    let b = by("t");
    assert!(
        matches!(traits::ObjectRepository::write_by(&f, &b, vec![]).await, Err(Error::Invalid(m)) if m.contains("write_by"))
    );
    assert!(
        matches!(traits::ObjectRepository::birth(&f, &b, obj("Note", "a")).await, Err(Error::Invalid(m)) if m.contains("birth"))
    );
    assert!(
        matches!(traits::UndoRedo::revert_by(&f, ActivityId::new(), &b).await, Err(Error::Invalid(m)) if m.contains("revert_by"))
    );
    assert!(
        matches!(traits::ActivityLog::changes_by(&f, &ChangeFilter::default()).await, Err(Error::Invalid(m)) if m.contains("changes_by"))
    );
}
