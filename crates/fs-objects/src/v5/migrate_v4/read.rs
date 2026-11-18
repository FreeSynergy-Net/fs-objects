//! Reading the v4 objects page by page. A page that the v4 store refuses (an object written unlogged is never
//! returned by a logged read) is read again object by object, so that EVERY such object is found and listed with its
//! reason, not just the first.
//!
//! Pattern: Iterator over pages with a fallback to single reads (a person sees all problems in one run).

use super::report::{Held, Report};
use crate::v5::codec::id_text;
use crate::v5::id::from_v4;
use crate::v5::{Error, Result};
use crate::{ObjectId, Query, Store as V4Store, StoredObject, TursoStore, UnloggedObjects};

/// Objects per page.
pub(super) const PAGE: usize = 200;

fn unavailable(e: impl std::fmt::Display) -> Error {
    Error::Unavailable(e.to_string())
}

/// The number of objects in the v4 file (the number every other count must add up to).
pub(super) async fn total(v4: &TursoStore) -> Result<usize> {
    V4Store::count(v4, &Query::new()).await.map_err(unavailable)
}

/// One object that the v4 store refuses to return: its id (from the refusal), its type, the reason.
async fn refused(v4: &TursoStore, message: String) -> Held {
    let id = message
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .to_owned();
    let parsed = id.parse::<ObjectId>().ok();
    let kind = match parsed {
        Some(i) => v4
            .read_unlogged(i)
            .await
            .ok()
            .flatten()
            .map_or_else(|| "?".to_owned(), |o| o.envelope.object_type),
        None => "?".to_owned(),
    };
    Held {
        id: parsed.map_or(id, |i| id_text(from_v4(i))),
        kind,
        reason: format!("the v4 store refuses to return it: {message}"),
    }
}

fn page(offset: usize, len: usize) -> Query {
    Query {
        limit: Some(len),
        offset,
        ..Query::new()
    }
}

/// The objects `offset..offset+len` and the objects of that range the v4 store refused (listed, not converted).
pub(super) async fn read_page(
    v4: &TursoStore,
    offset: usize,
    len: usize,
) -> Result<(Vec<StoredObject>, Vec<Held>)> {
    match V4Store::query(v4, &page(offset, len)).await {
        Ok(objects) => Ok((objects, Vec::new())),
        Err(crate::Error::Invalid(_)) => {
            let (mut objects, mut refused_all) = (Vec::new(), Vec::new());
            for i in offset..offset + len {
                match V4Store::query(v4, &page(i, 1)).await {
                    Ok(o) => objects.extend(o),
                    Err(crate::Error::Invalid(m)) => refused_all.push(refused(v4, m).await),
                    Err(e) => return Err(unavailable(e)),
                }
            }
            Ok((objects, refused_all))
        }
        Err(e) => Err(unavailable(e)),
    }
}

/// The explicit count: every object of the v4 file was read (`seen`), and v4 = converted + held back. An object
/// that the v4 store refuses to return (`problems`) stops the run (nothing is switched half), ALL of them listed.
pub(super) fn reconcile(
    report: &mut Report,
    total: usize,
    seen: usize,
    problems: &[Held],
) -> Result<()> {
    report.v4_total = total;
    if !problems.is_empty() {
        let list: Vec<String> = problems
            .iter()
            .map(|p| format!("{} ({}): {}", p.id, p.kind, p.reason))
            .collect();
        return Err(Error::Unavailable(format!(
            "{} of {total} v4 objects cannot be migrated, nothing is switched; all of them: [{}]",
            problems.len(),
            list.join("; ")
        )));
    }
    let done = report.converted_total() + report.held.len();
    if seen != total || done != total {
        return Err(Error::Unavailable(format!(
            "the counts do not add up: the v4 file has {total} objects, {seen} were read, {} converted + {} held back = {done}",
            report.converted_total(),
            report.held.len()
        )));
    }
    Ok(())
}

#[cfg(test)]
#[path = "read_tests.rs"]
mod tests;
