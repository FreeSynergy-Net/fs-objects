//! R3 (one transaction), R4 (foreign keys, cascade, trash instead of delete), R5 (CHECK refuses), compare-and-set.
#![cfg(feature = "v5")]
#![allow(clippy::many_single_char_names)]

mod common_v5;

use common_v5::{actor, count, domain, id, store};
use fs_objects::v5::codec::{id_text, id_value, text};
use fs_objects::v5::envelope::{
    bump, delete_final, insert, set_trash_days, state, trash, trash_days, untrash,
};
use fs_objects::v5::{Error, Store};
use fs_types::{ObjectId, Revision};
use turso::Value;

const T0: i64 = 1_794_823_200_000;
const DAY: i64 = 86_400_000;

async fn make(s: &Store, name: &str) -> ObjectId {
    let o = id(T0);
    let c = s.begin().await.unwrap();
    let r = async {
        insert(&c, o, "Note", name, &actor(), T0).await?;
        let note = id_text(id(T0));
        c.exec(
            "INSERT INTO demo_note (id, object, body) VALUES (?, ?, 'b')",
            vec![text(&note), id_value(o)],
        )
        .await
        .map(|_| ())
    }
    .await;
    c.finish(r).await.unwrap();
    o
}

#[tokio::test]
async fn r3_a_failure_in_a_late_table_leaves_no_row_anywhere() {
    let s = store(&[]).await;
    let o = id(T0);
    let c = s.begin().await.unwrap();
    let r = async {
        insert(&c, o, "Note", "x", &actor(), T0).await?;
        c.exec(
            "INSERT INTO demo_note (id, object, body) VALUES ('nope', 'nope', 'b')",
            Vec::new(),
        )
        .await
        .map(|_| ())
    }
    .await;
    assert!(c.finish(r).await.is_err());
    assert_eq!(count(&s, "objects").await, 0, "the envelope is gone too");
}

#[tokio::test]
async fn r4_foreign_keys_are_on_and_the_database_refuses_a_dangling_row() {
    let s = store(&[]).await;
    let fk = s
        .read()
        .await
        .query("PRAGMA foreign_keys", Vec::new())
        .await
        .unwrap();
    assert_eq!(fk[0][0], Value::Integer(1));
    let c = s.begin().await.unwrap();
    let dangling = format!(
        "INSERT INTO demo_note (id, object, body) VALUES ('{}', '{}', 'b')",
        id_text(id(T0)),
        id_text(id(T0))
    );
    let r = c.exec(&dangling, Vec::new()).await;
    assert!(matches!(r, Err(Error::Invalid(_))), "{r:?}");
    c.finish(Ok(())).await.unwrap();
}

#[tokio::test]
async fn compare_and_set_and_trash_and_untrash() {
    let s = store(&[]).await;
    let o = make(&s, "a").await;
    let c = s.begin().await.unwrap();
    assert_eq!(
        bump(&c, o, Revision(1), Some("A"), T0 + 1).await.unwrap(),
        Revision(2)
    );
    let stale = bump(&c, o, Revision(1), None, T0 + 2).await;
    assert_eq!(
        stale,
        Err(Error::Changed {
            current: Revision(2)
        })
    );
    let (rev, until) = trash(&c, o, Revision(2), T0 + 3).await.unwrap();
    assert_eq!((rev, until), (Revision(3), T0 + 3 + 30 * DAY));
    assert_eq!(
        bump(&c, o, Revision(3), None, T0 + 4).await,
        Err(Error::NotFound(o)),
        "no write in the trash"
    );
    assert_eq!(
        trash(&c, o, Revision(3), T0 + 4).await.map(|_| ()),
        Err(Error::NotFound(o))
    );
    assert_eq!(untrash(&c, o, T0 + 5).await.unwrap(), Revision(4));
    let s0 = state(&c, o).await.unwrap().unwrap();
    assert_eq!((s0.rev, s0.deleted, s0.purge_after), (4, None, None));
    assert!(
        matches!(untrash(&c, o, T0 + 6).await, Err(Error::Invalid(_))),
        "not in the trash"
    );
    let ghost = id(T0);
    assert_eq!(
        bump(&c, ghost, Revision(1), None, T0).await,
        Err(Error::NotFound(ghost))
    );
    c.finish(Ok(())).await.unwrap();
}

#[tokio::test]
async fn the_trash_period_is_positive_and_applies_to_later_deletes_only() {
    let s = store(&[]).await;
    let (a, b) = (make(&s, "a").await, make(&s, "b").await);
    let c = s.begin().await.unwrap();
    assert_eq!(trash_days(&c).await.unwrap(), 30);
    assert!(matches!(
        set_trash_days(&c, 0).await,
        Err(Error::Invalid(_))
    ));
    trash(&c, a, Revision(1), T0).await.unwrap();
    set_trash_days(&c, 7).await.unwrap();
    let (_, until) = trash(&c, b, Revision(1), T0).await.unwrap();
    assert_eq!(until, T0 + 7 * DAY);
    assert_eq!(
        state(&c, a).await.unwrap().unwrap().purge_after,
        Some(T0 + 30 * DAY),
        "earlier rows keep theirs"
    );
    assert_eq!(
        untrash(&c, b, T0 + 8 * DAY).await,
        Err(Error::NotFound(b)),
        "the period is over"
    );
    c.finish(Ok(())).await.unwrap();
}

#[tokio::test]
async fn r4_final_delete_cascades_and_takes_the_children_with_it() {
    let s = store(&[]).await;
    let (m, child) = (make(&s, "master").await, make(&s, "exception").await);
    let c = s.begin().await.unwrap();
    c.exec(
        "INSERT INTO demo_link (master, child) VALUES (?, ?)",
        vec![id_value(m), id_value(child)],
    )
    .await
    .unwrap();
    let d = domain(&[]);
    assert!(
        matches!(delete_final(&c, &d, m).await, Err(Error::Invalid(_))),
        "only from the trash"
    );
    trash(&c, m, Revision(1), T0).await.unwrap();
    delete_final(&c, &d, m).await.unwrap();
    c.finish(Ok(())).await.unwrap();
    for t in ["objects", "demo_note", "demo_link"] {
        assert_eq!(count(&s, t).await, 0, "{t}");
    }
}

#[tokio::test]
async fn r5_check_constraints_refuse_a_half_trash_row_and_a_revision_of_zero() {
    let s = store(&[]).await;
    let o = make(&s, "a").await;
    let c = s.begin().await.unwrap();
    let half = c
        .exec(
            "UPDATE objects SET deleted = 5 WHERE id = ?",
            vec![id_value(o)],
        )
        .await;
    assert!(matches!(half, Err(Error::Invalid(_))), "{half:?}");
    let zero = c
        .exec("UPDATE objects SET rev = 0 WHERE id = ?", vec![id_value(o)])
        .await;
    assert!(matches!(zero, Err(Error::Invalid(_))), "{zero:?}");
    c.finish(Ok(())).await.unwrap();
}
