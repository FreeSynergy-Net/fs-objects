//! What the scaffold carries beyond the converter's rows: the original, the envelope times and the media type
//! (carried by the converter or HELD BACK), and what is not carried yet (held, with the reason).
//!
//! Pattern: Specification (small independent rules: each says "carried" or gives the reason).

use sha2::{Digest, Sha256};
use turso::Value;

use super::convert::Converter;
use crate::StoredObject;
use crate::v5::Conn;
use crate::v5::codec::{get_text, id_value, int, text};
use crate::v5::original::{original_of, sha256_hex};
use crate::v5::tables_more::OBJECT_ORIGINAL;

/// What the scaffold cannot carry: the object is held with a reason instead of dropping the data.
pub(super) fn not_carried(o: &StoredObject, conv: &dyn Converter) -> Option<String> {
    let e = &o.envelope;
    if [&e.to, &e.cc, &e.bto, &e.bcc, &e.audience]
        .iter()
        .any(|l| !l.is_empty())
    {
        return Some("addressing is not carried yet (tables object_addressing follow)".into());
    }
    if !e.sensitive.is_empty() {
        return Some(
            "protection categories are not carried yet (table object_protection follows)".into(),
        );
    }
    if (e.start_time.is_some() || e.end_time.is_some()) && !conv.carries_times() {
        return Some(
            "start_time/end_time are set and this converter does not carry them (the v5 envelope has no such columns)"
                .into(),
        );
    }
    let want = conv.content_type(&e.object_type);
    if o.content_type != want {
        return Some(format!(
            "content_type {} is not the media type the converter writes back ({want})",
            o.content_type
        ));
    }
    None
}

/// The row of the original (ADR 0030) for the new file, when the v4 object has one.
pub(super) fn original_row(o: &StoredObject, received: i64) -> Option<Vec<Value>> {
    let b = o.original.as_ref()?;
    Some(vec![
        id_value(crate::v5::id::from_v4(o.envelope.id)),
        text(&b.media_type),
        Value::Blob(b.bytes.clone()),
        text(&sha256_hex(&b.bytes)),
        int(received),
        Value::Null,
    ])
}

/// The table of the original, for the insert.
pub(super) const ORIGINAL_TABLE: &crate::v5::Table = &OBJECT_ORIGINAL;

/// The round trip of the original and of the times: what is in the new file must equal the v4 object.
pub(super) async fn verify_extras(
    c: &Conn<'_>,
    conv: &dyn Converter,
    o: &StoredObject,
) -> std::result::Result<(), String> {
    let id = crate::v5::id::from_v4(o.envelope.id);
    let back = original_of(c, id).await.map_err(|e| e.to_string())?;
    let same = match (&o.original, &back) {
        (None, None) => true,
        (Some(v4), Some((bytes, media))) => v4.bytes == *bytes && v4.media_type == media.0,
        _ => false,
    };
    if !same {
        return Err("the original read back differs from the v4 original".into());
    }
    if let Some(b) = &o.original {
        let rows = c
            .query(
                "SELECT sha256 FROM object_original WHERE object = ?",
                vec![id_value(id)],
            )
            .await
            .map_err(|e| e.to_string())?;
        let stored = rows
            .first()
            .and_then(|r| get_text(r, 0).ok())
            .unwrap_or_default();
        if stored != hex(&Sha256::digest(&b.bytes)) {
            return Err("the stored SHA-256 of the original is wrong".into());
        }
    }
    if conv.carries_times() {
        let e = &o.envelope;
        let want = (
            e.start_time.map(|t| t.timestamp_millis()),
            e.end_time.map(|t| t.timestamp_millis()),
        );
        let got = conv.export_times(c, id).await.map_err(|e| e.to_string())?;
        if got != want {
            return Err("start_time/end_time read back differ from the v4 envelope".into());
        }
    }
    Ok(())
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}
