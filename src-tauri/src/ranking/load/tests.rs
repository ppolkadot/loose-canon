use super::*;
use crate::{
    db::{
        Database,
        matches::{NewMatch, accept_match, store_candidate, undo_merge},
        testkit::{at, mem},
    },
    matching::testkit::{FULL_AD, add, add_typed, place},
    models::{
        characteristics::PropertyType,
        confidence::Confidence,
        listing::ListingStatus,
        matching::{MatchCategory, MatchEvidence},
    },
};

fn inputs(db: &Database) -> Vec<RankInput> {
    db.with_conn(|c| load_inputs(c)).unwrap()
}

fn evidence() -> MatchEvidence {
    MatchEvidence {
        rule_version: "match_rules/1".into(),
        signals: vec![],
        conflicts: vec![],
        unknown: vec![],
        image_pairs: vec![],
        notes: vec![],
    }
}

/// Stores a proposal for two listings and returns its match id.
fn propose(db: &Database, a: i64, b: i64) -> i64 {
    let evidence = evidence();
    let new = NewMatch {
        listing_a_id: a,
        listing_b_id: b,
        category: MatchCategory::High,
        min_image_distance: None,
        evidence: &evidence,
        inputs: serde_json::json!({}),
    };
    db.with_conn(|c| store_candidate(c, &new, at(1)))
        .unwrap()
        .match_id
}

#[test]
fn a_property_becomes_an_input_with_its_canonical_values_and_place() {
    let db = mem();
    let (_, property) = add_typed(&db, "alpha", 1, FULL_AD, PropertyType::House);
    place(&db, property, "Goiânia", Some("Setor Bueno"));
    let all = inputs(&db);
    assert_eq!(all.len(), 1);
    let p = &all[0];
    assert_eq!(p.property_id, property);
    assert_eq!(
        (p.municipality.as_deref(), p.neighborhood.as_deref()),
        (Some("Goiânia"), Some("Setor Bueno"))
    );
    assert_eq!(
        (p.bedrooms, p.suites, p.parking_spaces),
        (Some(3), Some(1), Some(2))
    );
    assert_eq!(
        (p.built_area_centi_m2, p.lot_area_centi_m2),
        (Some(18_000), Some(36_000))
    );
    assert_eq!(p.conflicting_fields, Vec::<String>::new());
    assert!(!p.verified_merge);
}

#[test]
fn only_an_established_full_price_is_a_price_to_rank_by() {
    let db = mem();
    add(&db, "alpha", 1, "Casa", FULL_AD);
    add(
        &db,
        "alpha",
        2,
        "Casa",
        "Casa 3 quartos. Ágio R$ 80 mil, assume financiamento",
    );
    add(&db, "alpha", 3, "Casa", "Casa 3 quartos sem preço");
    let all = inputs(&db);
    let price = |p: &RankInput| p.listings[0].established_price;
    assert_eq!(price(&all[0]).map(|(amount, _)| amount), Some(45_000_000));
    assert!(price(&all[0]).unwrap().1 >= Confidence::Medium);
    assert_eq!(
        price(&all[1]),
        None,
        "an ágio is not the price of the property"
    );
    assert_eq!(price(&all[2]), None);
}

#[test]
fn listings_keep_their_source_status_and_dates() {
    let db = mem();
    let (listing, _) = add(&db, "alpha", 1, "Casa", FULL_AD);
    db.with_conn(|c| crate::db::listings::mark_unavailable(c, listing, at(500), "HTTP 410"))
        .unwrap();
    let l = &inputs(&db)[0].listings[0];
    assert_eq!(
        (l.adapter_id.as_str(), l.status),
        ("alpha", ListingStatus::Unavailable)
    );
    assert_eq!(l.first_seen_at, at(0));
    assert_eq!(l.last_scraped_at, Some(at(0)));
}

#[test]
fn a_merge_group_is_one_input_with_all_its_listings_and_is_marked_verified() {
    let db = mem();
    let (la, pa) = add(&db, "alpha", 1, "Casa", FULL_AD);
    let (lb, pb) = add(&db, "beta", 1, "Casa", FULL_AD);
    let (_, pc) = add(&db, "gamma", 1, "Casa", FULL_AD);
    assert_eq!(inputs(&db).len(), 3);
    let id = propose(&db, la, lb);
    assert!(
        inputs(&db).iter().all(|p| !p.verified_merge),
        "a proposal is not a verification"
    );

    db.with_conn(|c| accept_match(c, id, None, at(5))).unwrap();
    let all = inputs(&db);
    assert_eq!(
        all.iter().map(|p| p.property_id).collect::<Vec<_>>(),
        vec![pa, pc],
        "the absorbed property is no longer a root"
    );
    assert_eq!(all[0].listings.len(), 2);
    assert!(all[0].verified_merge && !all[1].verified_merge);
    let adapters: Vec<&str> = all[0]
        .listings
        .iter()
        .map(|l| l.adapter_id.as_str())
        .collect();
    assert!(adapters.contains(&"alpha") && adapters.contains(&"beta"));

    db.with_conn(|c| undo_merge(c, id, None, at(9))).unwrap();
    let all = inputs(&db);
    assert_eq!(
        all.iter().map(|p| p.property_id).collect::<Vec<_>>(),
        vec![pa, pb, pc]
    );
    assert!(
        all.iter().all(|p| !p.verified_merge),
        "an undone merge is no longer verified"
    );
}

#[test]
fn values_the_listings_of_a_group_disagree_on_are_reported_as_conflicts() {
    let db = mem();
    let (la, _) = add(
        &db,
        "alpha",
        1,
        "Casa",
        "Casa 3 quartos. Valor total R$ 350 mil",
    );
    let (lb, _) = add(
        &db,
        "beta",
        1,
        "Casa",
        "Casa 2 quartos. Valor total R$ 350 mil",
    );
    let id = propose(&db, la, lb);
    assert!(inputs(&db).iter().all(|p| p.conflicting_fields.is_empty()));
    db.with_conn(|c| accept_match(c, id, None, at(5))).unwrap();
    let merged = &inputs(&db)[0];
    assert_eq!(merged.conflicting_fields, vec!["bedrooms".to_owned()]);
    assert_eq!(merged.bedrooms, None, "a disputed value stays unknown");
}

#[test]
fn inputs_come_in_property_order_and_an_empty_library_is_fine() {
    let db = mem();
    assert_eq!(inputs(&db), Vec::new());
    let ids: Vec<i64> = (1..=4)
        .map(|n| add(&db, "alpha", n, "Casa", FULL_AD).1)
        .collect();
    assert_eq!(
        inputs(&db)
            .iter()
            .map(|p| p.property_id)
            .collect::<Vec<_>>(),
        ids
    );
}
