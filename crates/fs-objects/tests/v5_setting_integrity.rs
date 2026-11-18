//! Teil A, `Store::inspect`: a file whose registry and steps are valid but whose `PRAGMA integrity_check` fails is
//! `Broken { cause: "integrity" }` (K2-Y5: the result of the check must count; every other test passes without it).
#![cfg(feature = "v5")]

mod common_setting;

use common_setting::{at, dir, file};
use fs_objects::v5::Inspect::{Broken, Fits};
use fs_objects::v5::Store;
use fs_objects::v5::setting::tables::DOMAIN;
use fs_objects::v5::setting::{PlaceRow, Prim, ScopeRow, SettingRows, Writer};
use turso::Value;

/// The root page of the index `settings_value_scope` (1-based) and the page size of a checkpointed copy of `p`.
async fn index_page(p: &str) -> (usize, usize) {
    let db = turso::Builder::new_local(p).build().await.unwrap();
    let c = db.connect().unwrap();
    // Move the write-ahead log into the main file so that the pages are in the file we change.
    let mut ck = c
        .query("PRAGMA wal_checkpoint(TRUNCATE)", ())
        .await
        .unwrap();
    while ck.next().await.unwrap().is_some() {}
    let mut r = c
        .query(
            "SELECT rootpage FROM sqlite_schema WHERE name = 'settings_value_scope'",
            (),
        )
        .await
        .unwrap();
    let page = match r.next().await.unwrap().unwrap().get_value(0).unwrap() {
        Value::Integer(n) => usize::try_from(n).unwrap(),
        v => panic!("rootpage is {v:?}"),
    };
    drop(r);
    drop(ck);
    let head = std::fs::read(p).unwrap();
    (page, usize::from(u16::from_be_bytes([head[16], head[17]])))
}

/// KA13 (integrity): the index of the values is emptied inside its page: the registry and the steps still read, only the
/// integrity check of the engine sees the damage. Before the damage the same file `Fits`.
#[tokio::test]
async fn ka13_a_failing_integrity_check_is_broken_integrity() {
    let d = dir();
    let p = file(&d);
    let name = p.to_str().unwrap();
    {
        let s = SettingRows::open(name, at(1)).await.unwrap();
        let r = SettingRows::new(&s).await.unwrap();
        for i in 0..40 {
            let k = format!("k.{i}");
            let w = Writer::default();
            let v = Prim::Int(i);
            r.put(
                0,
                ScopeRow::Program("t"),
                PlaceRow::Profile,
                &k,
                Some(&v),
                w,
                at(5),
            )
            .await
            .unwrap();
        }
    }
    let (page, size) = index_page(name).await;
    assert_eq!(
        Store::inspect(name, &DOMAIN).await.unwrap(),
        Fits { found: 5 }
    );
    let mut bytes = std::fs::read(&p).unwrap();
    let from = (page - 1) * size + 8; // keep the page header, empty the cells
    bytes[from..from + size - 8].fill(0);
    assert!(
        !d.path().join("settings.db-wal").exists()
            || std::fs::metadata(d.path().join("settings.db-wal"))
                .unwrap()
                .len()
                == 0
    );
    std::fs::write(&p, &bytes).unwrap();
    assert_eq!(
        Store::inspect(name, &DOMAIN).await.unwrap(),
        Broken { cause: "integrity" }
    );
}
