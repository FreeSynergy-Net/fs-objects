//! Helpers for tests (feature `testing`): a fixed owner, so a test opens its store with one line.
//!
//! Pattern: Object Mother — one ready-made, valid value that tests share instead of each building its own.

use fs_types::{BasicValue as _, PersonArt, PersonRef, Reference};

use crate::OpenOptions;

/// A fixed test owner: a person with a `UUIDv7` id (`urn:uuid:01900000-0000-7000-8000-00000000000a`).
///
/// # Panics
/// Never: the constant is a valid `urn:uuid`.
pub fn owner() -> PersonRef {
    PersonRef {
        id: Reference::parse("urn:uuid:01900000-0000-7000-8000-00000000000a")
            .expect("the test owner is a valid urn:uuid"),
        art: PersonArt::Person,
        abzug: None,
    }
}

/// Options for a test store: [`owner`] set, sensitive objects allowed without encryption.
pub fn options() -> OpenOptions {
    OpenOptions {
        allow_sensitive_unencrypted: true,
        ..OpenOptions::default()
    }
    .with_owner(owner())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_owner_is_a_v7_person() {
        let o = owner();
        assert_eq!(o.art, PersonArt::Person);
        assert!(o.id.canonical().starts_with("urn:uuid:01900000-0000-7"));
    }
}
