//! Teil A, the instance layer: K-cases KA9, KA10.
#![cfg(feature = "v5")]
#![allow(clippy::many_single_char_names)]

mod common_setting;

use common_setting::{actor, admin, at, invalid_is, mem};
use fs_objects::v5::setting::{Actor, Outcome, Prim, SettingRows};

const K: &str = "fs.shell.language";

fn de() -> Prim {
    Prim::Text("de".into())
}

/// KA9: put/lock/unlock are idempotent and need reasons; clearing a locked value fails.
#[tokio::test]
async fn ka9_instance_writes_locks_and_reasons() {
    let s = mem().await;
    let r = SettingRows::new(&s).await.unwrap();
    let by = admin();
    let a = actor(&by);
    let o = r.put_instance(K, Some(&de()), a, at(10)).await.unwrap();
    assert_eq!(o, Outcome::Changed { rev: 1 });
    let o = r.put_instance(K, Some(&de()), a, at(11)).await.unwrap();
    assert_eq!(o, Outcome::Same { rev: 1 }, "the same value: no rev");
    assert_eq!(r.log(None, 10).await.unwrap().len(), 1, "one log row");
    let o = r.lock(K, &de(), "policy", a, at(12)).await.unwrap();
    assert_eq!(o, Outcome::Changed { rev: 2 });
    let i = r.instance().await.unwrap();
    assert_eq!(i.values, vec![(K.to_owned(), de())]);
    assert_eq!(
        (i.locks.len(), i.locks[0].reason.as_str(), i.locks[0].at),
        (1, "policy", at(12))
    );
    assert_eq!(
        r.log(None, 10).await.unwrap().len(),
        2,
        "value + lock = one more log row"
    );
    assert_eq!(
        r.lock(K, &de(), "policy", a, at(13)).await.unwrap(),
        Outcome::Same { rev: 2 }
    );
    let res = r
        .lock(K, &Prim::Text("en".into()), "policy", a, at(14))
        .await;
    assert!(
        invalid_is(&res, "settings_instance_lock.key", "locked"),
        "{res:?}"
    );
    let res = r.lock(K, &de(), "other reason", a, at(14)).await;
    assert!(
        invalid_is(&res, "settings_instance_lock.key", "locked"),
        "{res:?}"
    );
    let res = r.put_instance(K, None, a, at(15)).await;
    assert!(
        invalid_is(&res, "settings_instance_lock.key", "locked"),
        "clear: {res:?}"
    );
    let res = r
        .put_instance(K, Some(&Prim::Text("en".into())), a, at(15))
        .await;
    assert!(
        invalid_is(&res, "settings_instance_lock.key", "locked"),
        "set: {res:?}"
    );
    for bad in ["", "  ", "\t\n"] {
        let res = r.unlock(K, bad, a, at(16)).await;
        assert!(
            invalid_is(&res, "settings_instance_lock.reason", "reason-missing"),
            "{res:?}"
        );
        let res = r.lock("other", &de(), bad, a, at(16)).await;
        assert!(
            invalid_is(&res, "settings_instance_lock.reason", "reason-missing"),
            "{res:?}"
        );
    }
    assert_eq!(r.instance().await.unwrap().locks.len(), 1, "still locked");
    assert_eq!(r.rev().await.unwrap(), 2, "refused calls changed nothing");
    let o = r.unlock(K, "maintenance over", a, at(17)).await.unwrap();
    assert_eq!(o, Outcome::Changed { rev: 3 });
    let i = r.instance().await.unwrap();
    assert!(
        (i.locks.is_empty(), i.values.len()) == (true, 1),
        "the value stays as default"
    );
    assert_eq!(
        r.unlock(K, "again", a, at(18)).await.unwrap(),
        Outcome::Same { rev: 3 }
    );
    let log = r.log(None, 10).await.unwrap();
    let last = log.last().unwrap();
    assert_eq!(
        (
            last.action,
            last.reason.as_deref(),
            last.old.clone(),
            last.new.clone()
        ),
        ("unlock", Some("maintenance over"), Some(de()), Some(de()))
    );
    assert_eq!(
        r.put_instance(K, None, a, at(19)).await.unwrap(),
        Outcome::Changed { rev: 4 }
    );
    assert_eq!(
        r.put_instance(K, None, a, at(20)).await.unwrap(),
        Outcome::Same { rev: 4 }
    );
}

/// KA9 (raw): the engine's `CHECK` on the lock reason (`trim`), as a second line of defence.
#[tokio::test]
async fn ka9_raw_lock_with_a_blank_reason_is_refused_by_the_engine() {
    let s = mem().await;
    let r = SettingRows::new(&s).await.unwrap();
    let by = admin();
    r.put_instance(K, Some(&de()), actor(&by), at(2))
        .await
        .unwrap();
    let res = s
        .read()
        .await
        .run("INSERT INTO settings_instance_lock (key, reason, locked_by, locked_at) VALUES ('fs.shell.language', '  ', 'https://example.org/a', 3)")
        .await;
    assert!(
        matches!(res, Err(fs_objects::v5::Error::Invalid(_))),
        "{res:?}"
    );
    let res = s
        .read()
        .await
        .run("INSERT INTO settings_instance_lock (key, reason, locked_by, locked_at) VALUES ('nope', 'x', 'https://example.org/a', 3)")
        .await;
    assert!(
        res.is_err(),
        "a lock never exists without its value (FK): {res:?}"
    );
}

/// KA10: the log is append-only (triggers), ordered by time, paged without gaps; `os_user` and `by` are stored.
#[tokio::test]
async fn ka10_the_log_is_append_only_and_pages_without_gaps() {
    let s = mem().await;
    let r = SettingRows::new(&s).await.unwrap();
    let by = admin();
    let a = Actor {
        by: &by,
        os_user: Some("root"),
    };
    for (n, v) in (0..7).zip(1_i64..) {
        r.put_instance(&format!("k{n}"), Some(&Prim::Int(v)), a, at(100 + n))
            .await
            .unwrap();
    }
    let none = Actor {
        by: &by,
        os_user: None,
    };
    r.put_instance("k0", None, none, at(200)).await.unwrap();
    for sql in [
        "UPDATE settings_instance_log SET key = 'x'",
        "DELETE FROM settings_instance_log",
    ] {
        let res = s.read().await.run(sql).await;
        assert!(res.is_err(), "{sql}: {res:?}");
    }
    let all = r.log(None, 100).await.unwrap();
    assert_eq!(all.len(), 8);
    assert!(
        all.windows(2)
            .all(|w| w[0].at <= w[1].at && w[0].id < w[1].id),
        "time order"
    );
    assert_eq!((all[0].os_user.as_deref(), &all[0].by), (Some("root"), &by));
    assert_eq!(
        (all[7].action, all[7].os_user.clone(), all[7].old.clone()),
        ("clear", None, Some(Prim::Int(1)))
    );
    let mut paged = Vec::new();
    let mut after = None;
    loop {
        let page = r.log(after, 3).await.unwrap();
        if page.is_empty() {
            break;
        }
        after = page.last().map(|x| x.id);
        paged.extend(page);
    }
    assert_eq!(paged, all, "no gap, no repeat");
}
