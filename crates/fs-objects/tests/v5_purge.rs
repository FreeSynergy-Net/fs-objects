//! The period run: due rows are removed with the hook of the domain, due objects are deleted for good.
#![cfg(feature = "v5")]
#![allow(clippy::many_single_char_names)]

mod common_v5;

use std::sync::atomic::{AtomicUsize, Ordering};

use common_v5::{actor, count, domain, id, store};
use fs_objects::v5::codec::{get_text, id_value, int, text};
use fs_objects::v5::envelope::{insert, trash};
use fs_objects::v5::migrate::StepFuture;
use fs_objects::v5::purge::{Swept, sweep};
use fs_objects::v5::{Conn, Expiry};
use fs_types::Revision;

const T0: i64 = 1_794_823_200_000;
const DAY: i64 = 86_400_000;
static HOOKED: AtomicUsize = AtomicUsize::new(0);

fn forget_note<'a>(c: &'a Conn<'_>, key: &'a Vec<turso::Value>) -> StepFuture<'a> {
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
    let early = sweep(&c, &d, T0 + 2 * DAY).await.unwrap();
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
    let late = sweep(&c, &d, T0 + 31 * DAY).await.unwrap();
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
