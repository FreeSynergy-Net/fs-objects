//! Teil A, `settings_profile` / `settings_active`: K-cases KA6, KA7, KA8, KA17.
#![cfg(feature = "v5")]
#![allow(clippy::many_single_char_names)]

mod common_setting;

use common_setting::{at, dir, file, invalid_is, mem};
use fs_objects::v5::setting::{
    IndexChange, IndexRows, PlaceRow, Prim, ScopeRow, SettingRows, Writer,
};

fn create(id: i64, name: &str) -> IndexChange {
    IndexChange::Create {
        id,
        name: name.to_owned(),
        name_key: name.to_lowercase(),
    }
}

async fn change(
    r: &SettingRows<'_>,
    t: i64,
    c: Vec<IndexChange>,
) -> fs_objects::v5::Result<IndexRows> {
    r.change_index::<()>(at(t), &mut |_| Ok(c.clone()))
        .await
        .map(|x| x.unwrap())
}

/// KA6: ids only grow, names are unique among the living, a deleted profile is a tombstone and frees its name.
#[tokio::test]
async fn ka6_index_changes_ids_names_and_tombstones() {
    let s = mem().await;
    let r = SettingRows::new(&s).await.unwrap();
    let i = change(&r, 2, vec![create(1, "Work")]).await.unwrap();
    assert_eq!(
        (i.top, i.active, i.profiles.len()),
        (1, 0, 2),
        "base + Work"
    );
    assert_eq!(i.profiles[0].name, None, "the base has no name");
    let rev = r.rev().await.unwrap();
    let res = change(&r, 3, vec![create(1, "Talk")]).await;
    assert!(
        invalid_is(&res, "settings_profile.id", "id-invalid"),
        "id <= top: {res:?}"
    );
    let res = change(&r, 3, vec![create(2, "WORK")]).await;
    assert!(
        invalid_is(&res, "settings_profile.name_key", "exists"),
        "{res:?}"
    );
    change(&r, 3, vec![create(2, "Talk")]).await.unwrap();
    let rename = IndexChange::Rename {
        id: 2,
        name: "work".into(),
        name_key: "work".into(),
    };
    let res = change(&r, 4, vec![rename]).await;
    assert!(
        invalid_is(&res, "settings_profile.name_key", "exists"),
        "rename onto a living name: {res:?}"
    );
    assert_eq!(
        r.rev().await.unwrap(),
        rev + 1,
        "refused changes do not write (only Talk did)"
    );
    // Delete the active profile: its values go in ALL scopes, the active one falls back to the base.
    change(&r, 5, vec![IndexChange::Switch { id: 1 }])
        .await
        .unwrap();
    assert_eq!(r.index().await.unwrap().active, 1);
    let v = Prim::Int(1);
    for (sc, pl) in [
        (ScopeRow::General, PlaceRow::Profile),
        (ScopeRow::Program("tasks"), PlaceRow::Profile),
        (ScopeRow::Program("tasks"), PlaceRow::Device),
    ] {
        r.put(1, sc, pl, "k", Some(&v), Writer::default(), at(6))
            .await
            .unwrap();
    }
    r.put(
        2,
        ScopeRow::General,
        PlaceRow::Profile,
        "k",
        Some(&v),
        Writer::default(),
        at(6),
    )
    .await
    .unwrap();
    let i = change(&r, 7, vec![IndexChange::Delete { id: 1 }])
        .await
        .unwrap();
    assert_eq!(i.active, 0, "the active profile fell back to the base");
    let dead = i.profiles.iter().find(|p| p.id == 1).unwrap();
    assert_eq!(dead.deleted, Some(at(7)), "a tombstone, not a removed row");
    for (sc, pl) in [
        (ScopeRow::General, PlaceRow::Profile),
        (ScopeRow::Program("tasks"), PlaceRow::Profile),
        (ScopeRow::Program("tasks"), PlaceRow::Device),
    ] {
        assert!(
            r.values(1, sc, pl).await.unwrap().is_empty(),
            "{sc:?} {pl:?}"
        );
    }
    assert_eq!(
        r.values(2, ScopeRow::General, PlaceRow::Profile)
            .await
            .unwrap()
            .len(),
        1,
        "others stay"
    );
    // The name is free again; the id is never reissued.
    let res = change(&r, 8, vec![create(1, "Work")]).await;
    assert!(
        invalid_is(&res, "settings_profile.id", "id-invalid"),
        "a tombstone is never revived: {res:?}"
    );
    let i = change(&r, 8, vec![create(3, "Work")]).await.unwrap();
    assert_eq!(i.top, 3);
    assert_eq!(
        i.profiles
            .iter()
            .filter(|p| p.name.as_deref() == Some("Work"))
            .count(),
        2,
        "old tombstone + new"
    );
}

/// KA7: an error of the closure changes nothing and comes back as the inner `Err`.
#[tokio::test]
async fn ka7_a_closure_error_changes_nothing() {
    let s = mem().await;
    let r = SettingRows::new(&s).await.unwrap();
    change(&r, 2, vec![create(1, "Work")]).await.unwrap();
    let (rev, before) = (r.rev().await.unwrap(), r.index().await.unwrap());
    let out = r
        .change_index(at(3), &mut |_| Err::<Vec<IndexChange>, _>("no"))
        .await
        .unwrap();
    assert_eq!(out, Err("no"));
    assert_eq!(r.rev().await.unwrap(), rev);
    assert_eq!(r.index().await.unwrap(), before);
    // A change that fails after another one was applied rolls the first back too (all or nothing).
    let res = change(&r, 4, vec![create(2, "Talk"), create(2, "Other")]).await;
    assert!(
        invalid_is(&res, "settings_profile.id", "id-invalid"),
        "{res:?}"
    );
    assert_eq!(r.rev().await.unwrap(), rev);
    assert_eq!(
        r.index().await.unwrap(),
        before,
        "the first change is not kept"
    );
    // The closure sees the index as stored now, inside the transaction.
    let out = r
        .change_index::<()>(at(4), &mut |i| Ok(vec![create(i.top + 1, "Talk")]))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out.top, 2);
}

/// KA8: two creations at the same time end with the ids 1 and 2 (the closure reads `top` inside the transaction).
#[tokio::test]
async fn ka8_concurrent_creations_get_distinct_ids() {
    let s = mem().await;
    let r = SettingRows::new(&s).await.unwrap();
    let make = |name: &'static str| async {
        r.change_index::<()>(at(2), &mut |i| Ok(vec![create(i.top + 1, name)]))
            .await
            .unwrap()
            .unwrap()
    };
    // Read and write really interleave: the connection is held, both calls queue on it (the first one queued first), then it is
    // released. A change that reads the index BEFORE `begin` would give both the stale `top` 0 and a clash on id 1.
    let held = s.read().await;
    let (a, b, ()) = tokio::join!(make("Work"), make("Talk"), async move {
        tokio::task::yield_now().await;
        drop(held);
    });
    assert_ne!(a.top, b.top);
    assert_eq!((a.top, b.top), (1, 2), "the second sees the first's id");
    let mut ids: Vec<i64> = r
        .index()
        .await
        .unwrap()
        .profiles
        .iter()
        .map(|p| p.id)
        .collect();
    ids.sort_unstable();
    assert_eq!(ids, vec![0, 1, 2]);
}

/// KA17: no active row = base; `Switch` sets it; it survives reopening the file.
#[tokio::test]
async fn ka17_active_profile_survives_reopening() {
    let d = dir();
    let p = file(&d);
    let path = p.to_str().unwrap();
    {
        let s = SettingRows::open(path, at(1)).await.unwrap();
        let r = SettingRows::new(&s).await.unwrap();
        assert_eq!(r.index().await.unwrap().active, 0);
        change(&r, 2, vec![create(1, "Work")]).await.unwrap();
        change(&r, 3, vec![IndexChange::Switch { id: 1 }])
            .await
            .unwrap();
        assert_eq!(r.index().await.unwrap().active, 1);
    }
    let s = SettingRows::open(path, at(9)).await.unwrap();
    let r = SettingRows::new(&s).await.unwrap();
    assert_eq!(r.index().await.unwrap().active, 1);
    let res = change(&r, 10, vec![IndexChange::Switch { id: 5 }]).await;
    assert!(
        invalid_is(&res, "settings_profile.id", "unknown"),
        "{res:?}"
    );
}
