use std::sync::Arc;

use tokio::sync::Semaphore;

use super::*;
use crate::{
    db::{
        Database,
        matches::{MatchQuery, accept_match, list_matches, merge_events, reject_match},
        testkit::at,
    },
    error::Error,
    matching::{
        RULE_VERSION,
        testkit::{FULL_AD, add, add_typed, hash_image, place},
    },
    models::{
        characteristics::PropertyType,
        matching::{DecisionStatus, MatchCategory},
    },
};

const H1: u64 = 0x0123_4567_89ab_cdef;
const H2: u64 = 0xf0e1_d2c3_b4a5_9687;

fn deps(db: &Arc<Database>) -> CollectionDeps {
    CollectionDeps {
        db: Arc::clone(db),
        global_limit: Arc::new(Semaphore::new(4)),
        clock: Arc::new(|| at(100)),
    }
}

async fn run(db: &Arc<Database>) -> MatchRunReport {
    run_matching(&deps(db), &MatchConfig::default(), &CancelToken::new())
        .await
        .unwrap()
}

fn matches(db: &Database) -> Vec<crate::db::matches::MatchView> {
    db.with_conn(|c| list_matches(c, &MatchQuery::default()))
        .unwrap()
        .items
}

fn fresh() -> Arc<Database> {
    Arc::new(Database::open_in_memory().unwrap())
}

/// Two ads on different platforms: the same two photos (one re-encoded), the same place, area,
/// rooms and text. Returns `(listing a, property a, listing b, property b)`.
fn the_same_house_twice(db: &Database) -> (i64, i64, i64, i64) {
    let (la, pa) = add_typed(db, "alpha", 1, FULL_AD, PropertyType::House);
    let (lb, pb) = add_typed(db, "beta", 1, FULL_AD, PropertyType::House);
    for p in [pa, pb] {
        place(db, p, "Goiânia", Some("Setor Bueno"));
    }
    hash_image(db, la, "https://img.test/a1.png", H1);
    hash_image(db, la, "https://img.test/a2.png", H2);
    hash_image(db, lb, "https://img.test/b1.png", H1 ^ 0b1);
    hash_image(db, lb, "https://img.test/b2.png", H2 ^ 0b111);
    (la, pa, lb, pb)
}

fn merged_into(db: &Database, property: i64) -> Option<i64> {
    db.with_conn(|c| {
        Ok(c.query_row(
            "SELECT merged_into_property_id FROM properties WHERE id = ?1",
            [property],
            |r| r.get(0),
        )?)
    })
    .unwrap()
}

#[tokio::test]
async fn strongly_matching_ads_are_proposed_for_review_and_never_merged() {
    let db = fresh();
    let (la, pa, lb, pb) = the_same_house_twice(&db);
    let report = run(&db).await;
    assert_eq!(report.rule_version, RULE_VERSION);
    assert_eq!(
        (report.listings, report.candidate_pairs, report.compared),
        (2, 1, 1)
    );
    assert_eq!((report.high, report.possible, report.created), (1, 0, 1));

    let stored = matches(&db);
    assert_eq!(stored.len(), 1);
    assert_eq!((stored[0].a.listing_id, stored[0].b.listing_id), (la, lb));
    assert_eq!(
        (stored[0].category, stored[0].decision_status),
        (MatchCategory::High, DecisionStatus::ReviewRequired)
    );
    assert_eq!(stored[0].evidence.image_pairs.len(), 2);
    assert_eq!(
        stored[0].evidence.conflicts.len(),
        0,
        "{:?}",
        stored[0].evidence.conflicts
    );

    // The most certain case there is, and still nothing was merged or decided on the user's behalf.
    assert_eq!((merged_into(&db, pa), merged_into(&db, pb)), (None, None));
    assert_eq!((stored[0].decision_source, stored[0].merge), (None, None));
    let events = db.with_conn(|c| merge_events(c, stored[0].id)).unwrap();
    assert_eq!(events.len(), 0, "{events:?}");
}

#[tokio::test]
async fn running_again_changes_nothing() {
    let db = fresh();
    the_same_house_twice(&db);
    run(&db).await;
    let again = run(&db).await;
    assert_eq!((again.created, again.updated, again.unchanged), (0, 0, 1));
    assert_eq!(matches(&db).len(), 1);
}

#[tokio::test]
async fn new_evidence_updates_a_proposal_that_has_not_been_decided() {
    let db = fresh();
    let (la, pa) = add_typed(&db, "alpha", 1, FULL_AD, PropertyType::House);
    let (lb, pb) = add_typed(&db, "beta", 1, FULL_AD, PropertyType::House);
    for p in [pa, pb] {
        place(&db, p, "Goiânia", Some("Setor Bueno"));
    }
    hash_image(&db, la, "https://img.test/a1.png", H1);
    hash_image(&db, lb, "https://img.test/b1.png", H1);
    let first = run(&db).await;
    assert_eq!(
        (first.created, first.possible, first.high),
        (1, 1, 0),
        "one photo is only worth a look"
    );
    assert_eq!(matches(&db)[0].category, MatchCategory::Possible);

    hash_image(&db, la, "https://img.test/a2.png", H2);
    hash_image(&db, lb, "https://img.test/b2.png", H2);
    let second = run(&db).await;
    assert_eq!((second.updated, second.high), (1, 1));
    assert_eq!(matches(&db)[0].category, MatchCategory::High);
}

#[tokio::test]
async fn a_pair_the_user_rejected_is_not_compared_or_proposed_again() {
    let db = fresh();
    the_same_house_twice(&db);
    run(&db).await;
    let id = matches(&db)[0].id;
    db.with_conn(|c| reject_match(c, id, None, at(5))).unwrap();
    let again = run(&db).await;
    assert_eq!(
        (again.candidate_pairs, again.skipped_decided, again.compared),
        (1, 1, 0)
    );
    assert_eq!(matches(&db)[0].decision_status, DecisionStatus::Rejected);
}

#[tokio::test]
async fn listings_the_user_merged_are_not_proposed_again() {
    let db = fresh();
    the_same_house_twice(&db);
    run(&db).await;
    let id = matches(&db)[0].id;
    db.with_conn(|c| accept_match(c, id, None, at(5))).unwrap();
    let again = run(&db).await;
    assert_eq!((again.candidate_pairs, again.compared), (0, 0));
    assert_eq!(matches(&db)[0].decision_status, DecisionStatus::Accepted);
}

#[tokio::test]
async fn look_alike_photos_of_unrelated_ads_are_kept_apart_and_not_queued_for_review() {
    let db = fresh();
    let (la, pa) = add(&db, "alpha", 1, "Casa", "Casa 3 quartos");
    let (lb, pb) = add(&db, "beta", 1, "Casa", "Casa 3 quartos");
    place(&db, pa, "Goiânia", Some("Setor Bueno"));
    place(&db, pb, "Aparecida de Goiânia", Some("Garavelo"));
    hash_image(&db, la, "https://img.test/a.png", H1);
    hash_image(&db, lb, "https://img.test/b.png", H1);
    let report = run(&db).await;
    assert_eq!((report.low_stored, report.high, report.possible), (1, 0, 0));
    let stored = matches(&db);
    assert_eq!(stored.len(), 1);
    assert_eq!(
        (stored[0].category, stored[0].decision_status),
        (MatchCategory::Low, DecisionStatus::Candidate)
    );
    assert!(
        stored[0]
            .evidence
            .has_conflict(crate::models::matching::ConflictKind::Municipality)
    );
}

#[tokio::test]
async fn pairs_that_only_share_a_neighbourhood_are_not_stored() {
    let db = fresh();
    for n in 1..=3 {
        let (_, p) = add(&db, "alpha", n, "Casa", "Casa 3 quartos");
        place(&db, p, "Goiânia", Some("Setor Bueno"));
    }
    let report = run(&db).await;
    assert_eq!(
        (
            report.candidate_pairs,
            report.low_discarded,
            report.low_stored
        ),
        (3, 3, 0)
    );
    let leftover = matches(&db);
    assert_eq!(leftover.len(), 0, "{leftover:?}");
}

#[tokio::test]
async fn a_photo_shared_by_many_ads_creates_no_proposals() {
    let db = fresh();
    for n in 1..=4 {
        let (l, _) = add(&db, "alpha", n, "Casa", "Casa 3 quartos");
        hash_image(&db, l, &format!("https://img.test/placeholder-{n}.png"), H1);
    }
    let report = run(&db).await;
    assert_eq!((report.common_hashes, report.candidate_pairs), (1, 0));
    let leftover = matches(&db);
    assert_eq!(leftover.len(), 0, "{leftover:?}");
}

#[tokio::test]
async fn what_was_compared_is_stored_without_any_of_the_words_of_the_ads() {
    let db = fresh();
    let (la, pa) = add_typed(
        &db,
        "alpha",
        1,
        &format!("{FULL_AD}. Falar com Maria 62 99999-0000"),
        PropertyType::House,
    );
    let (lb, pb) = add_typed(&db, "beta", 1, FULL_AD, PropertyType::House);
    for p in [pa, pb] {
        place(&db, p, "Goiânia", Some("Setor Bueno"));
    }
    hash_image(&db, la, "https://img.test/a.png", H1);
    hash_image(&db, lb, "https://img.test/b.png", H1);
    run(&db).await;
    let id = matches(&db)[0].id;
    let (inputs, evidence): (String, String) = db
        .with_conn(|c| {
            Ok(c.query_row(
                "SELECT metadata_signals_json, evidence_json FROM property_matches WHERE id = ?1",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?)
        })
        .unwrap();
    for stored in [&inputs, &evidence] {
        assert!(
            !stored.contains("Maria") && !stored.contains("maria") && !stored.contains("99999"),
            "{stored}"
        );
    }
    assert!(
        inputs.contains("goiania") && inputs.contains("setor bueno"),
        "{inputs}"
    );
}

#[tokio::test]
async fn a_cancelled_pass_stores_nothing() {
    let db = fresh();
    the_same_house_twice(&db);
    let cancel = CancelToken::new();
    cancel.cancel();
    let report = run_matching(&deps(&db), &MatchConfig::default(), &cancel)
        .await
        .unwrap();
    assert!(report.cancelled);
    assert_eq!((report.compared, report.created), (0, 0));
    assert_eq!(
        report.listings, 0,
        "a run cancelled up front does not even load the library"
    );
    let leftover = matches(&db);
    assert_eq!(leftover.len(), 0, "{leftover:?}");
}

#[tokio::test]
async fn an_invalid_configuration_is_rejected_before_any_work() {
    let db = fresh();
    the_same_house_twice(&db);
    let config = MatchConfig {
        strong_image_pairs: 0,
        ..MatchConfig::default()
    };
    let error = run_matching(&deps(&db), &config, &CancelToken::new())
        .await
        .unwrap_err();
    assert!(matches!(error, Error::InvalidConfig { .. }), "{error}");
    let leftover = matches(&db);
    assert_eq!(leftover.len(), 0, "{leftover:?}");
}

#[tokio::test]
async fn an_empty_library_is_a_valid_pass() {
    let db = fresh();
    let report = run(&db).await;
    assert_eq!(
        (report.listings, report.candidate_pairs, report.created),
        (0, 0, 0)
    );
    assert_eq!(report.rule_version, RULE_VERSION);
}

#[tokio::test]
async fn cancelling_between_batches_keeps_what_was_stored_and_stops_there() {
    let db = fresh();
    // Three unrelated pairs of ads, each pair sharing one photo: three proposals.
    for (n, hash) in [(1_u32, H1), (3, H2), (5, 0x5a5a_a5a5_3c3c_c3c3)] {
        for k in [n, n + 1] {
            let (listing, _) = add(&db, "alpha", k, "Casa", "Casa 3 quartos");
            hash_image(&db, listing, &format!("https://img.test/{k}.png"), hash);
        }
    }
    let cancel = CancelToken::new();
    let trigger = cancel.clone();
    let cancelling = CollectionDeps {
        db: Arc::clone(&db),
        global_limit: Arc::new(Semaphore::new(4)),
        // Reading the clock for the first batch is what cancels the run.
        clock: Arc::new(move || {
            trigger.cancel();
            at(100)
        }),
    };
    let report = run_batched(&cancelling, &MatchConfig::default(), &cancel, 1)
        .await
        .unwrap();
    assert!(report.cancelled);
    assert_eq!(
        (report.candidate_pairs, report.created),
        (3, 1),
        "one batch was stored, then it stopped"
    );
    assert_eq!(matches(&db).len(), 1);
    // A later, uncancelled pass completes the work without duplicating anything.
    let rest = run(&db).await;
    assert_eq!(
        (rest.created, rest.unchanged, rest.cancelled),
        (2, 1, false)
    );
    assert_eq!(matches(&db).len(), 3);
}
