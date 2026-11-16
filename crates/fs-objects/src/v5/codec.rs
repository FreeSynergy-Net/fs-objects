//! Between values and the engine: ids, rows, readers, and the typed time values of `fs-types`.
//!
//! Pattern: Adapter (typed values to engine values and back). A stored value that cannot be read back is
//! `Unavailable("damaged row ...")`, never a guess.

use chrono::{DateTime, Datelike, NaiveDate, NaiveDateTime, Timelike};
use fs_types::{
    BasicValue, Date, LocalDateTime, ObjectId, PersonArt, PersonRef, Problem, Reference, Zone,
};
use turso::Value;

use super::error::{Error, Result};

/// One engine row.
pub type Row = Vec<Value>;

/// A one-field `Invalid`.
#[must_use]
pub fn invalid(field: &str, reason: &str) -> Error {
    Error::Invalid(vec![Problem {
        field: field.to_owned(),
        reason: reason.to_owned(),
    }])
}

/// A stored value that cannot be read back.
#[must_use]
pub fn damaged(what: &str) -> Error {
    Error::Unavailable(format!("damaged row: {what}"))
}

/// The UUID text of an object id (`8-4-4-4-12`, lower case).
#[must_use]
pub fn id_text(id: ObjectId) -> String {
    let h =
        id.0.iter()
            .map(|b| format!("{b:02x}"))
            .collect::<Vec<_>>()
            .concat();
    format!(
        "{}-{}-{}-{}-{}",
        &h[0..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..32]
    )
}

/// The object id of UUID text; `None` when it is not one.
#[must_use]
pub fn parse_id(text: &str) -> Option<ObjectId> {
    let h: String = text.chars().filter(|c| *c != '-').collect();
    if text.len() != 36 || h.len() != 32 || !h.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let mut out = [0_u8; 16];
    for (i, b) in out.iter_mut().enumerate() {
        *b = u8::from_str_radix(&h[2 * i..2 * i + 2], 16).ok()?;
    }
    Some(ObjectId(out))
}

/// An integer value.
#[must_use]
pub fn int(v: i64) -> Value {
    Value::Integer(v)
}
/// A text value.
#[must_use]
pub fn text(v: &str) -> Value {
    Value::Text(v.to_owned())
}
/// 0 or 1.
#[must_use]
pub fn boolean(v: bool) -> Value {
    Value::Integer(i64::from(v))
}
/// Text or null.
#[must_use]
pub fn opt_text(v: Option<&str>) -> Value {
    v.map_or(Value::Null, text)
}
/// Integer or null.
#[must_use]
pub fn opt_int(v: Option<i64>) -> Value {
    v.map_or(Value::Null, Value::Integer)
}
/// An object id as UUID text.
#[must_use]
pub fn id_value(id: ObjectId) -> Value {
    Value::Text(id_text(id))
}

/// An integer column of a row.
pub fn get_int(row: &Row, i: usize) -> Result<i64> {
    match row.get(i) {
        Some(Value::Integer(v)) => Ok(*v),
        _ => Err(damaged("expected an integer")),
    }
}
/// A text column of a row.
pub fn get_text(row: &Row, i: usize) -> Result<String> {
    match row.get(i) {
        Some(Value::Text(v)) => Ok(v.clone()),
        _ => Err(damaged("expected text")),
    }
}
/// A text column that may be null.
pub fn get_opt_text(row: &Row, i: usize) -> Result<Option<String>> {
    match row.get(i) {
        Some(Value::Null) => Ok(None),
        _ => get_text(row, i).map(Some),
    }
}
/// An integer column that may be null.
pub fn get_opt_int(row: &Row, i: usize) -> Result<Option<i64>> {
    match row.get(i) {
        Some(Value::Null) => Ok(None),
        _ => get_int(row, i).map(Some),
    }
}
/// A boolean column.
pub fn get_bool(row: &Row, i: usize) -> Result<bool> {
    Ok(get_int(row, i)? != 0)
}
/// An object-id column.
pub fn get_id(row: &Row, i: usize) -> Result<ObjectId> {
    parse_id(&get_text(row, i)?).ok_or_else(|| damaged("not an object id"))
}

const EPOCH_DAYS: i64 = 719_163;

fn naive_date(d: Date) -> NaiveDate {
    d.as_naive()
}

/// `LOCAL`: seconds since 1970-01-01T00:00:00, read as wall clock.
pub fn local_to_int(l: &LocalDateTime) -> Result<i64> {
    naive_date(l.date())
        .and_hms_opt(
            u32::from(l.hour()),
            u32::from(l.minute()),
            u32::from(l.second()),
        )
        .map(|n| n.and_utc().timestamp())
        .ok_or_else(|| invalid("time", "no such date or time"))
}
/// The wall-clock value of `LOCAL` seconds.
pub fn int_to_local(v: i64) -> Result<LocalDateTime> {
    let n = DateTime::from_timestamp(v, 0)
        .map(|d| d.naive_utc())
        .ok_or_else(|| damaged("local time out of range"))?;
    let date = int_to_date(i64::from(n.date().num_days_from_ce()) - EPOCH_DAYS)?;
    let h = |x: u32| u8::try_from(x).map_err(|_| damaged("local time"));
    LocalDateTime::new(date, h(n.hour())?, h(n.minute())?, h(n.second())?)
        .map_err(|_| damaged("local time"))
}
/// `DATE`: days since 1970-01-01.
pub fn date_to_int(d: &Date) -> i64 {
    i64::from(naive_date(*d).num_days_from_ce()) - EPOCH_DAYS
}
/// The date of `DATE` days.
pub fn int_to_date(v: i64) -> Result<Date> {
    let days = v
        .checked_add(EPOCH_DAYS)
        .and_then(|d| i32::try_from(d).ok())
        .ok_or_else(|| damaged("date out of range"))?;
    let n =
        NaiveDate::from_num_days_from_ce_opt(days).ok_or_else(|| damaged("date out of range"))?;
    Date::parse(&n.format("%Y-%m-%d").to_string()).map_err(|_| damaged("date out of range"))
}
/// The wall clock as seconds (as-if-UTC) of a naive time.
#[must_use]
pub fn naive_secs(n: NaiveDateTime) -> i64 {
    n.and_utc().timestamp()
}
/// A stored zone name.
pub fn zone_of(text: &str) -> Result<Zone> {
    if text.starts_with('/') {
        Zone::custom(text)
    } else {
        Zone::iana(text)
    }
    .map_err(|_| damaged("zone"))
}
/// A stored reference.
pub fn reference_of(text: &str) -> Result<Reference> {
    Reference::parse(text).map_err(|_| damaged("reference"))
}

const ARTS: [(PersonArt, &str); 4] = [
    (PersonArt::Profile, "profile"),
    (PersonArt::Person, "person"),
    (PersonArt::Group, "group"),
    (PersonArt::Organization, "organization"),
];
/// The stored word of a person kind.
#[must_use]
pub fn art_word(a: PersonArt) -> &'static str {
    ARTS.iter()
        .find(|(x, _)| *x == a)
        .map_or("person", |(_, w)| w)
}
/// The person kind of a stored word.
pub fn art_of(word: &str) -> Result<PersonArt> {
    ARTS.iter()
        .find(|(_, w)| *w == word)
        .map(|(x, _)| *x)
        .ok_or_else(|| damaged("person art"))
}
/// The three columns of a `PERSON` (id, art, abzug).
#[must_use]
pub fn person_values(p: &PersonRef) -> [Value; 3] {
    [
        text(&p.id.canonical()),
        text(art_word(p.art)),
        p.abzug.map_or(Value::Null, id_value),
    ]
}
/// The `PERSON` stored from column `at` on.
pub fn person_of(row: &Row, at: usize) -> Result<PersonRef> {
    let abzug = match row.get(at + 2) {
        Some(Value::Null) | None => None,
        _ => Some(get_id(row, at + 2)?),
    };
    Ok(PersonRef {
        id: reference_of(&get_text(row, at)?)?,
        art: art_of(&get_text(row, at + 1)?)?,
        abzug,
    })
}
