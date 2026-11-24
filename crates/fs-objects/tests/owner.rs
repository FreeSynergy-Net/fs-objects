//! The owner of a store is set once and for good: `with_owner`, `birth`, mismatch (S3-3b).

use fs_objects::testing::owner;
use fs_objects::*;
use fs_types::{BasicValue as _, PersonArt, Reference};

mod common;
use common::*;

fn person_of(o: &StoredObject) -> PersonRef {
    PersonRef {
        id: Reference::parse(&format!("urn:uuid:{}", o.envelope.id)).unwrap(),
        art: PersonArt::Person,
        abzug: None,
    }
}

fn put(o: StoredObject) -> Write {
    Write::Put {
        object: o,
        expect: None,
    }
}

fn done(o: Outcome) -> Recorded {
    match o {
        Outcome::Done(r) => r,
        other => panic!("expected Done, got {other:?}"),
    }
}

fn invalid(r: Result<Outcome>, word: &str) {
    match r {
        Err(Error::Invalid(m)) => assert!(m.contains(word), "{m}"),
        other => panic!("expected Invalid({word}), got {other:?}"),
    }
}

/// (9) `birth` writes the person that owns the store: once; afterwards the file knows its owner.
#[tokio::test]
async fn birth_happens_once_and_the_owner_stays() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("p.db");
    let me = obj("Person", "me");
    let by = By::new(person_of(&me).id.canonical(), person_of(&me));
    {
        let s = TursoStore::open(&path).await.unwrap();
        let r = done(s.birth(&by, me.clone()).await.unwrap());
        assert_eq!(r.head.owner, Some(person_of(&me)));
        let stored = s.get(me.envelope.id).await.unwrap().unwrap();
        assert_eq!(stored.owner, Some(person_of(&me)));
        // the store writes for its owner now, with no further ceremony
        done(s.write_by(&by, vec![put(obj("Note", "n"))]).await.unwrap());
    }
    // the next open needs no `with_owner`: the file holds it
    let s = TursoStore::open(&path).await.unwrap();
    done(s.write_by(&by, vec![put(obj("Note", "m"))]).await.unwrap());
    assert_eq!(s.count(&Query::default()).await.unwrap(), 3);
}

/// (10) A second `birth` with another uuid is refused; so is one that is not the object itself.
#[tokio::test]
async fn a_second_birth_is_refused() {
    let s = bare().await;
    let (me, other) = (obj("Person", "me"), obj("Person", "other"));
    let me_by = By::new("me", person_of(&me));
    done(s.birth(&me_by, me).await.unwrap());
    let other_by = By::new("other", person_of(&other));
    invalid(s.birth(&other_by, other).await, "already owned");
    // a store opened with an owner has one: no birth either
    let s = TursoStore::open_with(":memory:", OpenOptions::default().with_owner(owner()))
        .await
        .unwrap();
    let x = obj("Person", "x");
    invalid(
        s.birth(&By::new("x", person_of(&x)), x).await,
        "already owned",
    );
    // the owner must be the object itself, a person, and the only write
    let s = bare().await;
    let (x, y) = (obj("Person", "x"), obj("Person", "y"));
    invalid(
        s.birth(&By::new("x", person_of(&y)), x.clone()).await,
        "itself",
    );
    let mut group = person_of(&x);
    group.art = PersonArt::Group;
    invalid(s.birth(&By::new("x", group), x).await, "person");
    assert_eq!(
        s.count(&Query::default()).await.unwrap(),
        0,
        "nothing was written"
    );
    assert!(
        matches!(
            s.write_by(&By::new("x", owner()), vec![put(obj("Note", "n"))]).await,
            Err(Error::Invalid(m)) if m.contains("no owner")
        ),
        "a refused birth leaves the store without an owner"
    );
}

/// (11) `write_by` never creates an object that is its own owner; that is `birth` only.
#[tokio::test]
async fn self_ownership_is_refused_in_write_by() {
    let s = TursoStore::open_with(":memory:", OpenOptions::default().with_owner(owner()))
        .await
        .unwrap();
    let me = obj("Person", "me");
    let by = By::new("me", person_of(&me));
    invalid(s.write_by(&by, vec![put(me)]).await, "self-ownership");
    assert_eq!(s.count(&Query::default()).await.unwrap(), 0);
}

/// (12) Opening a file for another owner is refused; no owner at all takes the stored one.
#[tokio::test]
async fn another_owner_at_open_is_a_mismatch() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("o.db");
    drop(
        TursoStore::open_with(&path, OpenOptions::default().with_owner(owner()))
            .await
            .unwrap(),
    );
    let stranger = person_of(&obj("Person", "s"));
    let r = TursoStore::open_with(&path, OpenOptions::default().with_owner(stranger)).await;
    assert!(matches!(r, Err(Error::Invalid(m)) if m.contains("owner mismatch")));
    let same = TursoStore::open_with(&path, OpenOptions::default().with_owner(owner())).await;
    assert!(same.is_ok());
    drop(same);
    let none = TursoStore::open(&path).await.unwrap();
    done(
        none.write_by(&By::new("a", owner()), vec![put(obj("Note", "n"))])
            .await
            .unwrap(),
    );
}

async fn bare() -> TursoStore {
    TursoStore::open_in_memory().await.unwrap()
}
