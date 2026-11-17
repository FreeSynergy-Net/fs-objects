//! Migration steps: ordered, forward only, each transactional; after every step the schema keeps ADR 0086
//! (`STRICT`, foreign keys on parent columns, no JSON column, a blob only as `object_original.bytes`).
#![cfg(feature = "v5")]
#![allow(clippy::many_single_char_names)]

use fs_objects::v5::migrate::StepFuture;
use fs_objects::v5::spec::Logical::{Ref, Text, Utc};
use fs_objects::v5::{Col, Conn, Domain, Error, Step, Store, Table};
use turso::Value;

const ITEM_V1: Table = Table {
    name: "demo_item",
    cols: &[Col::req("id", Ref), Col::req("name", Text)],
    constraints: &["PRIMARY KEY (id)"],
    indexes: &[],
};
const ITEM: Table = Table {
    name: "demo_item",
    cols: &[
        Col::req("id", Ref),
        Col::req("name", Text),
        Col::opt("note", Text),
    ],
    constraints: &["PRIMARY KEY (id)"],
    indexes: &[],
};
const PART: Table = Table {
    name: "demo_part",
    cols: &[
        Col::req("id", Ref),
        Col::req("item", Ref),
        Col::req("at", Utc),
    ],
    constraints: &[
        "PRIMARY KEY (id)",
        "FOREIGN KEY (item) REFERENCES demo_item(id) ON DELETE CASCADE",
    ],
    indexes: &[],
};

fn create_part<'a>(c: &'a Conn<'_>) -> StepFuture<'a> {
    Box::pin(async move {
        c.run(&PART.ddl()).await?;
        c.run("INSERT INTO demo_part (id, item, at) SELECT id, id, 0 FROM demo_item")
            .await
    })
}

const STEPS: [Step; 3] = [
    Step::Create(&[ITEM_V1]),
    Step::Sql("ALTER TABLE demo_item ADD COLUMN note TEXT"),
    Step::Code("create_part", create_part),
];

fn domain(n: usize) -> Domain {
    let tables: &'static [Table] = match n {
        1 => &[ITEM_V1],
        2 => &[ITEM],
        _ => &[ITEM, PART],
    };
    Domain::new("demo", "demo", tables, &STEPS[..n])
}

/// The rules of ADR 0086 over every table the engine knows.
async fn adr_0086(s: &Store) {
    let c = s.read().await;
    let tables = c
        .query(
            "SELECT name, sql FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%'",
            Vec::new(),
        )
        .await
        .unwrap();
    assert!(tables.len() >= 11, "own tables and the domain");
    let text = |v: &Value| {
        if let Value::Text(t) = v {
            t.clone()
        } else {
            String::new()
        }
    };
    let no_fk_by_design = [
        "_abzug",
        "context",
        "in_reply_to",
        "describes",
        "target",
        "object_id",
    ];
    for t in &tables {
        let (name, sql) = (text(&t[0]), text(&t[1]));
        assert!(
            sql.trim_end().to_uppercase().ends_with("STRICT"),
            "{name} is STRICT: {sql}"
        );
        assert!(
            !sql.to_uppercase().contains("JSON"),
            "{name} has no JSON column"
        );
        let cols = c
            .query(
                "SELECT col, logical FROM fs_column WHERE tbl = ?",
                vec![Value::Text(name.clone())],
            )
            .await
            .unwrap();
        for col in &cols {
            let (col, logical) = (text(&col[0]), text(&col[1]));
            assert!(
                logical != "blob" || (name == "object_original" && col == "bytes"),
                "{name}.{col} is a blob"
            );
            let parent =
                logical == "ref" && col != "id" && !no_fk_by_design.iter().any(|w| col.contains(w));
            if parent {
                assert!(
                    sql.contains(&format!("FOREIGN KEY ({col})")),
                    "{name}.{col} needs a foreign key"
                );
            }
        }
    }
    let fk = c.query("PRAGMA foreign_keys", Vec::new()).await.unwrap();
    assert_eq!(fk[0][0], Value::Integer(1));
}

#[tokio::test]
async fn the_schema_keeps_adr_0086_after_every_migration_and_the_data_survives() {
    let s = Store::open(":memory:", 0).await.unwrap();
    adr_0086(&s).await;
    for n in 1..=3_usize {
        let at = i64::try_from(n).unwrap();
        s.register_domain(&domain(n), 10 + at).await.unwrap();
        adr_0086(&s).await;
        let v = s
            .read()
            .await
            .query(
                "SELECT version FROM fs_domain WHERE prefix = 'demo'",
                Vec::new(),
            )
            .await
            .unwrap();
        assert_eq!(
            v[0][0],
            Value::Integer(at),
            "schema version is the number of applied steps"
        );
        if n == 1 {
            let c = s.begin().await.unwrap();
            let r = c.exec("INSERT INTO demo_item (id, name) VALUES ('01927f00-0000-7000-8000-000000000001', 'a')", Vec::new()).await;
            c.finish(r).await.unwrap();
        }
    }
    let rows = s
        .read()
        .await
        .query("SELECT item FROM demo_part", Vec::new())
        .await
        .unwrap();
    assert_eq!(rows.len(), 1, "the code step moved the row");
    s.register_domain(&domain(3), 99).await.unwrap();
}

#[tokio::test]
async fn a_failing_step_changes_nothing_and_an_older_program_is_refused() {
    const BAD: [Step; 2] = [
        Step::Create(&[ITEM_V1]),
        Step::Sql("ALTER TABLE demo_item ADD COLUMN broken NOPE NOPE NOPE"),
    ];
    let s = Store::open(":memory:", 0).await.unwrap();
    let bad = Domain::new("demo", "demo", &[ITEM_V1], &BAD);
    let r = s.register_domain(&bad, 1).await;
    assert!(matches!(r, Err(Error::Schema(m)) if m.contains("step 2")));
    let v = s
        .read()
        .await
        .query(
            "SELECT version FROM fs_domain WHERE prefix = 'demo'",
            Vec::new(),
        )
        .await
        .unwrap();
    assert_eq!(
        v[0][0],
        Value::Integer(1),
        "step 1 stays, step 2 left no trace"
    );
    s.register_domain(&domain(2), 3).await.unwrap();
    let old = s.register_domain(&domain(1), 4).await;
    assert!(
        matches!(
            old,
            Err(Error::Newer {
                found: 2,
                known: 1,
                ..
            })
        ),
        "the store is newer than the program"
    );
}
