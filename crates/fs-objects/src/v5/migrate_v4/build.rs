//! Building the new v5 file: per object one transaction (envelope, rows, first log entry, round trip). A round trip
//! that does not match rolls the object back and holds it; nothing is dropped silently.
//!
//! Pattern: Transaction script (one unit of work per object; the outcome is an enum, not an exception).

use std::time::Instant;

use fs_types::{BasicValue, PersonArt, PersonRef, Reference};
use turso::Value;

use super::carry::{ORIGINAL_TABLE, not_carried, original_row, verify_extras};
use super::convert::{Converted, Converter};
use super::read;
use super::report::{Held, Report};
use crate::v5::codec::{id_text, id_value, int, invalid, opt_text, parse_id, text};
use crate::v5::id::from_v4;
use crate::v5::log::{Kind, log};
use crate::v5::tables::OBJECTS;
use crate::v5::{Conn, Error, Result, Store};
use crate::{Store as V4Store, StoredObject, TursoStore};

/// The actor of the first log entry of an object that has no `attributed_to`.
const MIGRATION_ACTOR: &str = "urn:freesynergy:migration:v4";

enum Outcome {
    Done,
    Held(String),
}

fn micros_to_ms(t: chrono::DateTime<chrono::Utc>, rounded: &mut usize) -> i64 {
    if !t.timestamp_subsec_micros().is_multiple_of(1000) {
        *rounded += 1;
    }
    t.timestamp_millis()
}

fn reference(s: &str) -> std::result::Result<Reference, String> {
    Reference::parse(s).map_err(|_| format!("not an absolute IRI: {s}"))
}

fn ref_col(what: &str, v: Option<&String>) -> std::result::Result<Value, String> {
    match v {
        None => Ok(Value::Null),
        Some(s) if parse_id(s).is_some() => Ok(text(s)),
        Some(s) => Err(format!(
            "{what} is not an object id (v5 stores a reference): {s}"
        )),
    }
}

/// The envelope row of the new file plus the values to compare after the round trip.
fn envelope_row(
    o: &StoredObject,
    rounded: &mut usize,
) -> std::result::Result<(Vec<Value>, Vec<Value>), String> {
    let e = &o.envelope;
    let id = id_text(from_v4(e.id));
    let who = match &e.attributed_to {
        Some(a) => Some(reference(a)?),
        None => None,
    };
    let (published, updated) = (
        micros_to_ms(e.published, rounded),
        micros_to_ms(e.updated, rounded),
    );
    let iri = e.iri.as_deref().map(reference).transpose()?;
    let iri = iri.map(|r| r.canonical());
    let (context, reply, describes) = (
        ref_col("context", e.context.as_ref())?,
        ref_col("in_reply_to", e.in_reply_to.as_ref())?,
        ref_col("describes", e.describes.as_ref())?,
    );
    let who_text = who.as_ref().map(BasicValue::canonical);
    let compare = vec![
        opt_text(iri.as_deref()),
        text(&e.object_type),
        text(&e.name),
        opt_text(who_text.as_deref()),
        int(published),
        int(updated),
        context.clone(),
        reply.clone(),
        describes.clone(),
    ];
    let row = vec![
        text(&id),
        opt_text(iri.as_deref()),
        text(&e.object_type),
        text(&e.name),
        text(&crate::sort_key(&e.name)),
        opt_text(who_text.as_deref()),
        who.as_ref().map_or(Value::Null, |_| text("person")),
        Value::Null,
        int(published),
        int(updated),
        context,
        reply,
        describes,
        int(1),
        Value::Null,
        Value::Null,
    ];
    Ok((row, compare))
}

async fn one(
    store: &Store,
    conv: &dyn Converter,
    o: &StoredObject,
    conv_out: Converted,
    rounded: &mut usize,
) -> Result<Outcome> {
    let (row, expect) = match envelope_row(o, rounded) {
        Ok(v) => v,
        Err(why) => return Ok(Outcome::Held(why)),
    };
    let id = from_v4(o.envelope.id);
    let actor = reference(
        o.envelope
            .attributed_to
            .as_deref()
            .unwrap_or(MIGRATION_ACTOR),
    )
    .map_err(|w| invalid("attributed_to", &w))?;
    let who = PersonRef {
        id: actor,
        art: PersonArt::Person,
        abzug: None,
    };
    let c = store.begin().await?;
    let r = async {
        c.insert(&OBJECTS, row).await?;
        for r in conv_out.rows {
            c.insert(r.table, r.values).await?;
        }
        let at = expect[4].clone();
        let now = if let Value::Integer(n) = at { n } else { 0 };
        if let Some(row) = original_row(o, now) {
            c.insert(ORIGINAL_TABLE, row).await?;
        }
        log(&c, id, Kind::Create, &who, now, &conv_out.changes).await?;
        verify(&c, conv, o, &expect).await
    }
    .await;
    match c.finish(r).await {
        Ok(()) => Ok(Outcome::Done),
        Err(Error::Invalid(p)) => Ok(Outcome::Held(
            p.iter()
                .map(|x| format!("{}: {}", x.field, x.reason))
                .collect::<Vec<_>>()
                .join("; "),
        )),
        Err(e) => Err(e),
    }
}

/// The round trip: the envelope and the content, read back from the new file, must equal the v4 object.
async fn verify(
    c: &Conn<'_>,
    conv: &dyn Converter,
    o: &StoredObject,
    expect: &[Value],
) -> Result<()> {
    let id = from_v4(o.envelope.id);
    let sql = "SELECT iri, type, name, attributed_to_id, published, updated, context, in_reply_to, describes FROM objects WHERE id = ?";
    let back = c.query(sql, vec![id_value(id)]).await?;
    if back.first().map(Vec::as_slice) != Some(expect) {
        return Err(invalid(
            "roundtrip.envelope",
            "the envelope read back differs from the v4 envelope",
        ));
    }
    if let Err(why) = verify_extras(c, conv, o).await {
        return Err(invalid("roundtrip.extras", &why));
    }
    let got = conv.export(c, id).await?;
    if got != conv.normalize(&o.content) {
        return Err(invalid(
            "roundtrip.content",
            "the content rebuilt from the rows differs from the v4 content",
        ));
    }
    Ok(())
}

/// Build the new file at `target` from the v4 store; returns what happened. `v4` is only read.
pub(super) async fn build(
    v4: &TursoStore,
    target: &str,
    conv: &dyn Converter,
    now_ms: i64,
    report: &mut Report,
) -> Result<()> {
    let started = Instant::now();
    let store = Store::open(target, now_ms).await?;
    store.register_domain(&conv.domain(), now_ms).await?;
    let total = read::total(v4).await?;
    let (mut problems, mut seen, mut offset) = (Vec::new(), 0_usize, 0_usize);
    while offset < total {
        let len = read::PAGE.min(total - offset);
        let (objects, refused) = read::read_page(v4, offset, len).await?;
        seen += objects.len() + refused.len();
        problems.extend(refused);
        for o in objects {
            let kind = o.envelope.object_type.clone();
            let id = id_text(from_v4(o.envelope.id));
            let verdict = if let Some(why) = not_carried(&o, conv) {
                Outcome::Held(why)
            } else if !conv.handles(&kind) {
                Outcome::Held(format!("no converter for the type {kind}"))
            } else {
                match conv.to_rows(&o) {
                    Err(h) => Outcome::Held(h.0),
                    Ok(out) => one(&store, conv, &o, out, &mut report.rounded).await?,
                }
            };
            match verdict {
                Outcome::Done => *report.converted.entry(kind).or_default() += 1,
                Outcome::Held(reason) => report.held.push(Held { id, kind, reason }),
            }
            *report
                .skipped
                .entry("search index fields (derived, rebuilt from the typed rows)".into())
                .or_default() += o.index.values().map(Vec::len).sum::<usize>();
        }
        offset += len;
    }
    read::reconcile(report, total, seen, &problems)?;
    let n = V4Store::log_stats(v4)
        .await
        .map_err(|e| Error::Unavailable(e.to_string()))?;
    report.skipped.insert(
        "v4 activity history (carried by a later step)".into(),
        n.entries,
    );
    report.millis = started.elapsed().as_millis();
    Ok(())
}
