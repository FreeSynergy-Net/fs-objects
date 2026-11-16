//! The hold (v0.0.3): a trashed object stays past its period while a hold lasts or `trash_auto` is off; the limits of
//! the period (one millisecond before and at `purge_after`), the overflow of a huge period, the appended step.
#![cfg(feature = "v5")]
#![allow(clippy::many_single_char_names)]

mod common_v5;

use common_v5::{actor, count, domain, id, store};
use fs_objects::v5::codec::{get_text, id_value};
use fs_objects::v5::envelope::{due, insert, set_trash_days, state, trash, untrash};
use fs_objects::v5::hold::{
    Restorable, hold, is_held, release, restorable_until, set_trash_auto, trash_auto,
};
use fs_objects::v5::purge::sweep;
use fs_objects::v5::{Error, Store};
use fs_types::{ObjectId, Revision};

const T0: i64 = 1_794_823_200_000;
const DAY: i64 = 86_400_000;
const PERIOD: i64 = 30 * DAY;

/// One object in the trash at `T0` (period 30 days).
async fn trashed(s: &Store) -> ObjectId {
    let o = id(T0);
    let c = s.begin().await.unwrap();
    let r = async {
        insert(&c, o, "Note", "n", &actor(), T0).await?;
        trash(&c, o, Revision(1), T0).await.map(|_| ())
    }
    .await;
    c.finish(r).await.unwrap();
    o
}

async fn run(s: &Store, now: i64) -> usize {
    let c = s.begin().await.unwrap();
    let n = sweep(&c, &domain(&[]), now, &actor())
        .await
        .unwrap()
        .objects;
    c.finish(Ok(())).await.unwrap();
    n
}

#[tokio::test]
async fn one_millisecond_before_the_end_the_object_stays_at_the_end_it_is_gone() {
    let s = store(&[]).await;
    let o = trashed(&s).await;
    assert_eq!(run(&s, T0 + PERIOD - 1).await, 0, "purge_after - 1");
    assert_eq!(count(&s, "objects").await, 1);
    let c = s.begin().await.unwrap();
    assert_eq!(untrash(&c, o, T0 + PERIOD - 1).await, Ok(Revision(3)));
    trash(&c, o, Revision(3), T0 + PERIOD - 1).await.unwrap();
    c.finish(Ok(())).await.unwrap();
    // the period restarted at the second trash: now the exact end of the first one is still inside the second
    assert_eq!(run(&s, T0 + PERIOD).await, 0);
    let end = T0 + PERIOD - 1 + PERIOD;
    assert_eq!(run(&s, end).await, 1, "purge_after");
    assert_eq!(count(&s, "objects").await, 0);
    let c = s.read().await;
    assert_eq!(untrash(&c, o, end).await, Err(Error::NotFound(o)));
}

#[tokio::test]
async fn untrash_at_the_exact_end_is_refused_one_millisecond_before_it_is_allowed() {
    let s = store(&[]).await;
    let o = trashed(&s).await;
    let c = s.begin().await.unwrap();
    assert_eq!(
        untrash(&c, o, T0 + PERIOD).await,
        Err(Error::NotFound(o)),
        "purge_after"
    );
    assert!(untrash(&c, o, T0 + PERIOD - 1).await.is_ok());
    c.finish(Ok(())).await.unwrap();
}

#[tokio::test]
async fn a_period_too_long_for_the_clock_is_refused_not_wrapped() {
    let s = store(&[]).await;
    let o = id(T0);
    let c = s.begin().await.unwrap();
    insert(&c, o, "Note", "n", &actor(), T0).await.unwrap();
    set_trash_days(&c, i64::MAX / DAY + 1).await.unwrap();
    let r = trash(&c, o, Revision(1), T0).await;
    assert!(matches!(r, Err(Error::Invalid(_))), "multiplication: {r:?}");
    set_trash_days(&c, i64::MAX / DAY).await.unwrap();
    let r = trash(&c, o, Revision(1), T0).await;
    assert!(matches!(r, Err(Error::Invalid(_))), "addition: {r:?}");
    assert!(state(&c, o).await.unwrap().unwrap().deleted.is_none());
    set_trash_days(&c, 2_000_000).await.unwrap();
    assert!(
        trash(&c, o, Revision(1), T0).await.is_ok(),
        "a long one is fine"
    );
    c.finish(Ok(())).await.unwrap();
}

#[tokio::test]
async fn a_hold_keeps_the_object_until_it_is_over_and_a_hold_without_end_keeps_it_for_good() {
    let s = store(&[]).await;
    let (a, b, free) = (trashed(&s).await, trashed(&s).await, trashed(&s).await);
    let c = s.begin().await.unwrap();
    hold(&c, a, Some(T0 + 40 * DAY)).await.unwrap();
    hold(&c, b, None).await.unwrap();
    assert_eq!(
        (
            is_held(&c, a, T0 + 40 * DAY - 1).await.unwrap(),
            is_held(&c, a, T0 + 40 * DAY).await.unwrap()
        ),
        (true, false),
        "held while until > now"
    );
    assert_eq!(
        due(&c, T0 + PERIOD).await.unwrap(),
        vec![free],
        "a and b are held"
    );
    assert_eq!(
        untrash(&c, a, T0 + 35 * DAY).await,
        Ok(Revision(3)),
        "back after the period, while held"
    );
    trash(&c, a, Revision(3), T0 + 35 * DAY).await.unwrap();
    c.finish(Ok(())).await.unwrap();
    assert_eq!(
        run(&s, T0 + 100 * DAY).await,
        2,
        "a: hold over and period over; free"
    );
    let c = s.read().await;
    assert_eq!(
        state(&c, b).await.unwrap().map(|x| x.rev),
        Some(2),
        "b stays: held without end"
    );
    assert_eq!(due(&c, T0 + 100_000 * DAY).await.unwrap(), Vec::new());
    drop(c);
    let c = s.begin().await.unwrap();
    release(&c, b).await.unwrap();
    assert_eq!(
        due(&c, T0 + 100 * DAY).await.unwrap(),
        vec![b],
        "released: due again"
    );
    assert_eq!(
        untrash(&c, b, T0 + 100 * DAY).await,
        Err(Error::NotFound(b)),
        "no hold, period over"
    );
    c.finish(Ok(())).await.unwrap();
}

#[tokio::test]
async fn a_hold_that_ends_before_the_period_changes_nothing_and_a_hold_needs_an_object() {
    let s = store(&[]).await;
    let o = trashed(&s).await;
    let c = s.begin().await.unwrap();
    hold(&c, o, Some(T0 + DAY)).await.unwrap();
    assert_eq!(due(&c, T0 + PERIOD - 1).await.unwrap(), Vec::new());
    assert_eq!(due(&c, T0 + PERIOD).await.unwrap(), vec![o]);
    assert_eq!(
        restorable_until(&c, o).await,
        Ok(Some(Restorable::Until(T0 + PERIOD)))
    );
    c.finish(Ok(())).await.unwrap();
    let c = s.begin().await.unwrap();
    let r = hold(&c, id(T0), None).await;
    assert!(matches!(r, Err(Error::Invalid(_))), "{r:?}");
    let _ = c.finish(Ok(())).await;
}

#[tokio::test]
async fn with_trash_auto_off_the_sweep_deletes_no_object_and_untrash_stays_possible() {
    let s = store(&[]).await;
    let o = trashed(&s).await;
    let c = s.begin().await.unwrap();
    assert!(trash_auto(&c).await.unwrap(), "on by default");
    set_trash_auto(&c, false).await.unwrap();
    assert!(!trash_auto(&c).await.unwrap());
    c.finish(Ok(())).await.unwrap();
    assert_eq!(run(&s, T0 + 10_000 * DAY).await, 0);
    assert_eq!(count(&s, "objects").await, 1);
    let c = s.begin().await.unwrap();
    assert_eq!(restorable_until(&c, o).await, Ok(Some(Restorable::Never)));
    set_trash_auto(&c, true).await.unwrap();
    assert_eq!(
        due(&c, T0 + PERIOD).await.unwrap(),
        vec![o],
        "on again: due"
    );
    set_trash_auto(&c, false).await.unwrap();
    assert_eq!(untrash(&c, o, T0 + 10_000 * DAY).await, Ok(Revision(3)));
    c.finish(Ok(())).await.unwrap();
}

#[tokio::test]
async fn kept_until_names_the_later_of_period_and_hold_and_nothing_for_a_live_object() {
    let s = store(&[]).await;
    let o = trashed(&s).await;
    let live = id(T0);
    let c = s.begin().await.unwrap();
    insert(&c, live, "Note", "l", &actor(), T0).await.unwrap();
    assert_eq!(restorable_until(&c, live).await, Ok(None));
    let none = id(T0);
    assert_eq!(restorable_until(&c, none).await, Err(Error::NotFound(none)));
    assert_eq!(
        restorable_until(&c, o).await,
        Ok(Some(Restorable::Until(T0 + PERIOD)))
    );
    hold(&c, o, Some(T0 + 50 * DAY)).await.unwrap();
    assert_eq!(
        restorable_until(&c, o).await,
        Ok(Some(Restorable::Until(T0 + 50 * DAY)))
    );
    hold(&c, o, None).await.unwrap();
    assert_eq!(restorable_until(&c, o).await, Ok(Some(Restorable::Never)));
    c.finish(Ok(())).await.unwrap();
}

#[tokio::test]
async fn the_hold_goes_with_the_object_and_the_store_has_the_appended_step_with_the_old_first_print()
 {
    let s = store(&[]).await;
    let o = trashed(&s).await;
    let c = s.begin().await.unwrap();
    hold(&c, o, None).await.unwrap();
    c.finish(Ok(())).await.unwrap();
    assert_eq!(count(&s, "object_hold").await, 1);
    let c = s.begin().await.unwrap();
    release(&c, o).await.unwrap();
    c.finish(Ok(())).await.unwrap();
    assert_eq!(run(&s, T0 + PERIOD).await, 1);
    assert_eq!(count(&s, "object_hold").await, 0);
    let c = s.read().await;
    let rows = c
        .query(
            "SELECT fingerprint FROM fs_step WHERE domain = 'fs_' ORDER BY seq",
            Vec::new(),
        )
        .await
        .unwrap();
    let prints: Vec<String> = rows.iter().map(|r| get_text(r, 0).unwrap()).collect();
    assert_eq!(
        prints.len(),
        3,
        "v0.0.2 had two steps, the hold is the third"
    );
    assert_eq!(prints[0], FIRST_PRINT_V002, "step 1 is unchanged");
    assert_eq!(prints[1], SECOND_PRINT_V002, "step 2 is unchanged");
    let col = c
        .query(
            "SELECT 1 FROM fs_column WHERE domain = 'fs_' AND tbl = 'object_hold'",
            Vec::new(),
        )
        .await
        .unwrap();
    assert_eq!(col.len(), 2);
    let _ = id_value(o);
}

const FIRST_PRINT_V002: &str = "0a2bb7c0baeec7c231fe9e117768380b2447a8e6f892a13d541c7d5ef79ebf88";
const SECOND_PRINT_V002: &str = "172bae5548ed99c15885807c4d3714df4b5bbdc79db298893c29aca020cad30f";
