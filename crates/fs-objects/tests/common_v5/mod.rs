//! A small demo domain for the tests of the shared layer: a note (with an object envelope) and a link to a child.
#![allow(dead_code)]

use fs_objects::v5::spec::Logical::{Ref, Text, Utc};
use fs_objects::v5::{Child, Col, Domain, Expiry, Step, Store, Table, new_id};
use fs_types::{BasicValue, ObjectId, PersonArt, PersonRef, Reference};

pub const NOTE: Table = Table {
    name: "demo_note",
    cols: &[
        Col::req("id", Ref),
        Col::req("object", Ref),
        Col::req("body", Text),
    ],
    constraints: &[
        "PRIMARY KEY (id)",
        "FOREIGN KEY (object) REFERENCES objects(id) ON DELETE CASCADE",
    ],
    indexes: &[],
};
pub const LINK: Table = Table {
    name: "demo_link",
    cols: &[Col::req("master", Ref), Col::req("child", Ref)],
    constraints: &[
        "PRIMARY KEY (master, child)",
        "FOREIGN KEY (master) REFERENCES objects(id) ON DELETE CASCADE",
        "FOREIGN KEY (child) REFERENCES objects(id) ON DELETE CASCADE",
    ],
    indexes: &[],
};
pub const CUT: Table = Table {
    name: "demo_cut",
    cols: &[Col::req("note", Ref), Col::req("until", Utc)],
    constraints: &["PRIMARY KEY (note)"],
    indexes: &[],
};
const STEPS: [Step; 1] = [Step::Create(&[NOTE, LINK, CUT])];

pub fn domain(expiries: &'static [Expiry]) -> Domain {
    Domain::new("demo", "demo", &[NOTE, LINK, CUT], &STEPS)
        .with_channels(&["import", "carddav"])
        .with_children(&[Child {
            table: "demo_link",
            parent: "master",
            object: "child",
        }])
        .with_expiries(expiries)
}

pub async fn store(expiries: &'static [Expiry]) -> Store {
    let s = Store::open(":memory:", 0).await.unwrap();
    s.register_domain(&domain(expiries), 0).await.unwrap();
    s
}

pub fn actor() -> PersonRef {
    PersonRef {
        id: Reference::parse("urn:uuid:01927f00-0000-7000-8000-0000000000a1").unwrap(),
        art: PersonArt::Person,
        abzug: None,
    }
}

pub fn id(now: i64) -> ObjectId {
    new_id(now).unwrap()
}

pub async fn count(s: &Store, table: &str) -> usize {
    s.read()
        .await
        .query(&format!("SELECT 1 FROM {table}"), Vec::new())
        .await
        .unwrap()
        .len()
}
