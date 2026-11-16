//! Migrator probe for the v5 draft (`docs/v5-entwurf.md` section 7 step 6, case (b)): the v4 rows "log purged, then
//! deleted" are recognized by one `SELECT`; the `redacted` activities are not copied (the v5 FK refuses them) and are
//! only counted for the report. Turso 0.8.3, in-memory. The migrator itself does not exist yet: this pins the rule.

use turso::{Builder, Connection, Value};

async fn mem() -> Connection {
    let db = Builder::new_local(":memory:").build().await.expect("open");
    db.connect().expect("connect")
}

async fn run(c: &Connection, sql: &str) {
    c.execute(sql, ())
        .await
        .unwrap_or_else(|e| panic!("{sql}: {e}"));
}

async fn ids(c: &Connection, sql: &str) -> Vec<String> {
    let mut r = c.query(sql, ()).await.expect("query");
    let mut out = Vec::new();
    while let Some(row) = r.next().await.expect("row") {
        match row.get_value(0).expect("value") {
            Value::Text(t) => out.push(t),
            other => panic!("not text: {other:?}"),
        }
    }
    out
}

/// The v4 tables as in `turso/schema.rs` (columns the migrator reads), plus the v4 rows of the case.
async fn v4_log_purged_then_deleted(c: &Connection) {
    run(c, "CREATE TABLE objects (id TEXT PRIMARY KEY)").await;
    run(c, "CREATE TABLE activities (id TEXT PRIMARY KEY, kind TEXT NOT NULL, object_id TEXT NOT NULL, at INTEGER NOT NULL, payload TEXT NOT NULL, redacted INTEGER NOT NULL DEFAULT 0)").await;
    run(
        c,
        "CREATE TABLE purged_objects (object_id TEXT PRIMARY KEY)",
    )
    .await;
    // gone: log purged, then deleted. Entry in purged_objects, no objects row, redacted activities (an edit after the
    // purge, then the delete without images).
    run(c, "INSERT INTO purged_objects VALUES ('gone')").await;
    run(
        c,
        "INSERT INTO activities VALUES ('01', 'Update', 'gone', 10, '{}', 1)",
    )
    .await;
    run(
        c,
        "INSERT INTO activities VALUES ('02', 'Delete', 'gone', 20, '{}', 1)",
    )
    .await;
    // alive: log purged, object alive: normal case (c), has an objects row.
    run(c, "INSERT INTO objects VALUES ('alive')").await;
    run(c, "INSERT INTO purged_objects VALUES ('alive')").await;
    run(
        c,
        "INSERT INTO activities VALUES ('03', 'Update', 'alive', 30, '{}', 1)",
    )
    .await;
    // trashed: deleted, log not purged (case (a)): not in purged_objects, no objects row, a Delete with images.
    run(
        c,
        "INSERT INTO activities VALUES ('04', 'Delete', 'trashed', 40, '{\"before\":{}}', 0)",
    )
    .await;
    // purged-after-delete (other order): only the purged_objects entry.
    run(c, "INSERT INTO purged_objects VALUES ('only-entry')").await;
}

const NOT_RESTORABLE: &str = "SELECT DISTINCT a.object_id FROM activities a \
    JOIN purged_objects p ON p.object_id = a.object_id \
    WHERE a.redacted = 1 AND NOT EXISTS (SELECT 1 FROM objects o WHERE o.id = a.object_id) \
    ORDER BY a.object_id";

#[tokio::test]
async fn case_b_is_recognized_by_a_select_and_nothing_else_matches() {
    let c = mem().await;
    v4_log_purged_then_deleted(&c).await;
    assert_eq!(ids(&c, NOT_RESTORABLE).await, vec!["gone".to_string()]);
    let n = ids(
        &c,
        "SELECT id FROM activities WHERE object_id = 'gone' AND redacted = 1 ORDER BY id",
    )
    .await;
    assert_eq!(
        n,
        vec!["01".to_string(), "02".to_string()],
        "the report lists these ids"
    );
    let high = ids(&c, "SELECT max(id) FROM activities").await;
    assert_eq!(
        high,
        vec!["04".to_string()],
        "meta.activity_high is taken over all activities, also the skipped"
    );
}

#[tokio::test]
async fn case_b_redacted_activities_are_refused_by_the_v5_fk_so_they_are_not_copied() {
    let c = mem().await;
    run(&c, "PRAGMA foreign_keys = ON").await;
    run(&c, "CREATE TABLE objects (id TEXT PRIMARY KEY, deleted INTEGER NULL, purge_after INTEGER NULL, CHECK ((deleted IS NULL) = (purge_after IS NULL))) STRICT").await;
    run(&c, "CREATE TABLE activity (id TEXT PRIMARY KEY, object TEXT NOT NULL REFERENCES objects(id) ON DELETE CASCADE, redacted INTEGER NOT NULL DEFAULT 0) STRICT").await;
    let r = c
        .execute(
            "INSERT INTO activity (id, object, redacted) VALUES ('02', 'gone', 1)",
            (),
        )
        .await;
    assert!(
        r.is_err(),
        "no trash row, so the activity cannot be copied; the migrator counts it instead"
    );
    let rows = ids(&c, "SELECT id FROM activity").await;
    assert_eq!(rows.len(), 0);
}
