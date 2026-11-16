//! A typed log value as the column group of `activity_change`, and back (ADR 0086 item 2).
//!
//! Pattern: Adapter (one column group per value kind; no JSON, no blob).

use fs_types::{
    Attendee, BasicValue, Date, LocalDateTime, ObjectId, PersonRef, Reference, Utc, Zone,
};
use turso::Value;

use super::codec::{
    Row, art_of, damaged, date_to_int, get_int, get_opt_text, get_text, id_text, int_to_date,
    int_to_local, local_to_int, parse_id, person_of, person_values, reference_of, text, zone_of,
};
use super::error::Result;

/// Number of columns of one side (old or new).
pub const SIDE: usize = 10;

/// A typed value in the log. `Object` is an object id in the own store, `Reference` a foreign or cross-program IRI;
/// enumerations are `Text` with their canonical word.
#[derive(Clone, Debug, PartialEq)]
pub enum LogValue {
    /// A whole number.
    Int(i64),
    /// A finite number.
    Real(f64),
    /// Text.
    Text(String),
    /// A flag.
    Bool(bool),
    /// An instant.
    Utc(Utc),
    /// A wall-clock time.
    Local(LocalDateTime),
    /// A date.
    Date(Date),
    /// A zone.
    Zone(Zone),
    /// An object of the own store.
    Object(ObjectId),
    /// A foreign reference.
    Reference(Reference),
    /// A person.
    Person(PersonRef),
    /// An attendee (person, resource or location).
    Attendee(Attendee),
}

/// The kind word and the ten columns of a value (`None` = the side is absent: all null).
///
/// # Errors
/// `Invalid` for a time or date that does not exist.
pub fn encode(v: Option<&LogValue>) -> Result<(Option<&'static str>, [Value; SIDE])> {
    let mut c: [Value; SIDE] = std::array::from_fn(|_| Value::Null);
    let Some(v) = v else { return Ok((None, c)) };
    let kind = match v {
        LogValue::Int(i) => {
            c[0] = Value::Integer(*i);
            "int"
        }
        LogValue::Bool(b) => {
            c[0] = Value::Integer(i64::from(*b));
            "bool"
        }
        LogValue::Utc(u) => {
            c[0] = Value::Integer(u.millis());
            "utc"
        }
        LogValue::Local(l) => {
            c[0] = Value::Integer(local_to_int(l)?);
            "local"
        }
        LogValue::Date(d) => {
            c[0] = Value::Integer(date_to_int(d));
            "date"
        }
        LogValue::Real(f) => {
            c[1] = Value::Real(*f);
            "real"
        }
        LogValue::Text(t) => {
            c[2] = text(t);
            "text"
        }
        LogValue::Zone(z) => {
            c[2] = text(z.id());
            "zone"
        }
        LogValue::Object(o) => {
            c[2] = text(&id_text(*o));
            "object"
        }
        LogValue::Reference(r) => {
            c[2] = text(&r.canonical());
            "iri"
        }
        LogValue::Person(p) => {
            let [a, b, d] = person_values(p);
            (c[3], c[4], c[5]) = (a, b, d);
            "person"
        }
        LogValue::Attendee(Attendee::Actor(p)) => {
            let [a, b, d] = person_values(p);
            c[6] = text("actor");
            (c[7], c[8], c[9]) = (a, b, d);
            "attendee"
        }
        LogValue::Attendee(Attendee::Resource(r)) => {
            (c[6], c[7]) = (text("resource"), text(&r.canonical()));
            "attendee"
        }
        LogValue::Attendee(Attendee::Location(r)) => {
            (c[6], c[7]) = (text("location"), text(&r.canonical()));
            "attendee"
        }
    };
    Ok((Some(kind), c))
}

/// The value of a kind word and ten columns starting at `at` of `row`; all null = absent.
///
/// # Errors
/// `Unavailable` for a row that does not read back.
pub fn decode(kind: &str, row: &[Value], at: usize) -> Result<Option<LogValue>> {
    let r: Row = row
        .get(at..at + SIDE)
        .ok_or_else(|| damaged("change"))?
        .to_vec();
    if r.iter().all(|v| matches!(v, Value::Null)) {
        return Ok(None);
    }
    Ok(Some(match kind {
        "int" => LogValue::Int(get_int(&r, 0)?),
        "bool" => LogValue::Bool(get_int(&r, 0)? != 0),
        "utc" => LogValue::Utc(Utc::from_millis(get_int(&r, 0)?)),
        "local" => LogValue::Local(int_to_local(get_int(&r, 0)?)?),
        "date" => LogValue::Date(int_to_date(get_int(&r, 0)?)?),
        "real" => match r.get(1) {
            Some(Value::Real(f)) => LogValue::Real(*f),
            _ => return Err(damaged("real")),
        },
        "text" => LogValue::Text(get_text(&r, 2)?),
        "zone" => LogValue::Zone(zone_of(&get_text(&r, 2)?)?),
        "object" => LogValue::Object(parse_id(&get_text(&r, 2)?).ok_or_else(|| damaged("object"))?),
        "iri" => LogValue::Reference(reference_of(&get_text(&r, 2)?)?),
        "person" => LogValue::Person(person_of(&r, 3)?),
        "attendee" => LogValue::Attendee(attendee(&r)?),
        _ => return Err(damaged("value kind")),
    }))
}

fn attendee(r: &Row) -> Result<Attendee> {
    let who = reference_of(&get_text(r, 7)?)?;
    match get_text(r, 6)?.as_str() {
        "actor" => {
            let abzug = match get_opt_text(r, 9)? {
                Some(t) => Some(parse_id(&t).ok_or_else(|| damaged("abzug"))?),
                None => None,
            };
            Ok(Attendee::Actor(PersonRef {
                id: who,
                art: art_of(&get_text(r, 8)?)?,
                abzug,
            }))
        }
        "resource" => Ok(Attendee::Resource(who)),
        "location" => Ok(Attendee::Location(who)),
        _ => Err(damaged("attendee kind")),
    }
}
