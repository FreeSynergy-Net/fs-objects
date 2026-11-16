//! Origin and original (ADR 0030), the channel list of a domain, and the file port.
#![cfg(feature = "v5")]
#![allow(clippy::many_single_char_names)]

mod common_v5;

use common_v5::{actor, count, domain, id, store};
use fs_objects::v5::Error;
use fs_objects::v5::envelope::insert;
use fs_objects::v5::files::{FileStore, MemoryFiles};
use fs_objects::v5::original::{NewOrigin, original_iri, original_of, record, sha256_hex};
use fs_types::{BasicValue, MediaType, Reference, Revision};

const T0: i64 = 1_794_823_200_000;

fn origin(channel: &str, bytes: Option<&[u8]>) -> NewOrigin {
    NewOrigin {
        channel: channel.into(),
        source: Reference::parse("https://example.org/cal.ics").unwrap(),
        media_type: MediaType("text/calendar".into()),
        foreign_uid: "uid-1".into(),
        foreign_recurrence_id: None,
        foreign_etag: Some("e1".into()),
        prod_id: None,
        importer: "test".into(),
        original: bytes.map(|b| (b.to_vec(), MediaType("text/calendar".into()))),
    }
}

#[tokio::test]
async fn the_original_is_kept_with_its_checksum_and_never_rewritten() {
    let s = store(&[]).await;
    let o = id(T0);
    let d = domain(&[]);
    let c = s.begin().await.unwrap();
    let r = async {
        insert(&c, o, "Note", "n", &actor(), T0).await?;
        record(
            &c,
            &d,
            o,
            &origin("import", Some(b"BEGIN:VCALENDAR")),
            Revision(1),
            T0,
        )
        .await
    }
    .await;
    c.finish(r).await.unwrap();
    let c = s.read().await;
    let (bytes, media) = original_of(&c, o).await.unwrap().unwrap();
    assert_eq!(
        (bytes.as_slice(), media.0.as_str()),
        (&b"BEGIN:VCALENDAR"[..], "text/calendar")
    );
    let rows = c
        .query("SELECT sha256 FROM object_original", Vec::new())
        .await
        .unwrap();
    assert_eq!(
        rows[0][0],
        turso::Value::Text(sha256_hex(b"BEGIN:VCALENDAR"))
    );
    assert!(original_iri(&sha256_hex(b"x")).starts_with("urn:sha256:"));
    drop(c);
    let c = s.begin().await.unwrap();
    let again = record(
        &c,
        &d,
        o,
        &origin("import", Some(b"other")),
        Revision(1),
        T0 + 1,
    )
    .await;
    assert!(
        again.is_err(),
        "a second origin row for the same object is refused"
    );
    c.finish(again).await.unwrap_err();
    assert_eq!(count(&s, "object_original").await, 1);
}

#[tokio::test]
async fn only_the_channels_a_domain_reported_are_accepted() {
    let s = store(&[]).await;
    let (d, o) = (domain(&[]), id(T0));
    let c = s.begin().await.unwrap();
    insert(&c, o, "Note", "n", &actor(), T0).await.unwrap();
    let bad = record(&c, &d, o, &origin("caldav", None), Revision(1), T0).await;
    assert!(
        matches!(&bad, Err(Error::Invalid(p)) if p[0].field == "object_origin.channel"),
        "{bad:?}"
    );
    record(&c, &d, o, &origin("carddav", None), Revision(1), T0)
        .await
        .unwrap();
    c.finish(Ok(())).await.unwrap();
    assert_eq!(
        original_of(&s.read().await, o).await.unwrap(),
        None,
        "no file kept"
    );
}

#[tokio::test]
async fn the_memory_file_store_checks_the_checksum() {
    let f = MemoryFiles::default();
    let media = MediaType("text/plain".into());
    let r = f.write(b"hello".to_vec(), &media).await.unwrap();
    assert_eq!(f.read(&r).await.unwrap(), b"hello");
    let mut wrong = r.clone();
    wrong.sha256 = [0; 32];
    assert!(matches!(f.read(&wrong).await, Err(Error::Invalid(_))));
    let mut absent = r;
    absent.iri = Reference::parse("urn:sha256:00").unwrap();
    assert!(matches!(f.read(&absent).await, Err(Error::Unavailable(_))));
}
