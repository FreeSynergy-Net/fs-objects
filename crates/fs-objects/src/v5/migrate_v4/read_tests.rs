//! Pattern: Unit tests of the count (v4 = converted + held back; a problem object stops the run).

use super::reconcile;
use crate::v5::migrate_v4::report::{Held, Report};

fn held(n: usize) -> Vec<Held> {
    (0..n)
        .map(|i| Held {
            id: format!("id{i}"),
            kind: "Note".into(),
            reason: "r".into(),
        })
        .collect()
}

fn report(converted: usize, held_n: usize) -> Report {
    let mut r = Report {
        held: held(held_n),
        ..Report::default()
    };
    r.converted.insert("Note".into(), converted);
    r
}

#[test]
fn equal_counts_pass_and_are_recorded() {
    let mut r = report(3, 2);
    reconcile(&mut r, 5, 5, &[]).unwrap();
    assert_eq!(r.v4_total, 5);
}

#[test]
fn an_object_that_is_neither_converted_nor_held_is_found() {
    let mut r = report(3, 1);
    let err = reconcile(&mut r, 5, 5, &[]).unwrap_err().to_string();
    assert!(
        err.contains("do not add up") && err.contains("3 converted + 1 held back = 4"),
        "{err}"
    );
}

#[test]
fn an_object_that_was_not_read_is_found() {
    let mut r = report(3, 2);
    let err = reconcile(&mut r, 5, 4, &[]).unwrap_err().to_string();
    assert!(err.contains("5 objects, 4 were read"), "{err}");
}

#[test]
fn all_problem_objects_are_named_in_one_error() {
    let mut r = report(2, 0);
    let err = reconcile(&mut r, 5, 5, &held(3)).unwrap_err().to_string();
    assert!(
        err.contains("3 of 5") && err.contains("id0") && err.contains("id1") && err.contains("id2"),
        "{err}"
    );
}
