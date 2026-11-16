//! K1: a unit of work that is cancelled (a timeout, a client that goes away) leaves no open transaction behind.
//! `finish` has one suspension point before the COMMIT (see `v5/conn.rs`); the second test cancels exactly there.
#![cfg(feature = "v5")]

use std::time::Duration;

use fs_objects::v5::Store;

const ID: &str = "01927f00-0000-7000-8000-0000000000ee";

async fn count(s: &Store) -> usize {
    s.read()
        .await
        .query("SELECT 1 FROM purged_objects", Vec::new())
        .await
        .unwrap()
        .len()
}

async fn write(s: &Store, id: &str) {
    let c = s.begin().await.unwrap();
    c.exec(
        &format!("INSERT INTO purged_objects (object_id) VALUES ('{id}')"),
        Vec::new(),
    )
    .await
    .unwrap();
    c.finish(Ok(())).await.unwrap();
}

#[tokio::test]
async fn a_cancelled_future_leaves_no_open_transaction_behind() {
    let s = Store::open(":memory:", 1).await.unwrap();
    let r = tokio::time::timeout(Duration::from_millis(50), async {
        let c = s.begin().await.unwrap();
        c.exec(
            &format!("INSERT INTO purged_objects (object_id) VALUES ('{ID}')"),
            Vec::new(),
        )
        .await
        .unwrap();
        tokio::time::sleep(Duration::from_secs(30)).await;
        c.finish(Ok(())).await
    })
    .await;
    assert!(r.is_err(), "the future was cancelled");
    assert_eq!(
        count(&s).await,
        0,
        "a read after the cancel sees no half write"
    );
    write(&s, "01927f00-0000-7000-8000-0000000000aa").await;
    assert_eq!(count(&s).await, 1, "the next write works");
    drop(s.begin().await.unwrap());
    write(&s, "01927f00-0000-7000-8000-0000000000ab").await;
    assert_eq!(count(&s).await, 2);
}

/// Poll a future exactly once with a waker that does nothing, then drop it: a cancel between two awaits.
fn poll_once_and_drop<F: std::future::Future>(f: F) -> bool {
    let mut f = std::pin::pin!(f);
    let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
    f.as_mut().poll(&mut cx).is_ready()
}

#[tokio::test]
async fn a_future_cancelled_inside_finish_still_rolls_back_at_the_next_access() {
    let s = Store::open(":memory:", 1).await.unwrap();
    let c = s.begin().await.unwrap();
    c.exec(
        &format!("INSERT INTO purged_objects (object_id) VALUES ('{ID}')"),
        Vec::new(),
    )
    .await
    .unwrap();
    let ready = poll_once_and_drop(c.finish(Ok(())));
    assert!(
        !ready,
        "finish must have a suspension point before the COMMIT, or this test proves nothing"
    );
    assert_eq!(
        count(&s).await,
        0,
        "the half finished unit of work is rolled back, not committed"
    );
    write(&s, "01927f00-0000-7000-8000-0000000000ac").await;
    assert_eq!(count(&s).await, 1);
}

#[tokio::test]
async fn an_error_rolls_everything_back() {
    let s = Store::open(":memory:", 1).await.unwrap();
    let c = s.begin().await.unwrap();
    c.exec(
        &format!("INSERT INTO purged_objects (object_id) VALUES ('{ID}')"),
        Vec::new(),
    )
    .await
    .unwrap();
    let r: fs_objects::v5::Result<()> = Err(fs_objects::v5::Error::Unavailable("x".into()));
    assert!(c.finish(r).await.is_err());
    assert_eq!(count(&s).await, 0);
}
