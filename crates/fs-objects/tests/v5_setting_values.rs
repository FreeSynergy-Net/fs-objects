//! Teil A, `settings_value`: K-cases KA3, KA4, KA5, KA11, KA14.
#![cfg(feature = "v5")]
#![allow(clippy::many_single_char_names)]

mod common_setting;

use common_setting::{actor, admin, at, invalid_is, mem};
use fs_objects::v5::setting::{IndexChange, PlaceRow, Prim, ScopeRow, SettingRows, Writer};

const G: ScopeRow<'static> = ScopeRow::General;
const T: ScopeRow<'static> = ScopeRow::Program("tasks");
const P: PlaceRow = PlaceRow::Profile;

/// KA3: put/values of bool, int, text, `None` removes; other scope, place, profile do not see it; `rev` +1 per write.
#[tokio::test]
async fn ka3_values_round_trip_and_rev_rises_per_write() {
    let s = mem().await;
    let r = SettingRows::new(&s).await.unwrap();
    assert_eq!(r.rev().await.unwrap(), 0);
    let w = Writer::default();
    let mut rev = 0;
    for (k, v) in [
        ("a.flag", Prim::Bool(true)),
        ("a.count", Prim::Int(-7)),
        ("a.name", Prim::Text("[1,2] {x}".into())),
    ] {
        rev = r.put(0, G, P, k, Some(&v), w, at(5)).await.unwrap();
    }
    assert_eq!(rev, 3, "one rev per write");
    assert_eq!(r.rev().await.unwrap(), 3);
    let got = r.values(0, G, P).await.unwrap();
    assert_eq!(
        got,
        vec![
            ("a.count".to_owned(), Prim::Int(-7)),
            ("a.flag".to_owned(), Prim::Bool(true)),
            ("a.name".to_owned(), Prim::Text("[1,2] {x}".into())),
        ]
    );
    assert!(r.values(0, T, P).await.unwrap().is_empty(), "other scope");
    assert_eq!(r.values(0, G, PlaceRow::Device).await.unwrap().len(), 0);
    assert!(r.values(1, G, P).await.unwrap().is_empty(), "other profile");
    r.put(0, G, P, "a.flag", Some(&Prim::Bool(false)), w, at(6))
        .await
        .unwrap();
    assert_eq!(r.values(0, G, P).await.unwrap()[1].1, Prim::Bool(false));
    assert_eq!(r.put(0, G, P, "a.flag", None, w, at(7)).await.unwrap(), 5);
    assert_eq!(r.values(0, G, P).await.unwrap().len(), 2, "removed");
    let by = admin();
    r.put(
        0,
        T,
        PlaceRow::Device,
        "t.x",
        Some(&Prim::Int(1)),
        Writer {
            by: Some(&by),
            via: Some("tasks"),
        },
        at(8),
    )
    .await
    .unwrap();
    assert_eq!(r.values(0, T, PlaceRow::Device).await.unwrap().len(), 1);
}

async fn raw(s: &fs_objects::v5::Store, sql: &str) -> Result<(), fs_objects::v5::Error> {
    s.read().await.run(sql).await
}

/// KA4: the engine refuses what the `CHECK`s forbid (raw SQL, bypassing the typed functions).
#[tokio::test]
async fn ka4_checks_refuse_wrong_rows() {
    let s = mem().await;
    let cols = "profile, scope, program, place, key, kind, v_int, v_text, changed";
    for (name, vals) in [
        ("bool=2", "0,'general','','profile','k','bool',2,NULL,1"),
        (
            "text with v_int",
            "0,'general','','profile','k','text',1,'x',1",
        ),
        (
            "int with v_text",
            "0,'general','','profile','k','int',1,'x',1",
        ),
        (
            "general with program",
            "0,'general','x','profile','k','int',1,NULL,1",
        ),
        (
            "program with ''",
            "0,'program','','profile','k','int',1,NULL,1",
        ),
        (
            "unknown kind",
            "0,'general','','profile','k','real',1,NULL,1",
        ),
        ("unknown scope", "0,'other','','profile','k','int',1,NULL,1"),
    ] {
        let r = raw(
            &s,
            &format!("INSERT INTO settings_value ({cols}) VALUES ({vals})"),
        )
        .await;
        assert!(
            matches!(r, Err(fs_objects::v5::Error::Invalid(_))),
            "{name}: {r:?}"
        );
    }
    raw(&s, &format!("INSERT INTO settings_value ({cols}) VALUES (0,'program','tasks','device','k','bool',1,NULL,1)"))
        .await
        .unwrap();
}

/// KA5: a write on an unknown profile or on a tombstone is refused with `unknown`.
#[tokio::test]
async fn ka5_unknown_profile_and_tombstone_are_refused() {
    let s = mem().await;
    let r = SettingRows::new(&s).await.unwrap();
    let v = Prim::Int(1);
    let res = r
        .put(7, G, P, "k", Some(&v), Writer::default(), at(2))
        .await;
    assert!(
        invalid_is(&res, "settings_value.profile", "unknown"),
        "{res:?}"
    );
    let mk = IndexChange::Create {
        id: 1,
        name: "Work".into(),
        name_key: "work".into(),
    };
    r.change_index::<()>(at(3), &mut |_| Ok(vec![mk.clone()]))
        .await
        .unwrap()
        .unwrap();
    r.put(1, G, P, "k", Some(&v), Writer::default(), at(4))
        .await
        .unwrap();
    r.change_index::<()>(at(5), &mut |_| Ok(vec![IndexChange::Delete { id: 1 }]))
        .await
        .unwrap()
        .unwrap();
    let res = r
        .put(1, G, P, "k", Some(&v), Writer::default(), at(6))
        .await;
    assert!(
        invalid_is(&res, "settings_value.profile", "unknown"),
        "{res:?}"
    );
    let rev = r.rev().await.unwrap();
    assert!(r.clear_profile(1, G, at(7)).await.is_err());
    assert_eq!(r.rev().await.unwrap(), rev, "refused writes change nothing");
}

/// KA11: a key locked in the same file cannot be written as a person value.
#[tokio::test]
async fn ka11_put_on_a_locked_key_is_refused() {
    let s = mem().await;
    let r = SettingRows::new(&s).await.unwrap();
    let by = admin();
    r.lock(
        "fs.shell.language",
        &Prim::Text("de".into()),
        "policy",
        actor(&by),
        at(2),
    )
    .await
    .unwrap();
    let rev = r.rev().await.unwrap();
    for v in [Some(Prim::Text("en".into())), None] {
        let res = r
            .put(
                0,
                G,
                P,
                "fs.shell.language",
                v.as_ref(),
                Writer::default(),
                at(3),
            )
            .await;
        assert!(invalid_is(&res, "settings_value.key", "locked"), "{res:?}");
    }
    assert_eq!(r.rev().await.unwrap(), rev);
    assert_eq!(r.values(0, G, P).await.unwrap().len(), 0);
}

/// KA11 (all layers): a lock beats every layer (ADR 0100: one lock per key, over all scopes, programs, profiles, places).
#[tokio::test]
async fn ka11_a_lock_holds_in_every_scope_profile_and_place() {
    let s = mem().await;
    let r = SettingRows::new(&s).await.unwrap();
    let by = admin();
    r.change_index::<()>(at(2), &mut |_| {
        Ok(vec![IndexChange::Create {
            id: 1,
            name: "Work".into(),
            name_key: "work".into(),
        }])
    })
    .await
    .unwrap()
    .unwrap();
    r.lock(
        "fs.shell.language",
        &Prim::Text("de".into()),
        "policy",
        actor(&by),
        at(3),
    )
    .await
    .unwrap();
    let rev = r.rev().await.unwrap();
    let scopes = [G, ScopeRow::Program("fs-shell")];
    for profile in [0, 1] {
        for scope in scopes {
            for place in [PlaceRow::Profile, PlaceRow::Device] {
                for v in [Some(Prim::Text("en".into())), None] {
                    let res = r
                        .put(
                            profile,
                            scope,
                            place,
                            "fs.shell.language",
                            v.as_ref(),
                            Writer::default(),
                            at(4),
                        )
                        .await;
                    assert!(
                        invalid_is(&res, "settings_value.key", "locked"),
                        "{profile} {scope:?} {place:?} {v:?}: {res:?}"
                    );
                }
            }
        }
    }
    assert_eq!(r.rev().await.unwrap(), rev, "refused writes change nothing");
    for profile in [0, 1] {
        for scope in scopes {
            for place in [PlaceRow::Profile, PlaceRow::Device] {
                assert_eq!(r.values(profile, scope, place).await.unwrap().len(), 0);
            }
        }
    }
}

/// KA14: a future cancelled inside the write is rolled back at the next access: the rev is the same, no value.
#[tokio::test]
async fn ka14_a_cancelled_put_rolls_back() {
    let s = mem().await;
    let r = SettingRows::new(&s).await.unwrap();
    let v = Prim::Int(9);
    {
        let w = Writer::default();
        let f = r.put(0, G, P, "k", Some(&v), w, at(2));
        let mut f = std::pin::pin!(f);
        let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
        assert!(
            f.as_mut().poll(&mut cx).is_pending(),
            "the write stops before the COMMIT"
        );
    }
    assert_eq!(r.rev().await.unwrap(), 0, "rolled back");
    assert_eq!(r.values(0, G, P).await.unwrap().len(), 0);
    r.put(0, G, P, "k", Some(&v), Writer::default(), at(3))
        .await
        .unwrap();
    assert_eq!(r.rev().await.unwrap(), 1, "the next write works");
}
