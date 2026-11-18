//! "A date stays a date" (normalization rule): an all-day value of the example domain is a DATE (days), never an
//! instant in UTC and never shifted by a zone; an instant (`due`) is the one that is rewritten as UTC.
#![cfg(all(feature = "v5", unix))] // the migrator is unix only for now (Windows: a locked file cannot be read through another handle)

mod common_migrate;

use chrono::NaiveDate;
use common_migrate::{Dir, NOW, note, opts, v4_file};
use fs_objects::v5::Store;
use fs_objects::v5::codec::{get_int, get_opt_int};
use fs_objects::v5::migrate_v4::MigrationPlan;
use fs_objects::v5::migrate_v4::notes::NotesConverter;
use serde_json::json;

#[tokio::test]
async fn an_all_day_value_stays_a_date() {
    let dir = Dir::new("fs-objects-date");
    let objects = vec![
        note("allday", json!({"title":"allday","day":"2026-03-01"})),
        note(
            "edge",
            json!({"title":"edge","day":"2026-12-31","due":"2026-12-31T23:30:00-05:00"}),
        ),
        note(
            "withtime",
            json!({"title":"withtime","day":"2026-03-01T00:00:00+01:00"}),
        ),
    ];
    let path = v4_file(&dir, objects).await;
    let mut plan = MigrationPlan::open(&path).unwrap();
    let report = plan
        .check(&NotesConverter, opts(), NOW)
        .await
        .unwrap()
        .clone();
    assert_eq!(report.converted_total(), 2);
    assert_eq!(
        report.held.len(),
        1,
        "a day with a time is no date: held back, not turned into an instant"
    );
    assert!(
        report.held[0].reason.contains("not a plain date"),
        "{:?}",
        report.held
    );
    let days = |y, m, d| {
        NaiveDate::from_ymd_opt(y, m, d)
            .unwrap()
            .signed_duration_since(NaiveDate::from_ymd_opt(1970, 1, 1).unwrap())
            .num_days()
    };
    let v5 = Store::open(&report.target, NOW).await.unwrap();
    let rows = v5.read().await.query("SELECT n.day, n.due, o.name FROM notes_note n JOIN objects o ON o.id = n.object ORDER BY o.name", Vec::new()).await.unwrap();
    assert_eq!(rows.len(), 2);
    // "allday": the day number of the calendar date, no due.
    assert_eq!(get_int(&rows[0], 0).unwrap(), days(2026, 3, 1));
    assert_eq!(get_opt_int(&rows[0], 1).unwrap(), None);
    // "edge": 31 December stays 31 December although the instant (23:30 at -05:00) is already 1 January in UTC.
    assert_eq!(get_int(&rows[1], 0).unwrap(), days(2026, 12, 31));
    let due = get_opt_int(&rows[1], 1).unwrap().unwrap();
    assert_eq!(
        due,
        chrono::DateTime::parse_from_rfc3339("2027-01-01T04:30:00Z")
            .unwrap()
            .timestamp_millis()
    );
}
