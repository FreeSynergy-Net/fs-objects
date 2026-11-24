//! `serde` for `Option<PersonRef>` (the type has none) and the stored words of a person kind.
//!
//! Pattern: Adapter — a foreign type (`fs_types::PersonRef`) as `{"id": "...", "art": "..."}`; the `abzug` is not part
//! of an owner and is refused.

use fs_types::{BasicValue as _, PersonArt, PersonRef, Reference};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

const ARTS: [(PersonArt, &str); 4] = [
    (PersonArt::Profile, "profile"),
    (PersonArt::Person, "person"),
    (PersonArt::Group, "group"),
    (PersonArt::Organization, "organization"),
];

/// The stored word of a person kind.
pub(crate) fn art_word(a: PersonArt) -> &'static str {
    ARTS.iter()
        .find(|(x, _)| *x == a)
        .map_or("person", |(_, w)| w)
}

/// The person kind of a stored word.
pub(crate) fn art_of(word: &str) -> Option<PersonArt> {
    ARTS.iter().find(|(_, w)| *w == word).map(|(x, _)| *x)
}

/// A person from its two stored words.
pub(crate) fn person_from(id: &str, art: &str) -> Option<PersonRef> {
    Some(PersonRef {
        id: Reference::parse(id).ok()?,
        art: art_of(art)?,
        abzug: None,
    })
}

#[derive(Serialize, Deserialize)]
struct Text {
    id: String,
    art: String,
}

/// `serde(with = "...")` for `Option<PersonRef>`.
pub(crate) mod option {
    use super::{Deserialize, Deserializer, PersonRef, Serializer, Text, art_word, person_from};
    use fs_types::BasicValue as _;
    use serde::Serialize as _;

    #[allow(clippy::ref_option)]
    pub(crate) fn serialize<S: Serializer>(p: &Option<PersonRef>, s: S) -> Result<S::Ok, S::Error> {
        match p {
            Some(p) => Text {
                id: p.id.canonical(),
                art: art_word(p.art).into(),
            }
            .serialize(s),
            None => s.serialize_none(),
        }
    }

    pub(crate) fn deserialize<'de, D: Deserializer<'de>>(
        d: D,
    ) -> Result<Option<PersonRef>, D::Error> {
        let Some(t) = Option::<Text>::deserialize(d)? else {
            return Ok(None);
        };
        person_from(&t.id, &t.art)
            .map(Some)
            .ok_or_else(|| serde::de::Error::custom("bad person reference"))
    }
}
