//! Teil A, batch and program list: K-cases KA18 to KA21.
#![cfg(feature = "v5")]
#![allow(clippy::many_single_char_names)]

mod common_setting;

use common_setting::{actor, admin, at, invalid_is, mem};
use fs_objects::v5::setting::{
    IndexChange, Outcome, PlaceRow, Prim, ScopeRow, SettingOp, SettingRows, Writer,
};

const G: ScopeRow<'static> = ScopeRow::General;

fn put(profile: i64, key: &'static str, n: i64) -> SettingOp<'static> {
    SettingOp::Put {
        profile,
        scope: G,
        place: PlaceRow::Profile,
        key,
        v: Some(Prim::Int(n)),
        w: Writer::default(),
    }
}

/// KA18: an error in the middle leaves everything as it was: values, index, instance rows, log, rev.
#[tokio::test]
async fn ka18_a_failing_batch_changes_nothing() {
    let s = mem().await;
    let r = SettingRows::new(&s).await.unwrap();
    let by = admin();
    let ops = vec![
        put(0, "a", 1),
        SettingOp::Index(IndexChange::Create {
            id: 1,
            name: "Work".into(),
            name_key: "work".into(),
        }),
        put(1, "b", 2),
        SettingOp::PutInstance {
            key: "i",
            v: Some(Prim::Int(3)),
        },
        SettingOp::Lock {
            key: "l",
            v: Prim::Int(4),
            reason: "why",
        },
        put(7, "c", 5),
        put(0, "d", 6),
    ];
    let res = r.apply(&ops, Some(actor(&by)), at(5)).await;
    assert!(
        invalid_is(&res, "settings_value.profile", "unknown"),
        "{res:?}"
    );
    assert_eq!(r.rev().await.unwrap(), 0);
    assert_eq!(r.values(0, G, PlaceRow::Profile).await.unwrap().len(), 0);
    assert_eq!(r.index().await.unwrap().profiles.len(), 1);
    assert_eq!(r.instance().await.unwrap().values.len(), 0);
    assert_eq!(r.log(None, 10).await.unwrap().len(), 0);
    // A later batch works (the connection is clean).
    assert_eq!(
        r.apply(&ops[..1], None, at(6)).await.unwrap(),
        Outcome::Changed { rev: 1 }
    );
}

/// KA19: a good batch is one transaction: one `rev + 1`, log rows inside, and a batch of no-ops is `Same`.
#[tokio::test]
async fn ka19_a_good_batch_is_one_write() {
    let s = mem().await;
    let r = SettingRows::new(&s).await.unwrap();
    let by = admin();
    let a = actor(&by);
    let ops = vec![
        put(0, "a", 1),
        SettingOp::Index(IndexChange::Create {
            id: 1,
            name: "Work".into(),
            name_key: "work".into(),
        }),
        put(1, "b", 2),
        SettingOp::PutInstance {
            key: "i",
            v: Some(Prim::Int(3)),
        },
        SettingOp::Lock {
            key: "l",
            v: Prim::Int(4),
            reason: "why",
        },
        SettingOp::Unlock {
            key: "l",
            reason: "done",
        },
    ];
    assert_eq!(
        r.apply(&ops, Some(a), at(5)).await.unwrap(),
        Outcome::Changed { rev: 1 }
    );
    assert_eq!(r.values(1, G, PlaceRow::Profile).await.unwrap().len(), 1);
    assert_eq!(r.instance().await.unwrap().values.len(), 2);
    let log = r.log(None, 10).await.unwrap();
    assert_eq!(
        log.iter().map(|l| l.action).collect::<Vec<_>>(),
        ["set", "lock", "unlock"]
    );
    let noop = [SettingOp::PutInstance {
        key: "i",
        v: Some(Prim::Int(3)),
    }];
    assert_eq!(
        r.apply(&noop, Some(a), at(6)).await.unwrap(),
        Outcome::Same { rev: 1 }
    );
    assert_eq!(
        r.apply(&[], None, at(6)).await.unwrap(),
        Outcome::Same { rev: 1 }
    );
}

/// KA18 (reason): a batch with a lock or unlock whose reason is only blanks is refused with `reason-missing`, as the single
/// calls do, and writes NOTHING (not even the operations before it).
#[tokio::test]
async fn ka18_a_blank_reason_in_a_batch_is_refused_and_writes_nothing() {
    let s = mem().await;
    let r = SettingRows::new(&s).await.unwrap();
    let by = admin();
    let a = actor(&by);
    r.lock("k", &Prim::Int(1), "policy", a, at(2))
        .await
        .unwrap();
    let rev = r.rev().await.unwrap();
    let logs = r.log(None, 10).await.unwrap().len();
    for bad in ["  ", "", "\t\n"] {
        let lock = SettingOp::Lock {
            key: "other",
            v: Prim::Int(2),
            reason: bad,
        };
        let unlock = SettingOp::Unlock {
            key: "k",
            reason: bad,
        };
        let unlock_free = SettingOp::Unlock {
            key: "never-locked",
            reason: bad,
        };
        for op in [lock, unlock, unlock_free] {
            let res = r.apply(&[put(0, "a", 1), op], Some(a), at(3)).await;
            assert!(
                invalid_is(&res, "settings_instance_lock.reason", "reason-missing"),
                "{bad:?}: {res:?}"
            );
            assert_eq!(r.rev().await.unwrap(), rev);
            assert!(r.values(0, G, PlaceRow::Profile).await.unwrap().is_empty());
            let i = r.instance().await.unwrap();
            assert_eq!(i.locks.len(), 1, "the lock on k stays, no lock on other");
            assert_eq!(r.log(None, 10).await.unwrap().len(), logs);
        }
    }
}

/// KA20: an instance operation without an actor is refused, nothing written.
#[tokio::test]
async fn ka20_instance_operation_needs_an_actor() {
    let s = mem().await;
    let r = SettingRows::new(&s).await.unwrap();
    let ops = [
        put(0, "a", 1),
        SettingOp::PutInstance {
            key: "i",
            v: Some(Prim::Int(3)),
        },
    ];
    let res = r.apply(&ops, None, at(2)).await;
    assert!(
        invalid_is(&res, "settings_instance_log.by", "actor-missing"),
        "{res:?}"
    );
    assert_eq!(r.rev().await.unwrap(), 0);
}

/// KA21: `programs` lists the programs that have values, sorted; general values do not count.
#[tokio::test]
async fn ka21_programs_lists_programs_with_values() {
    let s = mem().await;
    let r = SettingRows::new(&s).await.unwrap();
    assert_eq!(r.programs().await.unwrap().len(), 0);
    let v = Prim::Bool(true);
    let w = Writer::default();
    for p in ["tasks", "people", "tasks"] {
        r.put(
            0,
            ScopeRow::Program(p),
            PlaceRow::Device,
            "k",
            Some(&v),
            w,
            at(2),
        )
        .await
        .unwrap();
    }
    r.put(0, G, PlaceRow::Profile, "g", Some(&v), w, at(3))
        .await
        .unwrap();
    assert_eq!(r.programs().await.unwrap(), ["people", "tasks"]);
}
