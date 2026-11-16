//! Second set of engine probes for the v5 draft (`docs/v5-entwurf.md`, section 8): the shapes the real schema uses
//! (TEXT keys, self-referencing keys, cascades over several levels, a second connection, STRICT together with
//! CHECK + FK + partial index, rollback after an error). Plain SQL on scratch connections, turso 0.8.3.

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

const UID_A: &str = "01927f00-0000-7000-8000-00000000000a";
const UID_B: &str = "01927f00-0000-7000-8000-00000000000b";
const UID_C: &str = "01927f00-0000-7000-8000-00000000000c";

#[tokio::test]
async fn text_keys_with_foreign_key_and_partial_unique_index() {
    let c = mem().await;
    run(&c, "PRAGMA foreign_keys = ON").await;
    run(&c, "CREATE TABLE objects (id TEXT PRIMARY KEY) STRICT").await;
    run(&c, "CREATE TABLE appt (object TEXT PRIMARY KEY REFERENCES objects(id) ON DELETE CASCADE, uid TEXT NOT NULL, master TEXT NULL, rid INTEGER NULL) STRICT").await;
    run(
        &c,
        "CREATE UNIQUE INDEX appt_uid ON appt (uid) WHERE master IS NULL",
    )
    .await;
    run(
        &c,
        "CREATE UNIQUE INDEX appt_rid ON appt (master, rid) WHERE master IS NOT NULL",
    )
    .await;
    run(&c, &format!("INSERT INTO objects VALUES ('{UID_A}')")).await;
    run(&c, &format!("INSERT INTO objects VALUES ('{UID_B}')")).await;
    run(&c, &format!("INSERT INTO objects VALUES ('{UID_C}')")).await;
    run(
        &c,
        &format!("INSERT INTO appt VALUES ('{UID_A}', 'u', NULL, NULL)"),
    )
    .await;
    let orphan = c
        .execute(
            "INSERT INTO appt VALUES ('no-such-object', 'x', NULL, NULL)",
            (),
        )
        .await;
    assert!(orphan.is_err(), "TEXT foreign key refuses an orphan");
    let second_master = c
        .execute(
            &format!("INSERT INTO appt VALUES ('{UID_B}', 'u', NULL, NULL)"),
            (),
        )
        .await;
    assert!(second_master.is_err(), "partial UNIQUE on a TEXT column");
    run(
        &c,
        &format!("INSERT INTO appt VALUES ('{UID_B}', 'u', '{UID_A}', 5)"),
    )
    .await;
    let same_rid = c
        .execute(
            &format!("INSERT INTO appt VALUES ('{UID_C}', 'u', '{UID_A}', 5)"),
            (),
        )
        .await;
    assert!(
        same_rid.is_err(),
        "partial UNIQUE (master, rid) with a TEXT master"
    );
}

#[tokio::test]
async fn self_referencing_key_and_multi_level_cascade() {
    let c = mem().await;
    run(&c, "PRAGMA foreign_keys = ON").await;
    run(&c, "CREATE TABLE objects (id TEXT PRIMARY KEY) STRICT").await;
    run(&c, "CREATE TABLE appt (object TEXT PRIMARY KEY REFERENCES objects(id) ON DELETE CASCADE, master TEXT NULL REFERENCES appt(object) ON DELETE CASCADE) STRICT").await;
    run(&c, "CREATE TABLE part (appt TEXT NOT NULL REFERENCES appt(object) ON DELETE CASCADE, k TEXT NOT NULL, PRIMARY KEY (appt, k)) STRICT").await;
    run(&c, "CREATE TABLE alert (appt TEXT NOT NULL, k TEXT NOT NULL, a TEXT NOT NULL, PRIMARY KEY (appt, k, a), FOREIGN KEY (appt, k) REFERENCES part(appt, k) ON DELETE CASCADE) STRICT").await;
    for id in [UID_A, UID_B, UID_C] {
        run(&c, &format!("INSERT INTO objects VALUES ('{id}')")).await;
    }
    run(&c, &format!("INSERT INTO appt VALUES ('{UID_A}', NULL)")).await;
    run(
        &c,
        &format!("INSERT INTO appt VALUES ('{UID_B}', '{UID_A}')"),
    )
    .await;
    run(
        &c,
        &format!("INSERT INTO appt VALUES ('{UID_C}', '{UID_A}')"),
    )
    .await;
    run(&c, &format!("INSERT INTO part VALUES ('{UID_A}', 'p1')")).await;
    run(&c, &format!("INSERT INTO part VALUES ('{UID_B}', 'p1')")).await;
    run(
        &c,
        &format!("INSERT INTO alert VALUES ('{UID_A}', 'p1', 'a1')"),
    )
    .await;
    run(
        &c,
        &format!("INSERT INTO alert VALUES ('{UID_B}', 'p1', 'a1')"),
    )
    .await;
    // Deleting only the envelope row of the master: master row, its exceptions, their participants and alerts go.
    run(&c, &format!("DELETE FROM objects WHERE id = '{UID_A}'")).await;
    assert_eq!(
        count(&c, "appt").await,
        0,
        "master and both exceptions are gone"
    );
    assert_eq!(
        count(&c, "part").await,
        0,
        "participants of master and exception"
    );
    assert_eq!(count(&c, "alert").await, 0, "alerts, three levels down");
    assert_eq!(
        count(&c, "objects").await,
        2,
        "the exception envelopes stay: code deletes them"
    );
}

#[tokio::test]
async fn foreign_keys_pragma_is_per_connection_and_per_open() {
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR"));
    let path = dir.join(format!("v5_probe_{}.db", std::process::id()));
    let p = path.to_str().expect("utf-8 path").to_owned();
    let db = Builder::new_local(&p).build().await.expect("open file");
    let c1 = db.connect().expect("c1");
    let c2 = db.connect().expect("c2");
    run(&c1, "CREATE TABLE p (id INTEGER PRIMARY KEY) STRICT").await;
    run(
        &c1,
        "CREATE TABLE ch (p INTEGER NOT NULL REFERENCES p(id) ON DELETE CASCADE) STRICT",
    )
    .await;
    let on = c1.execute("PRAGMA foreign_keys = ON", ()).await;
    assert!(on.is_ok(), "pragma understood: {on:?}");
    assert!(
        c1.execute("INSERT INTO ch VALUES (9)", ()).await.is_err(),
        "connection 1 (pragma set) refuses an orphan"
    );
    let second = c2.execute("INSERT INTO ch VALUES (8)", ()).await;
    println!("PROBE second connection of the same file, orphan insert: {second:?}");
    let ok = second.is_ok();
    drop((c1, c2, db));
    let db2 = Builder::new_local(&p).build().await.expect("reopen file");
    let c3 = db2.connect().expect("c3");
    let reopened = c3.execute("INSERT INTO ch VALUES (7)", ()).await;
    println!("PROBE reopened file, orphan insert without pragma: {reopened:?}");
    let default_off = reopened.is_ok();
    assert!(
        ok && default_off,
        "pragma is not shared with a second connection and not stored in the file: every connection sets it"
    );
    run(&c3, "PRAGMA foreign_keys = ON").await;
    assert!(c3.execute("INSERT INTO ch VALUES (6)", ()).await.is_err());
    drop((c3, db2));
    for suffix in ["", "-wal", "-shm", "-log"] {
        let _ = std::fs::remove_file(format!("{p}{suffix}"));
    }
}

#[tokio::test]
async fn strict_together_with_check_foreign_key_and_partial_index() {
    let c = mem().await;
    run(&c, "PRAGMA foreign_keys = ON").await;
    run(&c, "CREATE TABLE objects (id TEXT PRIMARY KEY) STRICT").await;
    run(&c, "CREATE TABLE appt (object TEXT PRIMARY KEY REFERENCES objects(id) ON DELETE CASCADE, kind TEXT NOT NULL CHECK (kind IN ('timed','floating','all_day')), local INTEGER NULL, date INTEGER NULL, zone TEXT NULL, end_zone TEXT NULL, assumed INTEGER NOT NULL CHECK (assumed IN (0,1)), len_days INTEGER NOT NULL, len_seconds INTEGER NOT NULL, uid TEXT NOT NULL, master TEXT NULL, CHECK (kind = 'timed' AND local IS NOT NULL AND date IS NULL AND zone IS NOT NULL OR kind = 'floating' AND local IS NOT NULL AND date IS NULL AND zone IS NULL AND end_zone IS NULL AND assumed = 0 OR kind = 'all_day' AND local IS NULL AND date IS NOT NULL AND zone IS NULL AND end_zone IS NULL AND assumed = 0 AND len_seconds = 0 AND len_days >= 1)) STRICT").await;
    run(
        &c,
        "CREATE UNIQUE INDEX appt_uid ON appt (uid) WHERE master IS NULL",
    )
    .await;
    for id in [UID_A, UID_B, UID_C] {
        run(&c, &format!("INSERT INTO objects VALUES ('{id}')")).await;
    }
    run(&c, &format!("INSERT INTO appt VALUES ('{UID_A}', 'all_day', NULL, 20000, NULL, NULL, 0, 2, 0, 'u1', NULL)")).await;
    let bad_zone = c.execute(&format!("INSERT INTO appt VALUES ('{UID_B}', 'all_day', NULL, 20000, NULL, 'Europe/Berlin', 0, 1, 0, 'u2', NULL)"), ()).await;
    assert!(bad_zone.is_err(), "all_day with end_zone breaks the CHECK");
    let zero_days = c.execute(&format!("INSERT INTO appt VALUES ('{UID_B}', 'all_day', NULL, 20000, NULL, NULL, 0, 0, 0, 'u2', NULL)"), ()).await;
    assert!(
        zero_days.is_err(),
        "all_day with len_days = 0 breaks the CHECK"
    );
    let bool2 = c.execute(&format!("INSERT INTO appt VALUES ('{UID_B}', 'timed', 5, NULL, 'UTC', NULL, 2, 0, 60, 'u2', NULL)"), ()).await;
    assert!(bool2.is_err(), "BOOL CHECK (x IN (0,1))");
    let text_in_int = c.execute(&format!("INSERT INTO appt VALUES ('{UID_B}', 'timed', 'noon', NULL, 'UTC', NULL, 0, 0, 60, 'u2', NULL)"), ()).await;
    assert!(text_in_int.is_err(), "STRICT still refuses TEXT in INTEGER");
    let no_parent = c.execute("INSERT INTO appt VALUES ('nobody', 'timed', 5, NULL, 'UTC', NULL, 0, 0, 60, 'u3', NULL)", ()).await;
    assert!(
        no_parent.is_err(),
        "FK refused in a STRICT table with CHECKs"
    );
    run(&c, &format!("INSERT INTO appt VALUES ('{UID_B}', 'timed', 5, NULL, 'UTC', NULL, 0, 0, 60, 'u2', NULL)")).await;
    let dup = c.execute(&format!("INSERT INTO appt VALUES ('{UID_C}', 'timed', 5, NULL, 'UTC', NULL, 0, 0, 60, 'u2', NULL)"), ()).await;
    assert!(
        dup.is_err(),
        "partial unique index in the same STRICT table"
    );
    assert_eq!(count(&c, "appt").await, 2);
}

#[tokio::test]
async fn rollback_after_a_failed_statement_leaves_nothing() {
    let c = mem().await;
    run(&c, "PRAGMA foreign_keys = ON").await;
    run(&c, "CREATE TABLE objects (id TEXT PRIMARY KEY) STRICT").await;
    run(&c, "CREATE TABLE child (o TEXT NOT NULL REFERENCES objects(id), n INTEGER NOT NULL CHECK (n > 0)) STRICT").await;
    run(&c, "BEGIN IMMEDIATE").await;
    run(&c, &format!("INSERT INTO objects VALUES ('{UID_A}')")).await;
    run(&c, &format!("INSERT INTO child VALUES ('{UID_A}', 1)")).await;
    let failed = c
        .execute(&format!("INSERT INTO child VALUES ('{UID_A}', -1)"), ())
        .await;
    assert!(failed.is_err(), "CHECK failure inside the transaction");
    let orphan = c
        .execute("INSERT INTO child VALUES ('nobody', 1)", ())
        .await;
    assert!(orphan.is_err(), "FK failure inside the transaction");
    run(&c, "ROLLBACK").await;
    assert_eq!(count(&c, "objects").await + count(&c, "child").await, 0);
    // The connection is usable afterwards.
    run(&c, "BEGIN IMMEDIATE").await;
    run(&c, &format!("INSERT INTO objects VALUES ('{UID_B}')")).await;
    run(&c, "COMMIT").await;
    assert_eq!(count(&c, "objects").await, 1);
}

#[tokio::test]
async fn activity_follows_purge_but_not_soft_delete() {
    let c = mem().await;
    run(&c, "PRAGMA foreign_keys = ON").await;
    run(
        &c,
        "CREATE TABLE objects (id TEXT PRIMARY KEY, deleted INTEGER NULL) STRICT",
    )
    .await;
    run(&c, "CREATE TABLE activity (id TEXT PRIMARY KEY, object TEXT NOT NULL REFERENCES objects(id) ON DELETE CASCADE) STRICT").await;
    run(&c, "CREATE TABLE activity_change (activity TEXT NOT NULL REFERENCES activity(id) ON DELETE CASCADE, seq INTEGER NOT NULL, PRIMARY KEY (activity, seq)) STRICT").await;
    run(&c, &format!("INSERT INTO objects VALUES ('{UID_A}', NULL)")).await;
    run(
        &c,
        &format!("INSERT INTO activity VALUES ('{UID_B}', '{UID_A}')"),
    )
    .await;
    run(
        &c,
        &format!("INSERT INTO activity_change VALUES ('{UID_B}', 0)"),
    )
    .await;
    run(
        &c,
        &format!("UPDATE objects SET deleted = 1 WHERE id = '{UID_A}'"),
    )
    .await;
    assert_eq!(
        count(&c, "activity_change").await,
        1,
        "trash (UPDATE deleted) keeps the history"
    );
    run(&c, &format!("DELETE FROM objects WHERE id = '{UID_A}'")).await;
    assert_eq!(
        count(&c, "activity").await + count(&c, "activity_change").await,
        0,
        "purge removes it"
    );
}
