//! R2: the checks on write for the logical column types (the engine checks only the storage class).
//!
//! Pattern: Strategy (one check per logical type, chosen by the registry's column description). Whether an IANA zone
//! name exists is the program's knowledge (it has the zone database); here a zone must only be well formed.

use chrono::DateTime;
use fs_types::{BasicValue, Reference, Zone};
use turso::Value;

use super::codec::{int_to_date, invalid, parse_id};
use super::error::Result;
use super::spec::{Col, Logical, Table};

/// Check one value against its column. The error names `table.column`.
///
/// # Errors
/// `Invalid` when the value does not fit the logical type.
pub fn check_value(t: &Table, c: &Col, v: &Value) -> Result<()> {
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
        | (Logical::Text, Value::Text(_))
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
/// The first value that does not fit, or `Invalid` when the number of values is wrong.
pub fn check_row(t: &Table, values: &[Value]) -> Result<()> {
    if values.len() != t.cols.len() {
        return Err(invalid(t.name, "wrong number of values"));
    }
    t.cols
        .iter()
        .zip(values)
        .try_for_each(|(c, v)| check_value(t, c, v))
}
