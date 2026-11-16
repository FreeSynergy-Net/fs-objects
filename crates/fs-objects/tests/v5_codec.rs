//! The typed value codec: ids, dates, wall-clock times and persons survive a round trip through the stored form.
#![cfg(feature = "v5")]

use fs_objects::v5::codec::{
    date_to_int, id_text, int_to_date, int_to_local, local_to_int, parse_id, person_of,
    person_values,
};
use fs_types::{BasicValue, Date, LocalDateTime, ObjectId, PersonArt, PersonRef, Reference};

#[test]
fn ids_dates_times_and_persons_round_trip() {
    let id = ObjectId([1, 2, 3, 4, 5, 6, 0x77, 8, 0x89, 10, 11, 12, 13, 14, 15, 16]);
    assert_eq!(parse_id(&id_text(id)), Some(id));
    assert_eq!(parse_id("not-an-id"), None);
    for d in ["1970-01-01", "2026-11-16", "1969-12-31", "2100-02-28"] {
        let date = Date::parse(d).unwrap();
        assert_eq!(int_to_date(date_to_int(&date)).unwrap(), date, "{d}");
    }
    assert_eq!(date_to_int(&Date::parse("1970-01-02").unwrap()), 1);
    let l = LocalDateTime::parse("2026-11-16T09:30:15").unwrap();
    assert_eq!(int_to_local(local_to_int(&l).unwrap()).unwrap(), l);
    assert!(
        int_to_date(i64::MAX).is_err(),
        "a damaged value is an error, not a guess"
    );
    let p = PersonRef {
        id: Reference::parse("urn:uuid:01927f00-0000-7000-8000-000000000001").unwrap(),
        art: PersonArt::Group,
        abzug: Some(id),
    };
    let v = person_values(&p);
    assert_eq!(person_of(&v.to_vec(), 0).unwrap(), p);
}
