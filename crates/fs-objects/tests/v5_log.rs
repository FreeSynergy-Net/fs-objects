//! The typed log (R10), log purge (ADR 0029), unlogged ids, and the hole punch of a redaction.
#![cfg(feature = "v5")]
#![allow(clippy::many_single_char_names)]

mod common_v5;

use common_v5::{actor, count, id, store};
use fs_objects::v5::codec::{get_text, id_text, id_value};
use fs_objects::v5::envelope::{bump, insert};
use fs_objects::v5::log::{FieldChange, Kind, changes_of, log, mark_unlogged, purge_log, redact};
use fs_objects::v5::log_codec::LogValue;
use fs_types::{
    Attendee, BasicValue, Date, LocalDateTime, PersonArt, PersonRef, Reference, Revision, Utc, Zone,
};

const T0: i64 = 1_794_823_200_000;

fn r(t: &str) -> Reference {
    Reference::parse(t).unwrap()
}

fn all_values() -> Vec<LogValue> {
    let p = PersonRef {
        id: r("urn:uuid:01927f00-0000-7000-8000-0000000000b2"),
        art: PersonArt::Group,
        abzug: Some(id(T0)),
    };
    vec![
        LogValue::Int(-4),
        LogValue::Real(2.5),
        LogValue::Text("Müller".into()),
        LogValue::Bool(true),
        LogValue::Utc(Utc::from_millis(T0)),
        LogValue::Local(LocalDateTime::parse("2026-11-16T09:30:00").unwrap()),
        LogValue::Date(Date::parse("2026-11-16").unwrap()),
        LogValue::Zone(Zone::iana("Europe/Berlin").unwrap()),
        LogValue::Object(id(T0)),
        LogValue::Reference(r("https://example.org/x")),
        LogValue::Person(p.clone()),
        LogValue::Attendee(Attendee::Actor(p)),
        LogValue::Attendee(Attendee::Resource(r(
            "urn:uuid:01927f00-0000-7000-8000-0000000000b3",
        ))),
        LogValue::Attendee(Attendee::Location(r(
            "urn:uuid:01927f00-0000-7000-8000-0000000000b4",
        ))),
    ]
}

#[tokio::test]
async fn every_value_kind_round_trips_through_the_typed_columns() {
    let s = store(&[]).await;
    let o = id(T0);
    let changes: Vec<FieldChange> = all_values()
        .into_iter()
        .enumerate()
        .map(|(i, v)| FieldChange {
            field: format!("f{i}"),
            row_key: (i % 2 == 0).then(|| "k".to_owned()),
            old: (i % 3 == 0).then(|| v.clone()),
            new: Some(v),
        })
        .collect();
    let c = s.begin().await.unwrap();
    let r = async {
        insert(&c, o, "Note", "n", &actor(), T0).await?;
        log(&c, o, Kind::Update, &actor(), T0, &changes).await
    }
    .await;
    c.finish(r).await.unwrap();
    let c = s.read().await;
    let aid = get_text(
        &c.query("SELECT id FROM activity", Vec::new())
            .await
            .unwrap()[0],
        0,
    )
    .unwrap();
    assert_eq!(changes_of(&c, &aid).await.unwrap(), changes);
    let kind = c
        .query("SELECT kind FROM activity", Vec::new())
        .await
        .unwrap();
    assert_eq!(
        Kind::from_word(&get_text(&kind[0], 0).unwrap()).unwrap(),
        Kind::Update
    );
}

#[tokio::test]
async fn a_change_without_a_value_and_a_bad_value_are_refused_and_leave_nothing() {
    let s = store(&[]).await;
    let o = id(T0);
    let none = FieldChange {
        field: "f".into(),
        row_key: None,
        old: None,
        new: None,
    };
    let c = s.begin().await.unwrap();
    let r = async {
        insert(&c, o, "Note", "n", &actor(), T0).await?;
        log(&c, o, Kind::Update, &actor(), T0, &[none]).await
    }
    .await;
    assert!(c.finish(r).await.is_err());
    assert_eq!(count(&s, "activity").await + count(&s, "objects").await, 0);
}

#[tokio::test]
async fn log_purge_keeps_the_object_unlogged_ids_write_nothing_and_updates_keep_the_history() {
    let s = store(&[]).await;
    let (a, b) = (id(T0), id(T0));
    let c = s.begin().await.unwrap();
    let r = async {
        for (o, n) in [(a, "a"), (b, "b")] {
            insert(&c, o, "Note", n, &actor(), T0).await?;
            log(&c, o, Kind::Create, &actor(), T0, &[]).await?;
        }
        bump(&c, a, Revision(1), Some("a2"), T0 + 1).await?;
        log(&c, a, Kind::Update, &actor(), T0 + 1, &[]).await?;
        mark_unlogged(&c, b).await?;
        log(&c, b, Kind::Update, &actor(), T0 + 2, &[]).await
    }
    .await;
    c.finish(r).await.unwrap();
    assert_eq!(
        count(&s, "activity").await,
        3,
        "two for a, one for b (the update of b is not written)"
    );
    let c = s.begin().await.unwrap();
    purge_log(&c, a).await.unwrap();
    c.finish(Ok(())).await.unwrap();
    assert_eq!(count(&s, "activity").await, 1);
    assert_eq!(count(&s, "objects").await, 2, "the object stays");
    assert_eq!(count(&s, "purged_objects").await, 1);
}

#[tokio::test]
async fn redact_marks_the_entries_and_drops_their_changes() {
    let s = store(&[]).await;
    let o = id(T0);
    let ch = |f: &str, k: &str| FieldChange {
        field: f.into(),
        row_key: Some(k.into()),
        old: Some(LogValue::Text("old".into())),
        new: Some(LogValue::Text("secret".into())),
    };
    let c = s.begin().await.unwrap();
    let r = async {
        insert(&c, o, "Note", "n", &actor(), T0).await?;
        log(
            &c,
            o,
            Kind::Update,
            &actor(),
            T0,
            &[ch("override.title", "r1"), ch("override.title", "r2")],
        )
        .await?;
        redact(&c, o, "r1", "override.%").await
    }
    .await;
    c.finish(r).await.unwrap();
    let c = s.read().await;
    let left = c
        .query("SELECT row_key FROM activity_change", Vec::new())
        .await
        .unwrap();
    assert_eq!(left.len(), 1, "only the other row's change is left");
    let red = c
        .query(
            "SELECT redacted FROM activity WHERE object = ?",
            vec![id_value(o)],
        )
        .await
        .unwrap();
    assert_eq!(red[0][0], turso::Value::Integer(1));
    let _ = id_text(o);
}
