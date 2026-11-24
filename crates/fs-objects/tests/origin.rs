//! Schema 5 and `write_by`: owner, origin, revision, idempotent step, compare-and-set (S3-2).

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

/// The version 4 schema (3 plus `redacted` and the identity row), as 0.0.10 wrote it.
const V4_SCHEMA: &str = "
CREATE TABLE schema_version (version INTEGER NOT NULL);
INSERT INTO schema_version VALUES (4);
CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
INSERT INTO meta VALUES ('store_id', '01900000-0000-4000-8000-0000000000bb');
CREATE TABLE objects (
    id TEXT PRIMARY KEY, iri TEXT, type TEXT NOT NULL, name TEXT NOT NULL, context TEXT,
    attributed_to TEXT, in_reply_to TEXT, published INTEGER NOT NULL, updated INTEGER NOT NULL,
    content_type TEXT NOT NULL, content TEXT NOT NULL, original BLOB, original_type TEXT,
    addressing TEXT NOT NULL, sensitive TEXT NOT NULL, describes TEXT, start_time INTEGER,
    end_time INTEGER, name_sort TEXT NOT NULL DEFAULT ''
);
CREATE TABLE activities (
    id TEXT PRIMARY KEY, kind TEXT NOT NULL, object_id TEXT NOT NULL, actor TEXT NOT NULL,
    at INTEGER NOT NULL, target TEXT, payload TEXT NOT NULL, synced INTEGER NOT NULL DEFAULT 0,
    redacted INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE unlogged_ids (object_id TEXT PRIMARY KEY);
CREATE TABLE purged_objects (object_id TEXT PRIMARY KEY);
CREATE TABLE index_fields (
    object_id TEXT NOT NULL, key TEXT NOT NULL, value TEXT NOT NULL,
    value_norm TEXT NOT NULL DEFAULT ''
);
";

/// (1) A version 4 file migrates to 5 with the owner of the open in every old row, and writes on top of it.
#[tokio::test]
async fn migration_4_to_5_fills_the_columns() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("v4.db");
    {
        let db = turso::Builder::new_local(path.to_str().unwrap())
            .build()
            .await
            .unwrap();
        let c = db.connect().unwrap();
        c.execute_batch(V4_SCHEMA).await.unwrap();
        c.execute_batch(&old_rows_sql()).await.unwrap();
    }
    let opts = OpenOptions::default().with_owner(owner());
    let s = TursoStore::open_with(&path, opts).await.unwrap();
    assert_eq!(s.schema_version(), 5);
    let old = s.get(OLD_OBJECT.parse().unwrap()).await.unwrap().unwrap();
    assert_eq!(old.owner, Some(owner()));
    assert_eq!(old.rev, 1);
    let log = s.activities(&ActivityFilter::default()).await.unwrap();
    assert_eq!(log[0].owner, Some(owner()));
    assert_eq!(log[0].origin, Origin::default());
    assert_eq!((log[0].unit, log[0].step.as_deref()), (None, None));
    // the migrated object takes a compare-and-set write on its revision
    let mut next = old.clone();
    next.content = json!({"n": "new"});
    let r = done(s.write_by(&by(), vec![put(next, Some(1))]).await.unwrap());
    assert_eq!(r.head.revision, 2);
    assert_eq!(r.head.origin.program, "tasks");
}

/// (3) The same (run, step) is written once; the unique index refuses a second head from outside.
#[tokio::test]
async fn a_step_is_written_once_and_the_index_refuses_a_second_head() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("s.db");
    let s = TursoStore::open_with(&path, OpenOptions::default().with_owner(owner()))
        .await
        .unwrap();
    let a = note("a", "1");
    let b = by().step("run-1", "step-1");
    let first = done(s.write_by(&b, vec![put(a.clone(), None)]).await.unwrap());
    assert!(!first.replayed);
    assert_eq!(first.head.step.as_deref(), Some("step-1"));
    let again = done(
        s.write_by(&b, vec![put(note("other", "2"), None)])
            .await
            .unwrap(),
    );
    assert!(again.replayed);
    assert_eq!(again.head.id, first.head.id);
    let all = s.activities(&ActivityFilter::default()).await.unwrap();
    assert_eq!(all.len(), 1, "the repetition wrote nothing");
    assert_eq!(s.count(&Query::default()).await.unwrap(), 1);
    drop(s);
    // a foreign writer cannot add a second head for the same step
    let db = turso::Builder::new_local(path.to_str().unwrap())
        .build()
        .await
        .unwrap();
    let c = db.connect().unwrap();
    let sql = "INSERT INTO activities (id, kind, object_id, actor, at, payload, context, step) \
               VALUES ('01900000-0000-7000-8000-0000000000ff', 'Update', 'x', 'y', 1, '{}', 'run-1', 'step-1')";
    assert!(c.execute(sql, ()).await.is_err());
}

/// (4) A wrong `expect` is a `Conflict` with the last change and the paths that changed; nothing is written.
#[tokio::test]
async fn a_wrong_expect_conflicts_with_the_changed_fields() {
    let s = owned().await;
    let a = card("Ada");
    let id = a.envelope.id;
    done(s.write_by(&by(), vec![put(a.clone(), None)]).await.unwrap());
    let mut v2 = s.get(id).await.unwrap().unwrap();
    assert_eq!(v2.rev, 1);
    v2.content = json!({"@type":"Card","name":{"full":"Ada L."}});
    let other = By::new("someone-else", owner()).origin(Origin::new("other", "", ""));
    done(s.write_by(&other, vec![put(v2, Some(1))]).await.unwrap());
    // the first writer still thinks it is at rev 1
    let mut stale = s.get(id).await.unwrap().unwrap();
    stale.content = json!({"@type":"Card","name":{"full":"Mine"}});
    let before = s
        .activities(&ActivityFilter::default())
        .await
        .unwrap()
        .len();
    let out = s.write_by(&by(), vec![put(stale, Some(1))]).await.unwrap();
    let Outcome::Conflict { by: head, fields } = out else {
        panic!("expected a conflict");
    };
    assert_eq!(head.origin.program, "other");
    assert_eq!(head.revision, 2);
    assert_eq!(
        fields.iter().map(|f| f.path.as_str()).collect::<Vec<_>>(),
        vec!["content.name.full"]
    );
    assert_eq!(
        s.activities(&ActivityFilter::default())
            .await
            .unwrap()
            .len(),
        before
    );
    assert_eq!(s.get(id).await.unwrap().unwrap().rev, 2);
}

/// (5) An object id must be a `UUIDv7`: the constructors refuse any other, and `write_by` refuses a v4 id that came in
/// through serde (a `StoredObject` from the wire), which skips the constructors.
#[tokio::test]
async fn a_v4_id_is_refused_by_write_by() {
    let v4 = uuid::Uuid::new_v4();
    assert!(matches!(ObjectId::from_uuid(v4), Err(Error::Invalid(_))));
    assert!(v4.to_string().parse::<ObjectId>().is_err());
    let mut o = note("a", "1");
    o.envelope.id = serde_json::from_value(json!(v4.to_string())).unwrap();
    let s = owned().await;
    let r = s.write_by(&by(), vec![put(o.clone(), None)]).await;
    assert!(
        matches!(r, Err(Error::Invalid(ref m)) if m.contains("UUIDv7")),
        "{r:?}"
    );
    assert!(s.get(o.envelope.id).await.unwrap().is_none());
}

/// (6) Without an owner nothing is written through `write_by`, and a foreign owner on an object is refused.
#[tokio::test]
async fn writing_needs_an_owner() {
    let s = TursoStore::open_in_memory().await.unwrap();
    let r = s.write_by(&by(), vec![put(note("a", "1"), None)]).await;
    assert!(matches!(r, Err(Error::Invalid(m)) if m.contains("no owner")));
    assert_eq!(s.count(&Query::default()).await.unwrap(), 0);
    let s = owned().await;
    let mut o = note("b", "2");
    let mut foreign = owner();
    foreign.id = Reference::parse("urn:uuid:01900000-0000-7000-8000-0000000000cc").unwrap();
    o.owner = Some(foreign);
    assert!(matches!(
        s.write_by(&by(), vec![put(o, None)]).await,
        Err(Error::Invalid(_))
    ));
}

/// (7) Every row of `write_by` is unsynced and carries owner, origin and the unit; `mark_synced` flips it.
#[tokio::test]
async fn rows_start_unsynced_and_mark_synced_sets_them() {
    let s = owned().await;
    let (a, b) = (note("a", "1"), note("b", "2"));
    let r = done(
        s.write_by(&by().step("r", "s"), vec![put(a, None), put(b, None)])
            .await
            .unwrap(),
    );
    assert_eq!(r.all.len(), 2);
    assert_eq!(r.all[1].unit, Some(r.head.id));
    assert_eq!(r.head.unit, None);
    assert!(r.all[1].step.is_none());
    for act in &r.all {
        assert!(!act.synced);
        assert_eq!(act.owner, Some(owner()));
        assert_eq!(act.origin.device, "laptop");
        assert_eq!(act.context.as_deref(), Some("r"));
    }
    let stored = s.get(r.all[0].object).await.unwrap().unwrap();
    assert_eq!(stored.owner, Some(owner()));
    let last = r.all[1].id;
    assert_eq!(s.mark_synced(last).await.unwrap(), 2);
    let log = s.activities(&ActivityFilter::default()).await.unwrap();
    assert!(log.iter().all(|a| a.synced));
}
