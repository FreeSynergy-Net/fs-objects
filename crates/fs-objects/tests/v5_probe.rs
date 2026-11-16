//! What the embedded engine (turso 0.8.3) can do that the v5 draft (`docs/v5-entwurf.md`, requirement R5) relies on.
//!
//! Not a feature test of fs-objects: each probe runs plain SQL on a scratch connection and asserts what the
//! engine did when this was written. If the engine changes and a probe flips, the draft's table in section 8
//! must be re-read before anything is built on it. Probes that fail here are reported, not hidden.

use turso::{Builder, Connection, Value};

async fn scratch() -> Connection {
    let db = Builder::new_local(":memory:").build().await.expect("open");
    db.connect().expect("connect")
}

async fn rows(c: &Connection, sql: &str) -> Vec<Vec<Value>> {
    let mut r = c.query(sql, ()).await.expect("query");
    let mut out = Vec::new();
    while let Some(row) = r.next().await.expect("row") {
        out.push(
            (0..row.column_count())
                .map(|i| row.get_value(i).expect("value"))
                .collect(),
        );
    }
    out
}

fn int(v: &Value) -> i64 {
    match v {
        Value::Integer(i) => *i,
        other => panic!("not an integer: {other:?}"),
    }
}

#[tokio::test]
async fn integer_columns_hold_full_i64_and_sort_as_numbers() {
    let c = scratch().await;
    c.execute(
        "CREATE TABLE t (id INTEGER PRIMARY KEY, at INTEGER NOT NULL)",
        (),
    )
    .await
    .unwrap();
    for (id, at) in [
        (1, -62_135_596_800_000_i64),
        (2, 1_794_816_000_000),
        (3, 253_402_300_799_000),
        (4, 9),
        (5, i64::MAX),
    ] {
        c.execute(&format!("INSERT INTO t VALUES ({id}, {at})"), ())
            .await
            .unwrap();
    }
    let order: Vec<i64> = rows(&c, "SELECT id FROM t ORDER BY at")
        .await
        .iter()
        .map(|r| int(&r[0]))
        .collect();
    assert_eq!(order, vec![1, 4, 2, 3, 5]);
    let hit = rows(
        &c,
        "SELECT count(*) FROM t WHERE at >= 0 AND at < 1794816000001",
    )
    .await;
    assert_eq!(int(&hit[0][0]), 2);
}

#[tokio::test]
async fn check_constraint_is_enforced() {
    let c = scratch().await;
    let made = c
        .execute("CREATE TABLE a (id INTEGER PRIMARY KEY, all_day INTEGER NOT NULL, zone TEXT, CHECK (all_day = 0 OR zone IS NULL))", ())
        .await;
    assert!(made.is_ok(), "CHECK is accepted in CREATE TABLE: {made:?}");
    assert!(
        c.execute("INSERT INTO a VALUES (1, 1, NULL)", ())
            .await
            .is_ok()
    );
    assert!(
        c.execute("INSERT INTO a VALUES (2, 0, 'Europe/Berlin')", ())
            .await
            .is_ok()
    );
    let bad = c
        .execute("INSERT INTO a VALUES (3, 1, 'Europe/Berlin')", ())
        .await;
    assert!(bad.is_err(), "a row that breaks the CHECK must be refused");
}

#[tokio::test]
async fn partial_unique_index_is_enforced() {
    let c = scratch().await;
    c.execute(
        "CREATE TABLE a (id INTEGER PRIMARY KEY, uid TEXT NOT NULL, master INTEGER, rid INTEGER)",
        (),
    )
    .await
    .unwrap();
    let made = c
        .execute(
            "CREATE UNIQUE INDEX a_uid ON a (uid) WHERE master IS NULL",
            (),
        )
        .await;
    assert!(made.is_ok(), "partial unique index: {made:?}");
    assert!(
        c.execute("INSERT INTO a VALUES (1, 'u', NULL, NULL)", ())
            .await
            .is_ok()
    );
    assert!(
        c.execute("INSERT INTO a VALUES (2, 'u', 1, 5)", ())
            .await
            .is_ok(),
        "an exception row may share the uid"
    );
    assert!(
        c.execute("INSERT INTO a VALUES (3, 'u', NULL, NULL)", ())
            .await
            .is_err(),
        "a second master with the uid is refused"
    );
}

#[tokio::test]
async fn foreign_key_cascade_deletes_child_rows() {
    let c = scratch().await;
    let pragma = c.execute("PRAGMA foreign_keys = ON", ()).await;
    assert!(pragma.is_ok(), "the pragma is understood: {pragma:?}");
    c.execute("CREATE TABLE p (id INTEGER PRIMARY KEY)", ())
        .await
        .unwrap();
    let made = c
        .execute("CREATE TABLE ch (p INTEGER NOT NULL REFERENCES p(id) ON DELETE CASCADE, k TEXT NOT NULL, PRIMARY KEY (p, k))", ())
        .await;
    assert!(
        made.is_ok(),
        "FOREIGN KEY ... ON DELETE CASCADE is accepted: {made:?}"
    );
    c.execute("INSERT INTO p VALUES (1)", ()).await.unwrap();
    c.execute("INSERT INTO ch VALUES (1, 'a')", ())
        .await
        .unwrap();
    let orphan = c.execute("INSERT INTO ch VALUES (9, 'x')", ()).await;
    c.execute("DELETE FROM p WHERE id = 1", ()).await.unwrap();
    let left = int(&rows(&c, "SELECT count(*) FROM ch WHERE p = 1").await[0][0]);
    // Recorded outcome: see the assertions below; the draft says which of the two it needs.
    assert_eq!(left, 0, "cascade removes the child row");
    assert!(orphan.is_err(), "a child row without a parent is refused");
}

#[tokio::test]
async fn one_transaction_over_several_tables_rolls_back_together() {
    let c = scratch().await;
    c.execute("CREATE TABLE a (id INTEGER PRIMARY KEY)", ())
        .await
        .unwrap();
    c.execute("CREATE TABLE b (id INTEGER PRIMARY KEY)", ())
        .await
        .unwrap();
    c.execute("BEGIN IMMEDIATE", ()).await.unwrap();
    c.execute("INSERT INTO a VALUES (1)", ()).await.unwrap();
    c.execute("INSERT INTO b VALUES (1)", ()).await.unwrap();
    c.execute("ROLLBACK", ()).await.unwrap();
    assert_eq!(
        int(&rows(&c, "SELECT count(*) FROM a").await[0][0])
            + int(&rows(&c, "SELECT count(*) FROM b").await[0][0]),
        0
    );
}

#[tokio::test]
async fn composite_keys_text_enum_and_add_column() {
    let c = scratch().await;
    c.execute("CREATE TABLE r (a INTEGER NOT NULL, k TEXT NOT NULL, part TEXT NOT NULL CHECK (part IN ('day','month')), v INTEGER NOT NULL, PRIMARY KEY (a, k, part, v))", ()).await.unwrap();
    assert!(
        c.execute("INSERT INTO r VALUES (1, 'x', 'day', 3)", ())
            .await
            .is_ok()
    );
    assert!(
        c.execute("INSERT INTO r VALUES (1, 'x', 'day', 3)", ())
            .await
            .is_err(),
        "composite primary key is unique"
    );
    assert!(
        c.execute("INSERT INTO r VALUES (1, 'x', 'year', 3)", ())
            .await
            .is_err(),
        "ENUM as CHECK ... IN (...)"
    );
    assert!(
        c.execute(
            "ALTER TABLE r ADD COLUMN extra INTEGER NOT NULL DEFAULT 0",
            ()
        )
        .await
        .is_ok(),
        "ADD COLUMN with default"
    );
}

#[tokio::test]
async fn foreign_keys_are_off_until_the_pragma() {
    let c = scratch().await;
    c.execute("CREATE TABLE p (id INTEGER PRIMARY KEY)", ())
        .await
        .unwrap();
    c.execute(
        "CREATE TABLE ch (p INTEGER NOT NULL REFERENCES p(id) ON DELETE CASCADE)",
        (),
    )
    .await
    .unwrap();
    let without = c.execute("INSERT INTO ch VALUES (9)", ()).await;
    let pragma = c.execute("PRAGMA foreign_keys = ON", ()).await;
    let with = c.execute("INSERT INTO ch VALUES (9)", ()).await;
    // The draft (section 8) must say which of these the store has to do itself: enforce keys by pragma or by code.
    assert!(pragma.is_ok(), "the pragma is understood: {pragma:?}");
    assert!(with.is_err(), "with the pragma an orphan is refused");
    assert!(
        without.is_ok(),
        "without the pragma an orphan is accepted: v5 must switch keys on for every connection it opens"
    );
}

#[tokio::test]
async fn strict_tables_are_probed_not_assumed() {
    let c = scratch().await;
    let strict = c
        .execute(
            "CREATE TABLE s (a INTEGER NOT NULL, b TEXT NOT NULL) STRICT",
            (),
        )
        .await;
    let wrong_type = c
        .execute("INSERT INTO s VALUES ('not a number', 'x')", ())
        .await;
    let plain = c.execute("CREATE TABLE n (a INTEGER NOT NULL)", ()).await;
    let loose = c.execute("INSERT INTO n VALUES ('not a number')", ()).await;
    assert!(
        strict.is_ok() && plain.is_ok(),
        "STRICT tables are accepted"
    );
    assert!(
        wrong_type.is_err(),
        "STRICT refuses TEXT in an INTEGER column"
    );
    assert!(
        loose.is_ok(),
        "a table without STRICT takes anything: v5 tables must all be STRICT"
    );
}

#[tokio::test]
async fn generated_columns_are_probed_not_assumed() {
    let c = scratch().await;
    let generated = c
        .execute(
            "CREATE TABLE g (a INTEGER NOT NULL, b INTEGER GENERATED ALWAYS AS (a * 2) STORED)",
            (),
        )
        .await;
    assert!(
        generated.is_err(),
        "generated columns need an experimental flag: derived columns are written by code (section 8)"
    );
}
