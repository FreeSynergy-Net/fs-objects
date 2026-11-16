//! The activity kinds of v0.0.5 (`Add`, `Remove`, `Join`, `Leave`, `Move`): a word table filled by appended steps, the
//! log pointing at it, steps 1 to 3 unchanged, and an older log kept when step 7 rebuilds it.
#![cfg(feature = "v5")]

mod common_v5;

use common_v5::{actor, count, id, store};
use fs_objects::v5::envelope::insert;
use fs_objects::v5::log::{Kind, log};
use fs_objects::v5::own::steps;
use fs_objects::v5::tables::{ACTIVITY, ACTIVITY_CHANGE};
use fs_objects::v5::tables_kind::{ACTIVITY_KINDS, WORDS_AS2, WORDS_OLD, rebuild_activity};
use fs_objects::v5::{Step, Store};

const T0: i64 = 1_794_823_200_000;
const ORPHANS: &str = "SELECT name FROM sqlite_schema WHERE type='index' AND sql IS NULL \
    AND name NOT LIKE 'sqlite_autoindex_' || tbl_name || '_%'";

const AS2: [Kind; 5] = [Kind::Add, Kind::Remove, Kind::Join, Kind::Leave, Kind::Move];

#[test]
fn the_kind_words_are_the_thirteen_as2_names_and_round_trip() {
    assert_eq!(ACTIVITY_KINDS.len(), 13);
    for (w, k) in ["Add", "Remove", "Join", "Leave", "Move"].iter().zip(AS2) {
        assert_eq!(k.word(), *w);
        assert_eq!(Kind::from_word(w).unwrap(), k);
    }
    for w in ACTIVITY_KINDS {
        assert_eq!(Kind::from_word(w).unwrap().word(), w);
    }
    assert!(Kind::from_word("Like").is_err());
}

#[test]
fn steps_one_to_three_keep_their_fingerprints_and_the_new_ones_are_appended() {
    let prints: Vec<String> = steps().iter().map(Step::fingerprint).collect();
    assert_eq!(prints.len(), 7);
    assert_eq!(
        &prints[..3],
        &[
            "0a2bb7c0baeec7c231fe9e117768380b2447a8e6f892a13d541c7d5ef79ebf88",
            "172bae5548ed99c15885807c4d3714df4b5bbdc79db298893c29aca020cad30f",
            "e85ab7de6c18ec9bc3c3e082be9a5fa2ffc038b6287dc2592bd2313599a387bf"
        ]
    );
}

#[tokio::test]
async fn every_kind_is_written_and_read_back_and_the_word_table_is_filled() {
    let s = store(&[]).await;
    let c = s.begin().await.unwrap();
    let o = id(T0);
    insert(&c, o, "Note", "n", &actor(), T0).await.unwrap();
    for (i, k) in AS2.into_iter().enumerate() {
        log(&c, o, k, &actor(), T0 + i64::try_from(i).unwrap(), &[])
            .await
            .unwrap();
    }
    c.finish(Ok(())).await.unwrap();
    assert_eq!(count(&s, "fs_word").await, 13);
    let r = s.read().await;
    let rows = r
        .query("SELECT kind FROM activity ORDER BY id", Vec::new())
        .await
        .unwrap();
    let got: Vec<String> = rows
        .iter()
        .map(|x| fs_objects::v5::codec::get_text(x, 0).unwrap())
        .collect();
    assert_eq!(got, ["Add", "Remove", "Join", "Leave", "Move"]);
}

#[tokio::test]
async fn a_word_that_is_none_is_refused_by_the_foreign_key() {
    let s = store(&[]).await;
    let c = s.begin().await.unwrap();
    insert(&c, id(T0), "Note", "n", &actor(), T0).await.unwrap();
    let bad = c
        .run(
            "INSERT INTO activity (id,kind,object,actor_id,actor_art,at,synced,redacted,explained,kind_set) \
             VALUES ('a','Like','x','urn:uuid:01927f00-0000-7000-8000-0000000000a1','person',1,0,0,1,'activity_kind')",
        )
        .await;
    assert!(bad.is_err());
}

#[tokio::test]
async fn an_older_log_survives_the_rebuild_of_step_seven() {
    let s = Store::open(":memory:", 0).await.unwrap();
    let c = s.begin().await.unwrap();
    // Put the log back into its pre-0.0.5 shape (step 1), with a row and a change in it.
    c.run("DROP TABLE activity_change").await.unwrap();
    c.run("DROP TABLE activity").await.unwrap();
    c.run(&ACTIVITY.ddl()).await.unwrap();
    c.run(&ACTIVITY_CHANGE.ddl()).await.unwrap();
    let o = id(T0);
    insert(&c, o, "Note", "n", &actor(), T0).await.unwrap();
    let ins = "INSERT INTO activity (id,kind,object,actor_id,actor_art,at,synced,redacted,explained) \
        VALUES ('a1','Update',?,'urn:uuid:01927f00-0000-7000-8000-0000000000a1','person',1,0,0,1)";
    c.exec(ins, vec![fs_objects::v5::codec::id_value(o)])
        .await
        .unwrap();
    c.run("INSERT INTO activity_change (activity,seq,field,value_kind,new_int) VALUES ('a1',0,'f','int',7)")
        .await
        .unwrap();
    rebuild_activity(&c).await.unwrap();
    let k = c
        .query("SELECT kind, kind_set FROM activity", Vec::new())
        .await
        .unwrap();
    assert_eq!(k.len(), 1);
    assert_eq!(fs_objects::v5::codec::get_text(&k[0], 0).unwrap(), "Update");
    assert_eq!(
        fs_objects::v5::codec::get_text(&k[0], 1).unwrap(),
        "activity_kind"
    );
    assert_eq!(
        c.query("SELECT 1 FROM activity_change", Vec::new())
            .await
            .unwrap()
            .len(),
        1
    );
    c.finish(Ok(())).await.unwrap();
}

#[tokio::test]
async fn every_autoindex_belongs_to_its_table_fresh_and_after_the_rebuild() {
    let s = store(&[]).await;
    assert_eq!(
        s.read()
            .await
            .query(ORPHANS, Vec::new())
            .await
            .unwrap()
            .len(),
        0
    );
    let c = s.begin().await.unwrap();
    c.run("DROP TABLE activity_change").await.unwrap();
    c.run("DROP TABLE activity").await.unwrap();
    c.run(&ACTIVITY.ddl()).await.unwrap();
    c.run(&ACTIVITY_CHANGE.ddl()).await.unwrap();
    rebuild_activity(&c).await.unwrap();
    assert_eq!(c.query(ORPHANS, Vec::new()).await.unwrap().len(), 0);
    c.finish(Ok(())).await.unwrap();
}

#[tokio::test]
async fn the_insert_steps_turn_an_own_word_of_the_same_name_into_a_builtin() {
    // Step 5 (the eight old kinds) and step 6 (the five AS2 kinds) both end in `ON CONFLICT ... SET builtin = 1`.
    for (sql, code) in [(WORDS_OLD, "Create"), (WORDS_AS2, "Join")] {
        let s = store(&[]).await;
        let c = s.begin().await.unwrap();
        c.run(&format!(
            "UPDATE fs_word SET builtin = 0 WHERE code = '{code}'"
        ))
        .await
        .unwrap();
        let r = c
            .query(
                &format!("SELECT builtin FROM fs_word WHERE code = '{code}'"),
                Vec::new(),
            )
            .await
            .unwrap();
        assert_eq!(fs_objects::v5::codec::get_int(&r[0], 0).unwrap(), 0);
        c.run(sql).await.unwrap();
        let r = c
            .query(
                &format!("SELECT builtin FROM fs_word WHERE code = '{code}'"),
                Vec::new(),
            )
            .await
            .unwrap();
        assert_eq!(
            fs_objects::v5::codec::get_int(&r[0], 0).unwrap(),
            1,
            "{code}"
        );
        c.finish(Ok(())).await.unwrap();
    }
}
