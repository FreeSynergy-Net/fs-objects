//! `check::typed` against the real catalogue: what it accepts and what it finds.
#![cfg(feature = "v5")]

mod common_v5;

use common_v5::store;
use fs_objects::v5::check::{Rule, typed};
use fs_objects::v5::codec::id_value;
use fs_objects::v5::log::{FieldChange, Kind, log};
use fs_objects::v5::log_codec::LogValue;
use fs_objects::v5::{Store, new_id};

const T0: i64 = 1_794_823_200_000;

async fn rules(s: &Store) -> Vec<(Rule, String, Option<String>)> {
    typed(s)
        .await
        .unwrap()
        .into_iter()
        .map(|f| (f.rule, f.table, f.column))
        .collect()
}

async fn run(s: &Store, sql: &str) {
    s.read().await.run(sql).await.unwrap();
}

#[tokio::test]
async fn a_clean_store_has_no_findings() {
    let s = store(&[]).await;
    let found = rules(&s).await;
    // The demo domain has one REF column without a foreign key (demo_cut.note): the only finding.
    assert_eq!(
        found,
        vec![(
            Rule::MissingForeignKey,
            "demo_cut".into(),
            Some("note".into())
        )]
    );
}

#[tokio::test]
async fn json_in_a_text_column_is_found() {
    let s = store(&[]).await;
    let (id, o) = (new_id(T0).unwrap(), new_id(T0).unwrap());
    let _ = (id, o);
    run(&s, "INSERT INTO objects (id, type, name, name_sort, published, updated, rev) VALUES ('00000000-0000-7000-8000-000000000001','n','x','x',0,0,1)").await;
    run(&s, "INSERT INTO demo_note (id, object, body) VALUES ('00000000-0000-7000-8000-000000000002','00000000-0000-7000-8000-000000000001','{\"a\":1}')").await;
    assert!(
        rules(&s)
            .await
            .contains(&(Rule::JsonInText, "demo_note".into(), Some("body".into())))
    );
    // A text that only looks like JSON is no finding.
    run(&s, "UPDATE demo_note SET body = '[not json'").await;
    assert!(!rules(&s).await.iter().any(|f| f.0 == Rule::JsonInText));
}

#[tokio::test]
async fn a_table_that_is_not_strict_or_not_registered_is_found() {
    let s = store(&[]).await;
    run(&s, "CREATE TABLE loose (a INTEGER)").await;
    let found = rules(&s).await;
    assert!(found.contains(&(Rule::NotStrict, "loose".into(), None)));
    assert!(found.contains(&(Rule::Unregistered, "loose".into(), None)));
}

#[tokio::test]
async fn blobs_outside_the_exceptions_are_found() {
    let s = store(&[]).await;
    run(
        &s,
        "CREATE TABLE demo_raw (cipher BLOB, nonce BLOB, loro BLOB) STRICT",
    )
    .await;
    let found = rules(&s).await;
    assert!(found.contains(&(
        Rule::BlobNotAllowed,
        "demo_raw".into(),
        Some("nonce".into())
    )));
    assert!(found.contains(&(Rule::BlobNotAllowed, "demo_raw".into(), Some("loro".into()))));
    assert!(!found.contains(&(
        Rule::BlobNotAllowed,
        "demo_raw".into(),
        Some("cipher".into())
    )));
}

#[tokio::test]
async fn a_time_column_that_is_not_integer_is_found() {
    let s = store(&[]).await;
    run(
        &s,
        "UPDATE fs_column SET logical = 'utc' WHERE tbl = 'demo_note' AND col = 'body'",
    )
    .await;
    assert!(rules(&s).await.contains(&(
        Rule::TimeNotInteger,
        "demo_note".into(),
        Some("body".into())
    )));
}

#[tokio::test]
async fn an_object_value_that_is_a_blob_is_found() {
    let s = store(&[]).await;
    let id = new_id(T0).unwrap();
    run(&s, &format!("INSERT INTO objects (id, type, name, name_sort, published, updated, rev) VALUES ('{}','n','x','x',0,0,1)", fs_objects::v5::codec::id_text(id))).await;
    let c = s.begin().await.unwrap();
    let who = common_v5::actor();
    let change = FieldChange {
        field: "link".into(),
        row_key: None,
        old: None,
        new: Some(LogValue::Object(id)),
    };
    let r = log(&c, id, Kind::Update, &who, T0, &[change]).await;
    c.finish(r).await.unwrap();
    assert!(
        !rules(&s).await.iter().any(|f| f.0 == Rule::ObjectBlob),
        "an object id is fine"
    );
    run(
        &s,
        "UPDATE activity_change SET new_text = 'x{\"a\":1}' WHERE value_kind = 'object'",
    )
    .await;
    assert!(rules(&s).await.contains(&(
        Rule::ObjectBlob,
        "activity_change".into(),
        Some("new_text".into())
    )));
    let _ = id_value(id);
}

#[tokio::test]
async fn a_column_of_the_type_any_is_a_finding() {
    let s = store(&[]).await;
    run(&s, "CREATE TABLE demo_loose (id TEXT NOT NULL, v ANY, PRIMARY KEY (id)) STRICT").await;
    assert!(
        rules(&s)
            .await
            .contains(&(Rule::AnyType, "demo_loose".into(), Some("v".into())))
    );
}

#[tokio::test]
async fn core_free_text_that_looks_like_json_is_no_finding() {
    let s = store(&[]).await;
    run(&s, "INSERT INTO objects (id, type, name, name_sort, published, updated, rev) VALUES ('00000000-0000-7000-8000-000000000001','n','[1,2]','x',0,0,1)").await;
    run(&s, "INSERT INTO objects (id, type, name, name_sort, published, updated, rev) VALUES ('00000000-0000-7000-8000-000000000002','n','{}','x',0,0,1)").await;
    assert!(!rules(&s).await.iter().any(|f| f.0 == Rule::JsonInText));
}
