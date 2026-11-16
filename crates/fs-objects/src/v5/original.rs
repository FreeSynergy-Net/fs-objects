//! Where an imported object came from (`object_origin`) and the file as it was received (`object_original`, ADR 0030).
//!
//! Pattern: Transaction script. The original is immutable after insert; "unchanged since import" is computed
//! (`rev = rev_at_import`), never stored (ADR 0085). What a program reads back from its own origin rows stays there.

use fs_types::{BasicValue, LocalDateTime, MediaType, ObjectId, Reference, Revision};
use sha2::{Digest, Sha256};
use turso::Value;

use super::codec::{
    damaged, get_text, id_value, int, invalid, local_to_int, opt_int, opt_text, text,
};
use super::conn::Conn;
use super::error::Result;
use super::registry::Domain;
use super::tables_more::{OBJECT_ORIGIN, OBJECT_ORIGINAL};

/// What an import tells the store about the object it made.
#[derive(Clone, Debug)]
pub struct NewOrigin {
    /// The channel, one of `Domain::channels` (general names such as `import`, `sync`, `carddav`).
    pub channel: String,
    /// Where it was fetched from.
    pub source: Reference,
    /// The media type it came in.
    pub media_type: MediaType,
    /// The foreign id of the item.
    pub foreign_uid: String,
    /// The foreign id of one occurrence.
    pub foreign_recurrence_id: Option<LocalDateTime>,
    /// The etag of the foreign item.
    pub foreign_etag: Option<String>,
    /// The producer of the file.
    pub prod_id: Option<String>,
    /// Who imported.
    pub importer: String,
    /// The received file and its type, when it is kept.
    pub original: Option<(Vec<u8>, MediaType)>,
}

/// Lower-case hex SHA-256 of bytes.
#[must_use]
pub fn sha256_hex(bytes: &[u8]) -> String {
    hex32(&Sha256::digest(bytes).into())
}

/// Lower-case hex of 32 bytes.
#[must_use]
pub fn hex32(b: &[u8; 32]) -> String {
    b.iter()
        .map(|x| format!("{x:02x}"))
        .collect::<Vec<_>>()
        .concat()
}

/// The `urn:sha256:<hex>` IRI of a kept original.
#[must_use]
pub fn original_iri(hex: &str) -> String {
    format!("urn:sha256:{hex}")
}

/// Write the origin row and the original; `rev_at_import` is the revision the object has when the import is done.
///
/// # Errors
/// `Invalid` naming `table.column` (also a channel the domain did not report), or the engine's error.
pub async fn record(
    c: &Conn<'_>,
    d: &Domain,
    id: ObjectId,
    o: &NewOrigin,
    rev_at_import: Revision,
    now: i64,
) -> Result<()> {
    if !d.channels.contains(&o.channel.as_str()) {
        return Err(invalid(
            "object_origin.channel",
            "not a channel of this program",
        ));
    }
    let kept = o.original.as_ref().map(|(b, m)| (sha256_hex(b), m));
    if let Some((bytes, media)) = &o.original {
        let hex = sha256_hex(bytes);
        let row = vec![
            id_value(id),
            text(&media.0),
            Value::Blob(bytes.clone()),
            text(&hex),
            int(now),
            Value::Null,
        ];
        c.insert(&OBJECT_ORIGINAL, row).await?;
    }
    let (iri, hex, media) = match &kept {
        Some((h, m)) => (opt_text(Some(&original_iri(h))), text(h), text(&m.0)),
        None => (Value::Null, Value::Null, Value::Null),
    };
    let rev = i64::try_from(rev_at_import.0).unwrap_or(0);
    c.insert(
        &OBJECT_ORIGIN,
        vec![
            id_value(id),
            text(&o.channel),
            text(&o.source.canonical()),
            text(&o.media_type.0),
            text(&o.foreign_uid),
            opt_int(
                o.foreign_recurrence_id
                    .as_ref()
                    .map(local_to_int)
                    .transpose()?,
            ),
            opt_text(o.foreign_etag.as_deref()),
            opt_text(o.prod_id.as_deref()),
            int(now),
            text(&o.importer),
            int(rev),
            iri,
            hex,
            media,
        ],
    )
    .await
}

/// The received file of an object (ADR 0030), `None` when none was kept.
///
/// # Errors
/// `Unavailable` for a row that does not read back.
pub async fn original_of(c: &Conn<'_>, id: ObjectId) -> Result<Option<(Vec<u8>, MediaType)>> {
    let rows = c
        .query(
            "SELECT bytes, media_type FROM object_original WHERE object = ?",
            vec![id_value(id)],
        )
        .await?;
    match rows.first().map(|r| (r.first().cloned(), get_text(r, 1))) {
        Some((Some(Value::Blob(b)), m)) => Ok(Some((b, MediaType(m?)))),
        Some(_) => Err(damaged("original")),
        None => Ok(None),
    }
}
