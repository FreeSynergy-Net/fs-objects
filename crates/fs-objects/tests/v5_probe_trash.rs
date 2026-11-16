//! Fourth set of engine probes for the v5 draft (`docs/v5-entwurf.md`, R4 and 2.1): log purge versus final delete,
//! the trash CHECK, the lower-case hex CHECK of `sha256`, v4 cases of the migration. Turso 0.8.3.

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

async fn schema(c: &Connection) {
    run(c, "PRAGMA foreign_keys = ON").await;
    run(c, "CREATE TABLE objects (id TEXT PRIMARY KEY, deleted INTEGER NULL, purge_after INTEGER NULL, CHECK ((deleted IS NULL) = (purge_after IS NULL))) STRICT").await;
    run(c, "CREATE TABLE activity (id TEXT PRIMARY KEY, object TEXT NOT NULL REFERENCES objects(id) ON DELETE CASCADE, redacted INTEGER NOT NULL DEFAULT 0) STRICT").await;
    run(c, "CREATE TABLE activity_change (activity TEXT NOT NULL REFERENCES activity(id) ON DELETE CASCADE, seq INTEGER NOT NULL, PRIMARY KEY (activity, seq)) STRICT").await;
    run(
        c,
        "CREATE TABLE purged_objects (id TEXT PRIMARY KEY) STRICT",
    )
    .await;
}

#[tokio::test]
async fn log_purge_keeps_the_object_and_final_delete_removes_everything() {
    let c = mem().await;
    schema(&c).await;
    run(&c, "INSERT INTO objects VALUES ('o1', NULL, NULL)").await;
    run(&c, "INSERT INTO activity (id, object) VALUES ('a1', 'o1')").await;
    run(&c, "INSERT INTO activity_change VALUES ('a1', 0)").await;
    run(&c, "BEGIN IMMEDIATE").await;
    run(&c, "DELETE FROM activity WHERE object = 'o1'").await;
    run(&c, "INSERT INTO purged_objects VALUES ('o1')").await;
    run(&c, "COMMIT").await;
    assert_eq!(
        count(&c, "objects").await,
        1,
        "the object lives on after the log purge (v4 case: log purged, object alive)"
    );
    assert_eq!(
        count(&c, "activity").await + count(&c, "activity_change").await,
        0,
        "activity and changes are gone"
    );
    run(
        &c,
        "INSERT INTO activity (id, object, redacted) VALUES ('a2', 'o1', 1)",
    )
    .await;
    assert_eq!(
        count(&c, "activity").await,
        1,
        "a later entry (redacted) is accepted"
    );
    run(&c, "DELETE FROM objects WHERE id = 'o1'").await;
    assert_eq!(count(&c, "activity").await, 0, "final delete cascades");
}

#[tokio::test]
async fn v4_deleted_after_log_purge_has_no_before_image_and_no_row_to_hang_on() {
    let c = mem().await;
    schema(&c).await;
    // v4: delete activity gone with the log purge, object row gone: nothing is left to migrate, no trash row is created.
    assert_eq!(count(&c, "objects").await + count(&c, "activity").await, 0);
    let orphan = c
        .execute(
            "INSERT INTO activity (id, object) VALUES ('a-del', 'gone')",
            (),
        )
        .await;
    assert!(
        orphan.is_err(),
        "an activity without a parent row is refused: such a case is reported, not inserted"
    );
}

#[tokio::test]
async fn trash_columns_are_set_and_cleared_together() {
    let c = mem().await;
    schema(&c).await;
    assert!(
        c.execute("INSERT INTO objects VALUES ('o1', 1000, NULL)", ())
            .await
            .is_err(),
        "deleted without purge_after"
    );
    assert!(
        c.execute("INSERT INTO objects VALUES ('o1', NULL, 1000)", ())
            .await
            .is_err(),
        "purge_after without deleted"
    );
    run(&c, "INSERT INTO objects VALUES ('o1', 1000, 2592001000)").await;
    run(
        &c,
        "UPDATE objects SET deleted = NULL, purge_after = NULL WHERE id = 'o1'",
    )
    .await;
    assert!(
        c.execute("UPDATE objects SET deleted = 5 WHERE id = 'o1'", ())
            .await
            .is_err(),
        "half a trash entry on UPDATE"
    );
}

#[tokio::test]
async fn sha256_check_accepts_only_lower_case_hex_of_length_64() {
    let c = mem().await;
    run(&c, "CREATE TABLE o (id INTEGER PRIMARY KEY, sha256 TEXT NOT NULL CHECK (length(sha256) = 64 AND sha256 NOT GLOB '*[^0-9a-f]*')) STRICT").await;
    let good = "ab".repeat(32);
    let upper = "AB".repeat(32);
    let nonhex = format!("{}zz", "ab".repeat(31));
    let short = "ab".repeat(31);
    run(&c, &format!("INSERT INTO o VALUES (1, '{good}')")).await;
    for (id, bad) in [(2, upper), (3, nonhex), (4, short)] {
        let r = c
            .execute(&format!("INSERT INTO o VALUES ({id}, '{bad}')"), ())
            .await;
        assert!(r.is_err(), "refused: {bad}");
    }
    assert_eq!(count(&c, "o").await, 1);
}
