//! The period run: due rows are removed with the hook of the domain, due objects are deleted for good.
#![cfg(feature = "v5")]
#![allow(clippy::many_single_char_names)]

mod common_v5;

use std::sync::atomic::{AtomicUsize, Ordering};

use common_v5::{actor, count, domain, id, store};
use fs_objects::v5::codec::{get_text, id_value, int, text};
use fs_objects::v5::envelope::{insert, touch, trash};
use fs_objects::v5::log::{mark_unlogged, purge_log};
use fs_objects::v5::migrate::StepFuture;
use fs_objects::v5::purge::{Swept, sweep};
use fs_objects::v5::{Conn, Expiry};
use fs_types::{PersonRef, Revision};

const T0: i64 = 1_794_823_200_000;
const DAY: i64 = 86_400_000;
static HOOKED: AtomicUsize = AtomicUsize::new(0);

fn forget_note<'a>(
    c: &'a Conn<'_>,
    key: &'a Vec<turso::Value>,
    _now: i64,
    _actor: &'a PersonRef,
) -> StepFuture<'a> {
    Box::pin(async move {
        HOOKED.fetch_add(1, Ordering::SeqCst);
        c.exec(
            "DELETE FROM demo_note WHERE id = ?",
            vec![text(&get_text(key, 0)?)],
        )
        .await
        .map(|_| ())
    })
}

const EXPIRIES: &[Expiry] = &[Expiry {
    table: "demo_cut",
    until: "until",
    key: &["note"],
    on_expired: Some(forget_note),
}];

#[tokio::test]
async fn due_rows_go_with_their_hook_and_objects_after_the_trash_period() {
    let s = store(EXPIRIES).await;
    let d = domain(EXPIRIES);
    let (keep, gone) = (id(T0), id(T0));
    let (n1, n2) = (id(T0), id(T0));
    let c = s.begin().await.unwrap();
    let r = async {
        for (o, n) in [(keep, "keep"), (gone, "gone")] {
            insert(&c, o, "Note", n, &actor(), T0).await?;
        }
        for (n, o) in [(n1, keep), (n2, keep)] {
            c.exec(
                "INSERT INTO demo_note (id, object, body) VALUES (?, ?, 'b')",
                vec![id_value(n), id_value(o)],
            )
            .await?;
        }
        c.exec(
            "INSERT INTO demo_cut (note, until) VALUES (?, ?)",
            vec![id_value(n1), int(T0 + DAY)],
        )
        .await?;
        c.exec(
            "INSERT INTO demo_cut (note, until) VALUES (?, ?)",
            vec![id_value(n2), int(T0 + 9 * DAY)],
        )
        .await?;
        trash(&c, gone, Revision(1), T0).await.map(|_| ())
    }
    .await;
    c.finish(r).await.unwrap();
    let c = s.begin().await.unwrap();
    let early = sweep(&c, &d, T0 + 2 * DAY, &actor()).await.unwrap();
    assert_eq!(
        early,
        Swept {
            objects: 0,
            rows: vec![("demo_cut", 1)]
        }
    );
    c.finish(Ok(())).await.unwrap();
    assert_eq!(
        (
            HOOKED.load(Ordering::SeqCst),
            count(&s, "demo_note").await,
            count(&s, "demo_cut").await
        ),
        (1, 1, 1)
    );
    let c = s.begin().await.unwrap();
    let late = sweep(&c, &d, T0 + 31 * DAY, &actor()).await.unwrap();
    assert_eq!(
        late,
        Swept {
            objects: 1,
            rows: vec![("demo_cut", 1)]
        }
    );
    c.finish(Ok(())).await.unwrap();
    assert_eq!(
        count(&s, "objects").await,
        1,
        "the trashed object is gone, the other stays"
    );
}

/// The case of the calendar review kern-3c: the title "Geheim" was changed; after the period the envelope must show
/// the old value again, not the secret one.
fn scrub_title<'a>(
    c: &'a Conn<'_>,
    key: &'a Vec<turso::Value>,
    now: i64,
    actor: &'a PersonRef,
) -> StepFuture<'a> {
    Box::pin(async move {
        let object = get_text(key, 0)?;
        let rows = c
            .query(
                "SELECT body FROM demo_note WHERE object = ?",
                vec![text(&object)],
            )
            .await?;
        assert_eq!(actor.art, fs_types::PersonArt::Person);
        let old = get_text(&rows[0], 0)?;
        let obj = fs_objects::v5::codec::parse_id(&object).unwrap();
        c.exec(
            "UPDATE demo_note SET body = 'scrubbed' WHERE object = ?",
            vec![text(&object)],
        )
        .await?;
        touch(c, obj, Some(&old), now).await.map(|_| ())
    })
}

const SCRUB: &[Expiry] = &[Expiry {
    table: "demo_cut",
    until: "until",
    key: &["note"],
    on_expired: Some(scrub_title),
}];

#[tokio::test]
async fn a_hook_carries_the_envelope_along_and_leaves_no_changed_title_behind() {
    let s = store(SCRUB).await;
    let o = id(T0);
    let c = s.begin().await.unwrap();
    let r = async {
        insert(&c, o, "Note", "Geheim", &actor(), T0).await?;
        // the hook finds the object's old title in the note body; the cut row is keyed by the object here
        c.exec(
            "INSERT INTO demo_note (id, object, body) VALUES (?, ?, 'Alt')",
            vec![id_value(id(T0)), id_value(o)],
        )
        .await?;
        c.exec(
            "INSERT INTO demo_cut (note, until) VALUES (?, ?)",
            vec![id_value(o), int(T0 + DAY)],
        )
        .await
        .map(|_| ())
    }
    .await;
    c.finish(r).await.unwrap();
    let c = s.begin().await.unwrap();
    sweep(&c, &domain(SCRUB), T0 + 2 * DAY, &actor())
        .await
        .unwrap();
    c.finish(Ok(())).await.unwrap();
    let rows = s
        .read()
        .await
        .query("SELECT name, name_sort, rev FROM objects", Vec::new())
        .await
        .unwrap();
    assert_eq!(
        rows[0][..],
        [text("Alt"), text("alt"), int(2)],
        "the envelope follows the data"
    );
}

#[tokio::test]
async fn the_final_delete_takes_subtasks_of_subtasks_and_their_marks() {
    let s = store(&[]).await;
    let (top, mid, leaf) = (id(T0), id(T0), id(T0));
    let c = s.begin().await.unwrap();
    let r = async {
        for (o, n) in [(top, "task"), (mid, "subtask"), (leaf, "subsubtask")] {
            insert(&c, o, "Task", n, &actor(), T0).await?;
        }
        for (p, ch) in [(top, mid), (mid, leaf)] {
            c.exec(
                "INSERT INTO demo_link (master, child) VALUES (?, ?)",
                vec![id_value(p), id_value(ch)],
            )
            .await?;
        }
        mark_unlogged(&c, leaf).await?;
        purge_log(&c, mid).await?;
        trash(&c, top, Revision(1), T0).await.map(|_| ())
    }
    .await;
    c.finish(r).await.unwrap();
    let c = s.begin().await.unwrap();
    sweep(&c, &domain(&[]), T0 + 31 * DAY, &actor())
        .await
        .unwrap();
    c.finish(Ok(())).await.unwrap();
    for t in ["objects", "demo_link", "unlogged_ids", "purged_objects"] {
        assert_eq!(count(&s, t).await, 0, "{t}");
    }
}
