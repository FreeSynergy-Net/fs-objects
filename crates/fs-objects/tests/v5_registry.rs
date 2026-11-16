//! R1 (a program registers a domain with schema version and steps) and R2 (column types are checked on write).
#![cfg(feature = "v5")]
#![allow(clippy::many_single_char_names)]

use fs_objects::v5::spec::Logical::{
    Bool, Date, Enum, Int, Iri, Local, Real, Ref, Text, Utc, Zone,
};
use fs_objects::v5::{Col, Domain, Error, Step, Store, Table};
use fs_types::BasicValue;
use turso::Value;

const ONE: Table = Table {
    name: "tasks_task",
    cols: &[Col::req("id", Ref), Col::req("title", Text)],
    constraints: &["PRIMARY KEY (id)"],
    indexes: &[],
};
const OTHER: Table = Table {
    name: "tasks_label",
    cols: &[Col::req("id", Ref)],
    constraints: &["PRIMARY KEY (id)"],
    indexes: &[],
};
const OUTSIDE: Table = Table {
    name: "other_thing",
    cols: &[Col::req("id", Ref)],
    constraints: &["PRIMARY KEY (id)"],
    indexes: &[],
};
const TASKS: Domain = Domain {
    prefix: "tasks",
    about: "tasks",
    tables: &[ONE],
    channels: &[],
    children: &[],
    expiries: &[],
    steps: &[Step::Create(&[ONE])],
};
const SHOP: Domain = Domain {
    prefix: "shop",
    about: "shop",
    tables: &[OTHER_SHOP],
    channels: &[],
    children: &[],
    expiries: &[],
    steps: &[Step::Create(&[OTHER_SHOP])],
};
const OTHER_SHOP: Table = Table {
    name: "shop_item",
    cols: &[Col::req("id", Ref)],
    constraints: &["PRIMARY KEY (id)"],
    indexes: &[],
};

fn is_invalid<T>(r: &Result<T, Error>, field: &str) -> bool {
    matches!(r, Err(Error::Invalid(p)) if p.iter().any(|x| x.field.contains(field)))
}

#[tokio::test]
async fn r1_two_domains_idempotent_and_clashes_refused() {
    let s = Store::open(":memory:", 0).await.unwrap();
    s.register_domain(&TASKS, 5).await.unwrap();
    s.register_domain(&TASKS, 6).await.unwrap();
    let second = s.register_domain(&SHOP, 5).await;
    assert_eq!(
        second,
        Err(Error::SecondDomain {
            existing: "tasks".into(),
            new: "shop".into()
        })
    );
    let c = s.read().await;
    let doms = c
        .query(
            "SELECT prefix, version FROM fs_domain ORDER BY prefix",
            Vec::new(),
        )
        .await
        .unwrap();
    assert_eq!(doms.len(), 2, "fs_ and tasks");
    assert_eq!(
        doms[1],
        vec![Value::Text("tasks".into()), Value::Integer(1)]
    );
    let cols = c
        .query(
            "SELECT logical, derived FROM fs_column WHERE domain = 'tasks' AND col = 'title'",
            Vec::new(),
        )
        .await
        .unwrap();
    assert_eq!(cols[0], vec![Value::Text("text".into()), Value::Integer(0)]);
    drop(c);
    let more = Domain {
        tables: &[ONE, OTHER],
        ..TASKS
    };
    assert!(
        is_invalid(&s.register_domain(&more, 7).await, "prefix"),
        "other table list under a used prefix"
    );
    for reserved in ["object", "activity", "person", "fs", "", "Bad", "x_"] {
        let d = Domain {
            prefix: reserved,
            ..TASKS
        };
        assert!(
            is_invalid(&s.register_domain(&d, 7).await, "prefix"),
            "{reserved}"
        );
    }
    let outside = Domain {
        prefix: "shop2",
        tables: &[OUTSIDE],
        channels: &[],
        children: &[],
        expiries: &[],
        steps: &[Step::Create(&[OUTSIDE])],
        ..SHOP
    };
    assert!(
        is_invalid(&s.register_domain(&outside, 7).await, "tables"),
        "a table outside the prefix"
    );
    let none = Domain { steps: &[], ..SHOP };
    assert!(is_invalid(&s.register_domain(&none, 7).await, "steps"));
}

#[tokio::test]
async fn r1_opening_the_same_file_twice_is_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("r1.db");
    let p = path.to_str().unwrap();
    for t in 0..2 {
        let s = Store::open(p, t).await.unwrap();
        s.register_domain(&TASKS, t).await.unwrap();
        let rows = s
            .read()
            .await
            .query("SELECT value FROM meta WHERE key = 'store_id'", Vec::new())
            .await
            .unwrap();
        assert_eq!(rows.len(), 1, "one store id, made once");
    }
}

const TYPED: Table = Table {
    name: "tasks_typed",
    cols: &[
        Col::req("id", Ref),
        Col::req("u", Utc),
        Col::req("l", Local),
        Col::req("d", Date),
        Col::req("z", Zone),
        Col::req("e", Enum(&["a", "b"])),
        Col::req("i", Iri),
        Col::opt("r", Real),
        Col::req("b", Bool),
        Col::req("n", Int),
    ],
    constraints: &["PRIMARY KEY (id)"],
    indexes: &[],
};
const TYPED_DOMAIN: Domain = Domain {
    prefix: "tasks",
    about: "t",
    tables: &[TYPED],
    channels: &[],
    children: &[],
    expiries: &[],
    steps: &[Step::Create(&[TYPED])],
};
const ID: &str = "01927f00-0000-7000-8000-000000000001";

fn good() -> Vec<Value> {
    vec![
        ID.into(),
        Value::Integer(1_000),
        Value::Integer(86_400),
        Value::Integer(20_000),
        "Europe/Berlin".into(),
        "a".into(),
        "urn:uuid:x".into(),
        Value::Real(1.5),
        Value::Integer(1),
        Value::Integer(-7),
    ]
}

async fn put(s: &Store, row: Vec<Value>) -> Result<(), Error> {
    let c = s.begin().await?;
    let r = c.insert(&TYPED, row).await;
    c.finish(r).await
}

#[tokio::test]
async fn r2_each_type_stores_a_good_value_and_refuses_a_bad_one_naming_the_field() {
    let s = Store::open(":memory:", 0).await.unwrap();
    s.register_domain(&TYPED_DOMAIN, 0).await.unwrap();
    put(&s, good()).await.unwrap();
    let back = s
        .read()
        .await
        .query(
            "SELECT u, l, d, z, e, i, r, b, n FROM tasks_typed",
            Vec::new(),
        )
        .await
        .unwrap();
    assert_eq!(
        back[0][..4],
        [
            Value::Integer(1_000),
            Value::Integer(86_400),
            Value::Integer(20_000),
            Value::Text("Europe/Berlin".into())
        ]
    );
    assert_eq!(back[0][6], Value::Real(1.5));
    let bad: [(usize, Value, &str); 13] = [
        (0, "not-a-uuid".into(), "id"),
        (2, Value::Integer(i64::MAX), "l"),
        (3, Value::Integer(i64::MAX), "d"),
        (4, "".into(), "z"),
        (4, "/Custom/x".into(), "z"),
        (5, "c".into(), "e"),
        (6, "no scheme".into(), "i"),
        (7, Value::Real(f64::NAN), "r"),
        (7, Value::Real(f64::INFINITY), "r"),
        (8, Value::Integer(2), "b"),
        (9, Value::Null, "n"),
        (1, "text".into(), "u"),
        (3, "x".into(), "d"),
    ];
    for (i, v, field) in bad {
        let mut row = good();
        row[0] = format!("01927f00-0000-7000-8000-0000000000{:02}", i + 10)
            .as_str()
            .into();
        row[i] = v.clone();
        let r = put(&s, row).await;
        assert!(is_invalid(&r, field), "{field} {v:?}: {r:?}");
    }
    let n = s
        .read()
        .await
        .query("SELECT 1 FROM tasks_typed", Vec::new())
        .await
        .unwrap()
        .len();
    assert_eq!(n, 1, "nothing of the refused rows was stored");
    assert!(
        fs_types::Date::parse("2026-02-30").is_err(),
        "30 February is not a date"
    );
}

#[tokio::test]
async fn a_row_reports_one_problem_for_every_bad_field() {
    let s = Store::open(":memory:", 0).await.unwrap();
    s.register_domain(&TYPED_DOMAIN, 0).await.unwrap();
    let mut row = good();
    row[1] = "text".into();
    row[5] = "c".into();
    row[8] = Value::Integer(2);
    let Err(Error::Invalid(p)) = put(&s, row).await else {
        panic!("refused")
    };
    let fields: Vec<&str> = p.iter().map(|x| x.field.as_str()).collect();
    assert_eq!(fields, ["tasks_typed.u", "tasks_typed.e", "tasks_typed.b"]);
}
