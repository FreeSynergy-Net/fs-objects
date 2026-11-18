//! Pattern: Unit test of the original's comparison (bytes AND media type), on a hand-made new file.

use turso::Value;

use super::verify_extras;
use crate::v5::Store;
use crate::v5::codec::{id_text, id_value, int, text};
use crate::v5::id::from_v4;
use crate::v5::migrate_v4::notes::{NOTE_TYPE, NotesConverter};
use crate::v5::original::sha256_hex;
use crate::v5::tables::OBJECTS;
use crate::v5::tables_more::OBJECT_ORIGINAL;
use crate::{Blob, Envelope, StoredObject};

/// A v4 object with an original, and a new file whose row has the SAME bytes under another media type.
async fn check_with_media(stored_media: &str) -> Result<(), String> {
    let exe = std::env::current_exe().unwrap();
    let dir = exe.parent().unwrap().join(format!(
        "carry-{}-{}",
        std::process::id(),
        stored_media.len()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let bytes = b"BEGIN:VCARD".to_vec();
    let e = Envelope::new(NOTE_TYPE, "n");
    let o = StoredObject::new(e, "application/json", serde_json::json!({"title":"n"}))
        .with_original(Blob {
            media_type: "text/vcard".into(),
            bytes: bytes.clone(),
        });
    let id = from_v4(o.envelope.id);
    let file = dir.join("v5.db");
    let store = Store::open(file.to_str().unwrap(), 1).await.unwrap();
    let c = store.begin().await.unwrap();
    let row = vec![
        text(&id_text(id)),
        Value::Null,
        text(NOTE_TYPE),
        text("n"),
        text("n"),
        Value::Null,
        Value::Null,
        Value::Null,
        int(1),
        int(1),
        Value::Null,
        Value::Null,
        Value::Null,
        int(1),
        Value::Null,
        Value::Null,
    ];
    c.insert(&OBJECTS, row).await.unwrap();
    let original = vec![
        id_value(id),
        text(stored_media),
        Value::Blob(bytes.clone()),
        text(&sha256_hex(&bytes)),
        int(1),
        Value::Null,
    ];
    c.insert(&OBJECT_ORIGINAL, original).await.unwrap();
    let r = verify_extras(&c, &NotesConverter, &o).await;
    c.finish(Ok(())).await.unwrap();
    drop(store);
    let _ = std::fs::remove_dir_all(&dir);
    r
}

#[tokio::test]
async fn the_same_bytes_under_another_media_type_are_a_difference() {
    let r = check_with_media("text/x-other").await;
    assert!(
        r.as_ref().is_err_and(|m| m.contains("differs")),
        "same bytes, other media type: {r:?}"
    );
}

#[tokio::test]
async fn the_same_bytes_under_the_same_media_type_pass() {
    assert_eq!(check_with_media("text/vcard").await, Ok(()));
}
