use super::*;
use crate::{
    db::{
        Database,
        testkit::{at, mem},
    },
    error::Error,
    matching::testkit::{FULL_AD, add},
    ranking::{RANKING_VERSION, score::Component},
};

fn ranked(property_id: i64, score: Option<f64>, confidence: Confidence) -> Ranked {
    Ranked {
        property_id,
        score,
        confidence,
        details: ScoreDetails {
            ranking_version: RANKING_VERSION.to_owned(),
            comparable_price_centavos: Some(45_000_000),
            components: Vec::<Component>::new(),
            available_weight_share: 0.6,
            confidence_reasons: vec![],
        },
    }
}

fn properties(db: &Database, n: u32) -> Vec<i64> {
    (1..=n)
        .map(|i| add(db, "alpha", i, "Casa", FULL_AD).1)
        .collect()
}

fn save(db: &Database, ranked: &[Ranked], secs: i64) -> SaveReport {
    db.with_conn(|c| save_ranking(c, ranked, at(secs))).unwrap()
}

fn stored(db: &Database, id: i64) -> StoredScore {
    db.with_conn(|c| load_score(c, id))
        .unwrap()
        .expect("property exists")
}

#[test]
fn a_score_is_saved_with_its_confidence_details_version_and_time() {
    let db = mem();
    let ids = properties(&db, 2);
    let report = save(&db, &[ranked(ids[0], Some(72.5), Confidence::Medium)], 100);
    assert_eq!((report.scored, report.unscored), (1, 0));
    let s = stored(&db, ids[0]);
    assert_eq!(s.score, Some(72.5));
    assert_eq!(s.confidence, Some(Confidence::Medium));
    assert_eq!(s.ranking_version.as_deref(), Some(RANKING_VERSION));
    assert_eq!(s.scored_at, Some(at(100)));
    assert_eq!(
        s.details.unwrap().comparable_price_centavos,
        Some(45_000_000)
    );
}

#[test]
fn an_unscored_property_keeps_its_explanation_but_has_no_score() {
    let db = mem();
    let ids = properties(&db, 1);
    let report = save(&db, &[ranked(ids[0], None, Confidence::Low)], 100);
    assert_eq!((report.scored, report.unscored), (0, 1));
    let s = stored(&db, ids[0]);
    assert_eq!((s.score, s.confidence), (None, Some(Confidence::Low)));
    assert!(s.details.is_some(), "why there is no score is stored");
}

#[test]
fn properties_that_are_no_longer_ranked_are_cleared_not_left_with_a_stale_score() {
    let db = mem();
    let ids = properties(&db, 3);
    save(
        &db,
        &[
            ranked(ids[0], Some(10.0), Confidence::High),
            ranked(ids[1], Some(20.0), Confidence::High),
            ranked(ids[2], Some(30.0), Confidence::High),
        ],
        100,
    );
    let report = save(&db, &[ranked(ids[1], Some(55.0), Confidence::Low)], 200);
    assert_eq!((report.scored, report.cleared), (1, 2));
    assert_eq!(stored(&db, ids[1]).score, Some(55.0));
    for id in [ids[0], ids[2]] {
        let s = stored(&db, id);
        assert_eq!(
            (
                s.score,
                s.confidence,
                s.details,
                s.ranking_version,
                s.scored_at
            ),
            (None, None, None, None, None)
        );
    }
}

#[test]
fn saving_the_same_ranking_twice_changes_nothing_but_the_time() {
    let db = mem();
    let ids = properties(&db, 2);
    let r = [
        ranked(ids[0], Some(40.0), Confidence::Medium),
        ranked(ids[1], Some(60.0), Confidence::High),
    ];
    save(&db, &r, 100);
    let first = (stored(&db, ids[0]), stored(&db, ids[1]));
    save(&db, &r, 200);
    let again = (stored(&db, ids[0]), stored(&db, ids[1]));
    assert_eq!(
        (first.0.score, first.1.score),
        (again.0.score, again.1.score)
    );
    assert_eq!(again.0.scored_at, Some(at(200)));
}

#[test]
fn a_failure_midway_leaves_every_score_as_it_was() {
    let db = mem();
    let ids = properties(&db, 3);
    save(
        &db,
        &[
            ranked(ids[0], Some(11.0), Confidence::High),
            ranked(ids[1], Some(22.0), Confidence::High),
        ],
        100,
    );
    db.with_conn(|c| {
        c.execute_batch(
            "CREATE TRIGGER test_refuse_99 BEFORE UPDATE OF local_score ON properties
             WHEN NEW.local_score = 99.0 BEGIN SELECT RAISE(ABORT, 'refused'); END;",
        )?;
        Ok(())
    })
    .unwrap();
    let error = db
        .with_conn(|c| {
            save_ranking(
                c,
                &[
                    ranked(ids[0], Some(50.0), Confidence::Low),
                    ranked(ids[2], Some(99.0), Confidence::Low),
                ],
                at(200),
            )
        })
        .unwrap_err();
    assert!(matches!(error, Error::Database(_)), "{error}");
    assert_eq!(
        stored(&db, ids[0]).score,
        Some(11.0),
        "the first update was rolled back"
    );
    assert_eq!(
        stored(&db, ids[1]).score,
        Some(22.0),
        "the clearing was rolled back"
    );
    assert_eq!(stored(&db, ids[2]).score, None);
}

#[test]
fn an_unknown_property_has_no_stored_score_and_saving_for_it_is_ignored() {
    let db = mem();
    properties(&db, 1);
    assert_eq!(db.with_conn(|c| load_score(c, 999)).unwrap(), None);
    let report = save(&db, &[ranked(999, Some(50.0), Confidence::Low)], 100);
    assert_eq!(
        report.scored, 0,
        "nothing was written for a property that does not exist"
    );
}
