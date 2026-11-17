//! Teil A, the table family and its registration: K-cases KA1, KA2, KA12, KA15, KA16.
#![cfg(feature = "v5")]
#![allow(clippy::many_single_char_names)]

mod common_setting;

use common_setting::{DOMAIN6, at, dir, file};
use fs_objects::v5::check::{Rule, typed};
use fs_objects::v5::setting::tables::{ALL, DOMAIN, STEPS};
use fs_objects::v5::setting::{PlaceRow, Prim, ScopeRow, SettingRows, Writer};
use fs_objects::v5::spec::Logical::Int;
use fs_objects::v5::{Col, Domain, Error, Step, Store, Table};

const TASKS: Table = Table {
    name: "tasks_t",
    cols: &[Col::req("id", Int)],
    constraints: &["PRIMARY KEY (id)"],
    indexes: &[],
};
const TASKS_DOMAIN: Domain = Domain::new("tasks", "t", &[TASKS], &[Step::Create(&[TASKS])]);

const CHANGED: [Step; 5] = [
    STEPS[0],
    Step::Sql(
        "CREATE TRIGGER settings_instance_log_no_update BEFORE UPDATE ON settings_instance_log BEGIN SELECT RAISE(ABORT, 'other'); END",
    ),
    STEPS[2],
    STEPS[3],
    STEPS[4],
];
const OTHER: Domain = Domain::new("settings", "x", &ALL, &CHANGED);

async fn rows(s: &Store, sql: &str) -> Vec<Vec<turso::Value>> {
    s.read().await.query(sql, Vec::new()).await.unwrap()
}

/// KA1: version 5, seven tables in `fs_column`; registering twice changes nothing.
#[tokio::test]
async fn ka1_domain_registers_once_at_version_5() {
    let s = Store::open(":memory:", 0).await.unwrap();
    s.register_domain(&DOMAIN, 1).await.unwrap();
    let applied = rows(
        &s,
        "SELECT applied FROM fs_domain WHERE prefix = 'settings'",
    )
    .await;
    s.register_domain(&DOMAIN, 99).await.unwrap();
    assert_eq!(
        rows(
            &s,
            "SELECT applied FROM fs_domain WHERE prefix = 'settings'"
        )
        .await,
        applied
    );
    assert_eq!(
        rows(
            &s,
            "SELECT version FROM fs_domain WHERE prefix = 'settings'"
        )
        .await,
        vec![vec![turso::Value::Integer(5)]]
    );
    assert_eq!(
        rows(
            &s,
            "SELECT DISTINCT tbl FROM fs_column WHERE domain = 'settings'"
        )
        .await
        .len(),
        ALL.len()
    );
    assert_eq!(
        rows(&s, "SELECT seq FROM fs_step WHERE domain = 'settings'")
            .await
            .len(),
        5
    );
    assert_eq!(
        rows(&s, "SELECT id FROM settings_profile").await.len(),
        1,
        "the base row"
    );
    assert_eq!(
        rows(&s, "SELECT rev FROM settings_rev").await,
        vec![vec![turso::Value::Integer(0)]]
    );
}

/// KA2: one domain per file, both orders.
#[tokio::test]
async fn ka2_a_second_domain_is_refused_in_both_orders() {
    let s = Store::open(":memory:", 0).await.unwrap();
    s.register_domain(&TASKS_DOMAIN, 1).await.unwrap();
    assert!(matches!(
        s.register_domain(&DOMAIN, 2).await,
        Err(Error::SecondDomain { .. })
    ));
    let s = Store::open(":memory:", 0).await.unwrap();
    s.register_domain(&DOMAIN, 1).await.unwrap();
    assert!(matches!(
        s.register_domain(&TASKS_DOMAIN, 2).await,
        Err(Error::SecondDomain { .. })
    ));
    assert!(SettingRows::new(&s).await.is_ok());
    let t = Store::open(":memory:", 0).await.unwrap();
    assert!(SettingRows::new(&t).await.is_err(), "not registered");
}

/// KA12: a store of 6 steps opened by a program of 5 is `Newer`; a changed step 2 is `StepMismatch`.
#[tokio::test]
async fn ka12_newer_store_and_changed_step() {
    let s = Store::open(":memory:", 0).await.unwrap();
    s.register_domain(&DOMAIN6, 1).await.unwrap();
    assert_eq!(
        s.register_domain(&DOMAIN, 2).await,
        Err(Error::Newer {
            domain: "settings".into(),
            found: 6,
            known: 5
        })
    );
    let s = Store::open(":memory:", 0).await.unwrap();
    s.register_domain(&DOMAIN, 1).await.unwrap();
    assert_eq!(
        s.register_domain(&OTHER, 2).await,
        Err(Error::StepMismatch {
            domain: "settings".into(),
            step: 2
        })
    );
}

/// KA15: `check::typed` over the domain: no JSON column, no finding at all (`settings_active.device` is exempt like the other own-store REF columns).
#[tokio::test]
async fn ka15_the_domain_passes_the_typed_check() {
    let s = common_setting::mem().await;
    let found = typed(&s).await.unwrap();
    let mine: Vec<_> = found
        .iter()
        .filter(|f| f.table.starts_with("settings_"))
        .collect();
    assert!(
        mine.iter().all(|f| f.rule != Rule::JsonInText),
        "no JSON column: {mine:?}"
    );
    assert!(mine.is_empty(), "{mine:?}");
}

/// KA16: a later column is a new `ALTER` step; the old rows stay as they were.
#[tokio::test]
async fn ka16_a_column_is_added_by_a_new_step() {
    let d = dir();
    let p = file(&d);
    let path = p.to_str().unwrap();
    {
        let s = SettingRows::open(path, at(1)).await.unwrap();
        let r = SettingRows::new(&s).await.unwrap();
        r.put(
            0,
            ScopeRow::General,
            PlaceRow::Profile,
            "k",
            Some(&Prim::Int(3)),
            Writer::default(),
            at(2),
        )
        .await
        .unwrap();
    }
    let s = Store::open(path, 3).await.unwrap();
    s.register_domain(&DOMAIN6, 4).await.unwrap();
    assert_eq!(
        rows(
            &s,
            "SELECT version FROM fs_domain WHERE prefix = 'settings'"
        )
        .await,
        vec![vec![turso::Value::Integer(6)]]
    );
    assert_eq!(
        rows(&s, "SELECT key, v_int, note FROM settings_value").await,
        vec![vec![
            turso::Value::Text("k".into()),
            turso::Value::Integer(3),
            turso::Value::Null
        ]]
    );
}
