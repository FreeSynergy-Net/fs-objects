//! Every write path takes the owner from the store (or the checked caller), never from the object the caller built.

use fs_objects::traits::UnloggedObjects as _;
use fs_objects::*;
use fs_types::{BasicValue as _, PersonArt, Reference};
use serde_json::json;

mod common;
use common::*;

fn person(last: &str) -> PersonRef {
    PersonRef {
        id: Reference::parse(&format!(
            "urn:uuid:01900000-0000-7000-8000-0000000000{last}"
        ))
        .unwrap(),
        art: PersonArt::Person,
        abzug: None,
    }
}

fn selfowned(mut o: StoredObject) -> StoredObject {
    o.owner = Some(PersonRef {
        id: Reference::parse(&format!("urn:uuid:{}", o.envelope.id)).unwrap(),
        art: PersonArt::Person,
        abzug: None,
    });
    o
}

async fn owned() -> TursoStore {
    let o = OpenOptions {
        allow_sensitive_unencrypted: true,
        ..OpenOptions::default()
    }
    .with_owner(person("aa"));
    TursoStore::open_with(":memory:", o).await.unwrap()
}

fn invalid(r: &Result<impl std::fmt::Debug>, word: &str) {
    assert!(
        matches!(r, Err(Error::Invalid(m)) if m.contains(word)),
        "{r:?}"
    );
}

/// `put`, `put_and_log`, `put_unlogged`: self-ownership without birth and a foreign owner are refused; none is stamped.
#[tokio::test]
async fn put_paths_stamp_the_store_owner() {
    let s = owned().await;
    invalid(&s.put(selfowned(note("a", "1")), ME).await, "birth");
    let mut f = note("b", "1");
    f.owner = Some(person("cc"));
    invalid(&s.put(f.clone(), ME).await, "owner");
    invalid(&s.put_and_log(f.clone(), ME, vec![]).await, "owner");
    invalid(&s.put_unlogged(f).await, "owner");
    invalid(&s.put_unlogged(selfowned(note("c", "1"))).await, "birth");
    assert_eq!(s.query(&Query::default()).await.unwrap().len(), 0);
    let mut ok = note("d", "1");
    ok.owner = Some(person("aa"));
    let id = ok.envelope.id;
    s.put(ok, ME).await.unwrap();
    assert_eq!(s.get(id).await.unwrap().unwrap().owner, Some(person("aa")));
    let u = note("e", "1");
    let uid = u.envelope.id;
    s.put_unlogged(u).await.unwrap();
    assert_eq!(
        s.read_unlogged(uid).await.unwrap().unwrap().owner,
        Some(person("aa"))
    );
}

/// An existing object keeps its owner when the caller writes another one; `write_by` refuses both bad owners.
#[tokio::test]
async fn write_by_and_existing_owner() {
    let s = owned().await;
    let by = By::new(ME, person("aa"));
    let theirs = By::new(ME, person("cc"));
    let o = note("a", "1");
    let id = o.envelope.id;
    let w = |o: StoredObject| Write::Put {
        object: o,
        expect: None,
    };
    s.write_by(&theirs, vec![w(o.clone())]).await.unwrap();
    let mut mine = o.clone();
    mine.owner = Some(person("aa"));
    invalid(&s.write_by(&by, vec![w(mine.clone())]).await, "owner");
    invalid(&s.put(mine, ME).await, "owner");
    invalid(
        &s.write_by(&by, vec![w(selfowned(note("b", "1")))]).await,
        "owner",
    );
    s.put(o, ME).await.unwrap();
    assert_eq!(s.get(id).await.unwrap().unwrap().owner, Some(person("cc")));
}

/// `undo` of a delete (old path) restores the image with its owner, not the store owner.
#[tokio::test]
async fn undo_restores_the_owner() {
    let s = owned().await;
    let theirs = By::new(ME, person("cc"));
    let o = note("a", "1");
    let id = o.envelope.id;
    s.write_by(
        &theirs,
        vec![Write::Put {
            object: o,
            expect: None,
        }],
    )
    .await
    .unwrap();
    s.delete(id, ME).await.unwrap();
    s.undo(ME, 1).await.unwrap();
    let now = s.get(id).await.unwrap().unwrap();
    assert_eq!(now.owner, Some(person("cc")));
    assert_eq!(now.rev, 3);
}

/// `log` writes no object: its row carries the store owner.
#[tokio::test]
async fn log_row_carries_the_store_owner() {
    let s = owned().await;
    let a = s
        .log(ActivityKind::Read, ObjectId::new(), ME, json!({}))
        .await
        .unwrap();
    assert_eq!(a.owner, Some(person("aa")));
}

/// compact: the last revision of a deleted object survives, so a recreated id continues and an old `expect` conflicts.
#[tokio::test]
async fn compact_keeps_the_revision_of_a_deleted_object() {
    let s = owned().await;
    let by = By::new(ME, person("aa"));
    let o = note("a", "1");
    let id = o.envelope.id;
    let w = |expect| Write::Put {
        object: o.clone(),
        expect,
    };
    s.write_by(&by, vec![w(None)]).await.unwrap();
    s.delete(id, ME).await.unwrap();
    let all = s.activities(&ActivityFilter::default()).await.unwrap();
    s.mark_synced(all.last().unwrap().id).await.unwrap();
    s.set_retention(Retention::steps(0)).await.unwrap();
    s.compact().await.unwrap();
    s.write_by(&by, vec![w(None)]).await.unwrap();
    assert_eq!(s.get(id).await.unwrap().unwrap().rev, 3);
    let r = s.write_by(&by, vec![w(Some(1))]).await.unwrap();
    assert!(matches!(r, Outcome::Conflict { .. }), "{r:?}");
}
