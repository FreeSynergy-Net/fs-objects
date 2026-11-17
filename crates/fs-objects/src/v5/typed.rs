//! R2: the checks on write for the logical column types (the engine checks only the storage class).
//!
//! Pattern: Strategy (one check per logical type, chosen by the registry's column description). Whether an IANA zone
//! name exists is the program's knowledge (it has the zone database); here a zone must only be well formed.

use chrono::DateTime;
use fs_types::{BasicValue, Problem, Reference, Zone};
use turso::Value;

use super::codec::{int_to_date, invalid, parse_id};
use super::error::{Error, Result};
use super::spec::{Col, Logical, Table};

/// Check one value against its column. The error names `table.column`.
///
/// # Errors
/// `Invalid` when the value does not fit the logical type.
pub fn check_value(t: &Table, c: &Col, v: &Value) -> Result<()> {
    problem(t, c, v).map_or(Ok(()), |p| Err(Error::Invalid(vec![p])))
}

/// The problem of one value, if it has one.
fn problem(t: &Table, c: &Col, v: &Value) -> Option<Problem> {
    match check_inner(t, c, v) {
        Err(Error::Invalid(mut p)) if !p.is_empty() => Some(p.remove(0)),
        _ => None,
    }
}

fn check_inner(t: &Table, c: &Col, v: &Value) -> Result<()> {
    let field = format!("{}.{}", t.name, c.name);
    let bad = |why: &str| Err(invalid(&field, why));
    if matches!(v, Value::Null) {
        return if c.null {
            Ok(())
        } else {
            bad("must not be empty")
        };
    }
    match (c.logical, v) {
        (Logical::Int | Logical::Utc, Value::Integer(_))
        | (Logical::Bool, Value::Integer(0 | 1))
        | (Logical::Text | Logical::FreeText, Value::Text(_))
        | (Logical::Blob, Value::Blob(_)) => Ok(()),
        (Logical::Bool, _) => bad("must be 0 or 1"),
        (Logical::Real, Value::Real(f)) if f.is_finite() => Ok(()),
        (Logical::Real, _) => bad("NaN and infinity are refused"),
        (Logical::Local, Value::Integer(s)) if DateTime::from_timestamp(*s, 0).is_some() => Ok(()),
        (Logical::Date, Value::Integer(d)) if int_to_date(*d).is_ok() => Ok(()),
        (Logical::Local | Logical::Date, _) => bad("not a real calendar value"),
        (Logical::Zone, Value::Text(z)) => check_zone(&field, z),
        (Logical::Ref, Value::Text(r)) if parse_id(r).is_some() => Ok(()),
        (Logical::Ref, _) => bad("not an object id"),
        (Logical::Iri, Value::Text(i)) if Reference::parse(i).is_ok() => Ok(()),
        (Logical::Iri, _) => bad("not an absolute IRI"),
        (Logical::Enum(words), Value::Text(w)) if words.contains(&w.as_str()) => Ok(()),
        (Logical::Enum(_), _) => bad("not one of the allowed words"),
        _ => bad("wrong storage class"),
    }
}

fn check_zone(field: &str, name: &str) -> Result<()> {
    if name.starts_with('/') {
        return Err(invalid(field, "custom zones are not stored in this slice"));
    }
    Zone::iana(name)
        .map(|_| ())
        .map_err(|_| invalid(field, "not a zone name"))
}

/// Check a whole row (one value per column, in column order).
///
/// # Errors
/// `Invalid` with one problem for every value that does not fit, or when the number of values is wrong.
pub fn check_row(t: &Table, values: &[Value]) -> Result<()> {
    if values.len() != t.cols.len() {
        return Err(invalid(t.name, "wrong number of values"));
    }
    let all: Vec<Problem> = t
        .cols
        .iter()
        .zip(values)
        .filter_map(|(c, v)| problem(t, c, v))
        .collect();
    if all.is_empty() {
        Ok(())
    } else {
        Err(Error::Invalid(all))
    }
}
