//! Review fixes of S3: `rev` continues after delete, restored owner, owner for `put_unlogged` and `log`.

use fs_objects::traits::UnloggedObjects as _;
use fs_objects::*;
use fs_types::{BasicValue as _, PersonArt, Reference};
use serde_json::json;

mod common;
use common::*;

const OWNER: &str = "urn:uuid:01900000-0000-7000-8000-0000000000aa";

fn owner() -> PersonRef {
    PersonRef {
        id: Reference::parse(OWNER).unwrap(),
        art: PersonArt::Person,
        abzug: None,
    }
}

fn by() -> By {
    By::new(ME, owner()).origin(Origin::new("tasks", "i1", "laptop"))
}

async fn owned() -> TursoStore {
    let o = OpenOptions {
        allow_sensitive_unencrypted: true,
        ..OpenOptions::default()
    }
    .with_owner(owner());
    TursoStore::open_with(":memory:", o).await.unwrap()
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

/// A deleted object that comes back (new put) continues its revisions: an old `expect = 1` cannot match again.
#[tokio::test]
async fn rev_continues_after_delete_and_put() {
    let s = owned().await;
    let o = note("a", "1");
    let id = o.envelope.id;
    done(s.write_by(&by(), vec![put(o.clone(), None)]).await.unwrap());
    done(
        s.write_by(
            &by(),
            vec![Write::Delete {
                id,
                expect: Some(1),
            }],
        )
        .await
        .unwrap(),
    );
    done(s.write_by(&by(), vec![put(o.clone(), None)]).await.unwrap());
    assert_eq!(s.get(id).await.unwrap().unwrap().rev, 3);
    let r = s.write_by(&by(), vec![put(o, Some(1))]).await.unwrap();
    assert!(matches!(r, Outcome::Conflict { .. }), "{r:?}");
}

/// `revert_by` of a delete (and its redo) continues the revisions and keeps the owner of the restored image.
#[tokio::test]
async fn revert_of_a_delete_keeps_rev_and_owner() {
    let s = owned().await;
    let other = PersonRef {
        id: Reference::parse("urn:uuid:01900000-0000-7000-8000-0000000000cc").unwrap(),
        art: PersonArt::Person,
        abzug: None,
    };
    let theirs = By::new(ME, other.clone());
    let o = note("a", "1");
    let id = o.envelope.id;
    done(s.write_by(&theirs, vec![put(o, None)]).await.unwrap());
    let del = done(
        s.write_by(&theirs, vec![Write::Delete { id, expect: None }])
            .await
            .unwrap(),
    );
    let back = done(s.revert_by(del.head.id, &by()).await.unwrap());
    let now = s.get(id).await.unwrap().unwrap();
    assert_eq!(now.rev, 3);
    assert_eq!(now.owner, Some(other.clone()));
    let redo = done(s.revert_by(back.head.id, &by()).await.unwrap());
    assert!(s.get(id).await.unwrap().is_none());
    done(s.revert_by(redo.head.id, &by()).await.unwrap());
    let now = s.get(id).await.unwrap().unwrap();
    assert_eq!(now.rev, 5);
    assert_eq!(now.owner, Some(other));
    let r = s.write_by(&theirs, vec![put(now, Some(1))]).await.unwrap();
    assert!(matches!(r, Outcome::Conflict { .. }), "{r:?}");
}

/// `put_unlogged` and `log` need a store owner too, and `put_unlogged` stamps it and refuses self-ownership.
#[tokio::test]
async fn unlogged_and_log_need_an_owner() {
    let s = TursoStore::open_in_memory().await.unwrap();
    let r = s.put_unlogged(note("a", "1")).await;
    assert!(
        matches!(r, Err(Error::Invalid(ref m)) if m.contains("no owner")),
        "{r:?}"
    );
    let r = s
        .log(ActivityKind::Read, ObjectId::new(), ME, json!({}))
        .await;
    assert!(
        matches!(r, Err(Error::Invalid(ref m)) if m.contains("no owner")),
        "{r:?}"
    );
    let s = owned().await;
    let o = note("b", "2");
    s.put_unlogged(o.clone()).await.unwrap();
    assert_eq!(
        s.read_unlogged(o.envelope.id).await.unwrap().unwrap().owner,
        Some(owner())
    );
    let mut me = note("me", "3");
    me.owner = Some(PersonRef {
        id: Reference::parse(&format!("urn:uuid:{}", me.envelope.id)).unwrap(),
        art: PersonArt::Person,
        abzug: None,
    });
    let r = s.put_unlogged(me).await;
    assert!(
        matches!(r, Err(Error::Invalid(ref m)) if m.contains("birth")),
        "{r:?}"
    );
}
