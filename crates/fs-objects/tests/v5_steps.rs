//! Steps are applied once, roll back as a whole, are fingerprinted, and the real tables are checked (ADR 0086).
#![cfg(feature = "v5")]

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use fs_objects::v5::migrate::StepFuture;
use fs_objects::v5::spec::Logical::{Blob, Ref, Text};
use fs_objects::v5::{Col, Conn, Domain, Error, Step, Store, Table, schema};
use turso::Value;

const ITEM: Table = Table {
    name: "demo_item",
    cols: &[Col::req("id", Ref), Col::req("name", Text)],
    constraints: &["PRIMARY KEY (id)"],
    indexes: &[],
};
const COPY: Table = Table {
    name: "demo_copy",
    cols: &[Col::req("id", Ref)],
    constraints: &["PRIMARY KEY (id)"],
    indexes: &[],
};
const ID: &str = "01927f00-0000-7000-8000-000000000001";
static RUNS: AtomicUsize = AtomicUsize::new(0);

fn counted<'a>(c: &'a Conn<'_>) -> StepFuture<'a> {
    Box::pin(async move {
        RUNS.fetch_add(1, Ordering::SeqCst);
        c.run(&COPY.ddl()).await?;
        c.run(&format!("INSERT INTO demo_copy (id) VALUES ('{ID}')"))
            .await
    })
}

fn broken<'a>(c: &'a Conn<'_>) -> StepFuture<'a> {
    Box::pin(async move {
        c.run(&COPY.ddl()).await?;
        c.run(&format!("INSERT INTO demo_copy (id) VALUES ('{ID}')"))
            .await?;
        Err(Error::Unavailable(
            "the step failed after its writes".into(),
        ))
    })
}

const STEPS: [Step; 2] = [Step::Create(&[ITEM]), Step::Code("counted", counted)];
const DOMAIN: Domain = Domain {
    prefix: "demo",
    about: "demo",
    tables: &[ITEM, COPY],
    channels: &[],
    steps: &STEPS,
};

async fn count(s: &Store, sql: &str) -> usize {
    s.read().await.query(sql, Vec::new()).await.unwrap().len()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_code_step_runs_once_however_many_register_at_the_same_time() {
    let s = Arc::new(Store::open(":memory:", 0).await.unwrap());
    let tasks: Vec<_> = (0..6)
        .map(|n| {
            let s = s.clone();
            tokio::spawn(async move { s.register_domain(&DOMAIN, n).await })
        })
        .collect();
    for t in tasks {
        t.await.unwrap().unwrap();
    }
    s.register_domain(&DOMAIN, 9).await.unwrap();
    assert_eq!(RUNS.load(Ordering::SeqCst), 1, "applied once");
    assert_eq!(count(&s, "SELECT 1 FROM demo_copy").await, 1);
    assert_eq!(
        count(&s, "SELECT 1 FROM fs_step WHERE domain = 'demo'").await,
        2
    );
}

#[tokio::test]
async fn a_code_step_that_fails_after_ddl_and_insert_leaves_nothing_behind() {
    let s = Store::open(":memory:", 0).await.unwrap();
    let steps: &'static [Step] = &[Step::Create(&[ITEM]), Step::Code("broken", broken)];
    let d = Domain { steps, ..DOMAIN };
    assert!(matches!(
        s.register_domain(&d, 1).await,
        Err(Error::Schema(_))
    ));
    assert_eq!(
        count(&s, "SELECT 1 FROM sqlite_master WHERE name = 'demo_copy'").await,
        0,
        "the table is gone"
    );
    assert_eq!(
        count(&s, "SELECT 1 FROM sqlite_master WHERE name = 'demo_item'").await,
        1,
        "step 1 stays"
    );
    let v = s
        .read()
        .await
        .query(
            "SELECT version FROM fs_domain WHERE prefix = 'demo'",
            Vec::new(),
        )
        .await
        .unwrap();
    assert_eq!(v[0][0], Value::Integer(1));
    assert_eq!(
        count(&s, "SELECT 1 FROM fs_step WHERE domain = 'demo'").await,
        1
    );
}

#[tokio::test]
async fn reordered_inserted_or_changed_steps_are_found() {
    let s = Store::open(":memory:", 0).await.unwrap();
    let sql: &'static [Step] = &[
        Step::Create(&[ITEM]),
        Step::Sql("DELETE FROM meta WHERE key = \'k1\'"),
        Step::Sql("DELETE FROM meta WHERE key = \'k2\'"),
    ];
    let d = Domain {
        tables: &[ITEM],
        steps: sql,
        ..DOMAIN
    };
    s.register_domain(&d, 1).await.unwrap();
    for (label, steps) in [
        ("reordered", vec![sql[0], sql[2], sql[1]]),
        (
            "changed",
            vec![
                sql[0],
                sql[1],
                Step::Sql("DELETE FROM meta WHERE key = \'k3\'"),
            ],
        ),
        (
            "inserted",
            vec![
                sql[0],
                Step::Sql("DELETE FROM meta WHERE key = \'k0\'"),
                sql[1],
            ],
        ),
    ] {
        let d = Domain {
            tables: &[ITEM],
            steps: Box::leak(steps.into_boxed_slice()),
            ..DOMAIN
        };
        let r = s.register_domain(&d, 2).await;
        assert!(
            matches!(r, Err(Error::StepMismatch { .. })),
            "{label}: {r:?}"
        );
    }
    s.register_domain(&d, 3).await.unwrap();
    let more: &'static [Step] = Box::leak(
        vec![
            sql[0],
            sql[1],
            sql[2],
            Step::Sql("DELETE FROM meta WHERE key = \'k4\'"),
        ]
        .into_boxed_slice(),
    );
    s.register_domain(&Domain { steps: more, ..d }, 4)
        .await
        .unwrap();
}

const BLOBBY: Table = Table {
    name: "demo_item",
    cols: &[Col::req("id", Ref), Col::req("raw", Blob)],
    constraints: &["PRIMARY KEY (id)"],
    indexes: &[],
};

#[tokio::test]
async fn a_blob_or_an_undeclared_column_makes_the_registration_red() {
    let s = Store::open(":memory:", 0).await.unwrap();
    let steps: &'static [Step] = &[Step::Create(&[BLOBBY])];
    let r = s
        .register_domain(
            &Domain {
                tables: &[BLOBBY],
                steps,
                ..DOMAIN
            },
            1,
        )
        .await;
    assert!(
        matches!(&r, Err(Error::Invalid(p)) if p[0].field == "demo_item.raw"),
        "{r:?}"
    );
    let s = Store::open(":memory:", 0).await.unwrap();
    let steps: &'static [Step] = &[
        Step::Create(&[ITEM]),
        Step::Sql("ALTER TABLE demo_item ADD COLUMN raw BLOB"),
    ];
    let r = s
        .register_domain(
            &Domain {
                tables: &[ITEM],
                steps,
                ..DOMAIN
            },
            1,
        )
        .await;
    assert!(
        matches!(r, Err(Error::Invalid(_))),
        "a column added by ALTER and not declared: {r:?}"
    );
}

fn json_into<'a>(c: &'a Conn<'_>) -> StepFuture<'a> {
    Box::pin(async move {
        c.run("INSERT INTO demo_log (id, data) VALUES ('01927f00-0000-7000-8000-000000000002', '{\"a\": [1, 2]}')").await
    })
}

const LOG: Table = Table {
    name: "demo_log",
    cols: &[Col::req("id", Ref), Col::req("data", Text)],
    constraints: &["PRIMARY KEY (id)"],
    indexes: &[],
};

#[tokio::test]
async fn json_in_a_text_column_is_found_by_content() {
    let s = Store::open(":memory:", 0).await.unwrap();
    let steps: &'static [Step] = &[Step::Create(&[LOG]), Step::Code("json_into", json_into)];
    let r = s
        .register_domain(
            &Domain {
                tables: &[LOG],
                steps,
                ..DOMAIN
            },
            1,
        )
        .await;
    assert!(
        matches!(&r, Err(Error::Invalid(p)) if p[0].field == "demo_log.data"),
        "{r:?}"
    );
    assert_eq!(
        count(&s, "SELECT 1 FROM demo_log").await,
        0,
        "the step rolled back, step 1 stays"
    );

    let ok: &'static [Step] = &[Step::Create(&[LOG])];
    s.register_domain(
        &Domain {
            tables: &[LOG],
            steps: ok,
            ..DOMAIN
        },
        2,
    )
    .await
    .unwrap();
    let c = s.begin().await.unwrap();
    c.run("INSERT INTO demo_log (id, data) VALUES ('01927f00-0000-7000-8000-000000000003', '[\"x\"]')").await.unwrap();
    c.run("INSERT INTO demo_log (id, data) VALUES ('01927f00-0000-7000-8000-000000000004', '[Draft] a note')").await.unwrap();
    assert!(
        schema::json_in_text(&c, &[LOG]).await.is_err(),
        "an array that parses is JSON; '[Draft] a note' alone is not"
    );
    c.finish(Err::<(), _>(Error::Unavailable("undo".into())))
        .await
        .unwrap_err();
    let c = s.begin().await.unwrap();
    c.run("INSERT INTO demo_log (id, data) VALUES ('01927f00-0000-7000-8000-000000000004', '[Draft] a note')").await.unwrap();
    schema::json_in_text(&c, &[LOG]).await.unwrap();
    c.finish(Ok(())).await.unwrap();
}
