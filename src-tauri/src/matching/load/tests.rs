use super::*;
use crate::{
    db::{
        Database,
        images::{images_of_listing, record_final, register_images},
        matches::{NewMatch, accept_match, store_candidate},
        testkit::{at, mem},
    },
    matching::testkit::{FULL_AD, add, add_typed, hash_image, place},
    models::{
        characteristics::PropertyType,
        image::ImageProcessingStatus,
        matching::{MatchCategory, MatchEvidence},
    },
};

fn inputs(db: &Database) -> Vec<MatchInput> {
    db.with_conn(|c| load_inputs(c)).unwrap()
}

fn one(db: &Database) -> MatchInput {
    let mut all = inputs(db);
    assert_eq!(all.len(), 1);
    all.remove(0)
}

#[test]
fn a_listing_becomes_an_input_with_everything_the_rules_use() {
    let db = mem();
    let (listing, property) = add_typed(&db, "alpha", 1, FULL_AD, PropertyType::House);
    place(&db, property, "Goiânia", Some("Setor Bueno"));
    let input = one(&db);
    assert_eq!(
        (
            input.listing_id,
            input.adapter_id.as_str(),
            input.group_root
        ),
        (listing, "alpha", property)
    );
    assert_eq!(input.municipality.as_deref(), Some("goiania"), "folded");
    assert_eq!(input.neighborhood.as_deref(), Some("setor bueno"));
    assert_eq!(input.property_type, Some(PropertyType::House));
    assert_eq!(
        (input.bedrooms, input.suites, input.parking_spaces),
        (Some(3), Some(1), Some(2))
    );
    assert_eq!(
        (
            input.built_area_centi_m2,
            input.lot_area_centi_m2,
            input.usable_area_centi_m2
        ),
        (Some(18_000), Some(36_000), None)
    );
    assert_eq!(input.full_price_centavos, Some(45_000_000));
    assert_eq!(input.contradictory.len(), 0, "{:?}", input.contradictory);
}

#[test]
fn only_an_established_full_price_counts_as_the_price_to_compare() {
    let db = mem();
    add(
        &db,
        "alpha",
        1,
        "Casa",
        "Casa 3 quartos. Ágio R$ 80 mil, assume financiamento",
    );
    assert_eq!(
        one(&db).full_price_centavos,
        None,
        "an ágio is not the price of the property"
    );
}

#[test]
fn a_listing_with_conflicting_statements_is_unknown_and_says_why() {
    let db = mem();
    add(
        &db,
        "alpha",
        1,
        "Casa",
        "Casa 3 quartos, depois diz 2 quartos. Valor total R$ 450 mil",
    );
    let input = one(&db);
    assert_eq!(input.bedrooms, None);
    assert_eq!(input.contradictory, vec!["bedrooms"]);
}

#[test]
fn a_listing_without_stored_facts_gives_an_input_that_knows_nothing_instead_of_an_error() {
    let db = mem();
    let (listing, _) = add(&db, "alpha", 1, "Casa", FULL_AD);
    db.with_conn(|c| {
        c.execute(
            "UPDATE listing_links
                SET extracted_facts_json = NULL, price_centavos = NULL, price_confidence = NULL,
                    price_classification = 'unknown'
              WHERE id = ?1",
            [listing],
        )?;
        Ok(())
    })
    .unwrap();
    let input = one(&db);
    assert_eq!(
        (
            input.bedrooms,
            input.property_type,
            input.full_price_centavos
        ),
        (None, None, None)
    );
}

#[test]
fn a_property_without_a_resolved_place_has_no_location() {
    let db = mem();
    add(&db, "alpha", 1, "Casa", FULL_AD);
    let input = one(&db);
    assert_eq!((input.municipality, input.neighborhood), (None, None));
}

#[test]
fn description_terms_are_distinctive_words_only() {
    let db = mem();
    add(
        &db,
        "alpha",
        1,
        "Casa Térrea no Jardim América",
        "Ligue (62) 99999-0000 e fale com Maria. Quintal grande, 3 quartos, R$ 450.000",
    );
    let terms = one(&db).tokens;
    for expected in [
        "casa", "terrea", "jardim", "america", "ligue", "fale", "maria", "quintal", "grande",
        "quartos",
    ] {
        assert!(
            terms.contains(expected),
            "{expected} missing from {terms:?}"
        );
    }
    for rejected in ["no", "com", "62", "99999", "0000", "450", "000", "e", "r"] {
        assert!(
            !terms.contains(rejected),
            "{rejected} should not be a term: {terms:?}"
        );
    }
    assert!(
        terms
            .iter()
            .all(|t| t.chars().count() >= 4 && t.chars().all(char::is_alphabetic)),
        "{terms:?}"
    );
}

#[test]
fn hashed_images_are_included_and_unprocessed_or_failed_ones_are_not() {
    let db = mem();
    let (listing, _) = add(&db, "alpha", 1, "Casa", FULL_AD);
    let hashed = hash_image(
        &db,
        listing,
        "https://img.test/a.png",
        0xDEAD_BEEF_0000_0001,
    );
    db.with_conn(|c| {
        register_images(
            c,
            listing,
            &[
                "https://img.test/pending.png".into(),
                "https://img.test/bad.png".into(),
            ],
            10,
            at(0),
        )
    })
    .unwrap();
    let bad = db
        .with_conn(|c| images_of_listing(c, listing))
        .unwrap()
        .into_iter()
        .find(|i| i.source_url.as_deref() == Some("https://img.test/bad.png"))
        .unwrap()
        .id;
    db.with_conn(|c| record_final(c, bad, ImageProcessingStatus::Failed, "corrupt", at(2)))
        .unwrap();
    let images = one(&db).images;
    assert_eq!(images.len(), 1);
    assert_eq!(
        (images[0].image_id, images[0].hash),
        (hashed, 0xDEAD_BEEF_0000_0001)
    );
}

#[test]
fn listings_of_merged_properties_share_their_group_root() {
    let db = mem();
    let (a, pa) = add(&db, "alpha", 1, "Casa", FULL_AD);
    let (b, pb) = add(&db, "beta", 1, "Casa", FULL_AD);
    let (c, pc) = add(&db, "gamma", 1, "Casa", FULL_AD);
    let evidence = MatchEvidence {
        rule_version: "match_rules/1".into(),
        signals: vec![],
        conflicts: vec![],
        unknown: vec![],
        image_pairs: vec![],
        notes: vec![],
    };
    let new = NewMatch {
        listing_a_id: a,
        listing_b_id: b,
        category: MatchCategory::High,
        min_image_distance: None,
        evidence: &evidence,
        inputs: serde_json::json!({}),
    };
    let id = db
        .with_conn(|conn| store_candidate(conn, &new, at(5)))
        .unwrap()
        .match_id;
    db.with_conn(|conn| accept_match(conn, id, None, at(6)))
        .unwrap();
    let roots: Vec<(i64, i64)> = inputs(&db)
        .iter()
        .map(|i| (i.listing_id, i.group_root))
        .collect();
    assert_eq!(roots, vec![(a, pa), (b, pa), (c, pc)]);
    assert_ne!(pa, pb);
}

#[test]
fn inputs_come_in_listing_order() {
    let db = mem();
    let ids: Vec<i64> = (1..=3)
        .map(|n| add(&db, "alpha", n, "Casa", FULL_AD).0)
        .collect();
    assert_eq!(
        inputs(&db).iter().map(|i| i.listing_id).collect::<Vec<_>>(),
        ids
    );
}
