//! Third set of engine probes for the v5 draft (`docs/v5-entwurf.md`, R4 and 2.1): REPLACE and cascade, activity of a
//! v4 row that was deleted but not purged, NaN in a REAL column. Plain SQL on scratch connections, turso 0.8.3.

use turso::{Builder, Connection, Value};

async fn mem() -> Connection {
    let db = Builder::new_local(":memory:").build().await.expect("open");
    db.connect().expect("connect")
}

async fn count(c: &Connection, table: &str) -> i64 {
    let mut r = c
        .query(&format!("SELECT count(*) FROM {table}"), ())
        .await
        .expect("query");
    let row = r.next().await.expect("row").expect("one row");
    match row.get_value(0).expect("value") {
        Value::Integer(i) => i,
        other => panic!("not an integer: {other:?}"),
    }
}

async fn run(c: &Connection, sql: &str) {
    c.execute(sql, ())
        .await
        .unwrap_or_else(|e| panic!("{sql}: {e}"));
}

async fn history_schema(c: &Connection) {
    run(c, "PRAGMA foreign_keys = ON").await;
    run(c, "CREATE TABLE objects (id TEXT PRIMARY KEY, name TEXT NOT NULL, deleted INTEGER NULL) STRICT").await;
    run(c, "CREATE TABLE activity (id TEXT PRIMARY KEY, object TEXT NOT NULL REFERENCES objects(id) ON DELETE CASCADE) STRICT").await;
    run(c, "CREATE TABLE activity_change (activity TEXT NOT NULL REFERENCES activity(id) ON DELETE CASCADE, seq INTEGER NOT NULL, PRIMARY KEY (activity, seq)) STRICT").await;
    run(c, "INSERT INTO objects VALUES ('o1', 'one', NULL)").await;
    run(c, "INSERT INTO activity VALUES ('a1', 'o1')").await;
    run(c, "INSERT INTO activity_change VALUES ('a1', 0)").await;
}

#[tokio::test]
async fn delete_plus_insert_and_replace_wipe_the_history_but_upsert_does_not() {
    let c = mem().await;
    history_schema(&c).await;
    run(&c, "INSERT INTO objects VALUES ('o1', 'two', NULL) ON CONFLICT(id) DO UPDATE SET name = excluded.name").await;
    run(&c, "UPDATE objects SET name = 'three' WHERE id = 'o1'").await;
    assert_eq!(
        count(&c, "activity_change").await,
        1,
        "UPDATE and upsert keep the history"
    );
    let replaced = c
        .execute(
            "INSERT OR REPLACE INTO objects VALUES ('o1', 'four', NULL)",
            (),
        )
        .await;
    println!(
        "PROBE INSERT OR REPLACE with an FK child: {replaced:?}, activity rows left: {}",
        count(&c, "activity").await
    );
    let after_replace = count(&c, "activity").await + count(&c, "activity_change").await;
    run(&c, "DELETE FROM objects WHERE id = 'o1'").await;
    run(&c, "INSERT INTO objects VALUES ('o1', 'five', NULL)").await;
    assert_eq!(
        count(&c, "activity").await,
        0,
        "DELETE + INSERT (the v4 write) wipes the history"
    );
    assert_eq!(
        after_replace, 0,
        "REPLACE fires the cascade as well (or is refused): never use it on objects"
    );
}

#[tokio::test]
async fn activity_of_a_deleted_not_purged_v4_object_needs_the_trash_row() {
    let c = mem().await;
    run(&c, "PRAGMA foreign_keys = ON").await;
    run(&c, "CREATE TABLE objects (id TEXT PRIMARY KEY, name TEXT NOT NULL, deleted INTEGER NULL) STRICT").await;
    run(&c, "CREATE TABLE activity (id TEXT PRIMARY KEY, kind TEXT NOT NULL, object TEXT NOT NULL REFERENCES objects(id) ON DELETE CASCADE) STRICT").await;
    let without = c
        .execute(
            "INSERT INTO activity VALUES ('a-del', 'delete', 'gone-in-v4')",
            (),
        )
        .await;
    assert!(
        without.is_err(),
        "without an objects row the FK refuses the delete activity (migration step 6 would fail)"
    );
    run(
        &c,
        "INSERT INTO objects VALUES ('gone-in-v4', 'from the before image', 1700000000000)",
    )
    .await;
    run(
        &c,
        "INSERT INTO activity VALUES ('a-del', 'delete', 'gone-in-v4')",
    )
    .await;
    assert_eq!(
        count(&c, "activity").await,
        1,
        "with the trash row the activity hangs on it"
    );
    run(
        &c,
        "UPDATE objects SET deleted = NULL WHERE id = 'gone-in-v4'",
    )
    .await;
    assert_eq!(count(&c, "activity").await, 1, "restore keeps the history");
}

#[tokio::test]
async fn nan_becomes_null_and_inf_is_stored_so_the_layer_must_refuse_both() {
    let c = mem().await;
    run(&c, "CREATE TABLE r (id INTEGER PRIMARY KEY, v REAL, w REAL CHECK (w = w), n REAL NOT NULL) STRICT").await;
    c.execute("INSERT INTO r (id, v, n) VALUES (1, ?1, 0.0)", (f64::NAN,))
        .await
        .expect("a nullable REAL takes NaN");
    run(&c, "INSERT INTO r (id, v, n) VALUES (2, 9e999, 0.0)").await;
    c.execute("INSERT INTO r (id, w, n) VALUES (3, ?1, 0.0)", (f64::NAN,))
        .await
        .expect("CHECK (w = w) does not see NaN: it arrives as NULL");
    let mut rows = c
        .query("SELECT v FROM r WHERE id IN (1, 2) ORDER BY id", ())
        .await
        .expect("query");
    let first = rows
        .next()
        .await
        .expect("row")
        .expect("row 1")
        .get_value(0)
        .expect("v");
    let second = rows
        .next()
        .await
        .expect("row")
        .expect("row 2")
        .get_value(0)
        .expect("v");
    assert_eq!(first, Value::Null, "NaN is stored as NULL");
    assert_eq!(
        second,
        Value::Real(f64::INFINITY),
        "infinity is stored as it is"
    );
    let not_null = c
        .execute("INSERT INTO r (id, n) VALUES (4, ?1)", (f64::NAN,))
        .await;
    assert!(
        not_null.is_err(),
        "NOT NULL REAL refuses NaN (it would be NULL)"
    );
}
