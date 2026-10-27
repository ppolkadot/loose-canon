use serde_json::json;

use super::*;
use crate::{
    db::{
        price_history::{RecordOutcome, list_observations},
        regions::{DEFAULT_SEED_JSON, LocationIndex, parse_seed, seed_regions},
        testkit::{at, mem, normalized},
    },
    error::Error,
    models::{
        confidence::Confidence,
        listing::{ExtractionStatus, ListingStatus},
        price::PriceClassification,
    },
    parser::{PriceFieldHint, StructuredText, parse_listing_with_price_hint},
};

const FULL: &str = "Casa 3/4 com suíte, lote 300 m². Valor total R$ 350 mil";

fn write(listing: &NormalizedListing) -> ListingWrite<'_> {
    ListingWrite {
        listing,
        source_fields: None,
        location: None,
    }
}

fn scalar(db: &crate::db::Database, sql: &str) -> i64 {
    db.with_conn(|c| Ok(c.query_row(sql, [], |r| r.get(0))?))
        .unwrap()
}

fn upsert(db: &crate::db::Database, listing: &NormalizedListing) -> UpsertOutcome {
    db.with_conn(|c| upsert_listing(c, &write(listing)))
        .unwrap()
}

fn stored(db: &crate::db::Database, id: i64) -> StoredListing {
    db.with_conn(|c| get_listing(c, id))
        .unwrap()
        .expect("listing exists")
}

#[test]
fn a_new_listing_creates_the_listing_its_property_and_the_first_price_observation() {
    let db = mem();
    let l = normalized("fake", "https://fake.test/1", FULL, 0);
    let out = upsert(&db, &l);
    assert!(out.listing_created && out.property_created && !out.reappeared);
    assert!(matches!(out.price, Some(RecordOutcome::Recorded { .. })));

    let s = stored(&db, out.listing_id);
    assert_eq!(s.property_id, Some(out.property_id));
    assert_eq!(
        (s.url.as_str(), s.normalized_url.as_str()),
        ("https://fake.test/1", "https://fake.test/1")
    );
    assert_eq!(
        (s.source_platform.as_str(), s.adapter_id.as_str()),
        ("FAKE", "fake")
    );
    assert_eq!(s.title.as_deref(), Some("Casa"));
    assert_eq!(s.raw_description.as_deref(), Some(FULL));
    let price = s.price.expect("headline price stored");
    assert_eq!(
        (
            price.amount.centavos(),
            price.classification,
            price.confidence
        ),
        (35_000_000, PriceClassification::FullPrice, Confidence::High)
    );
    assert_eq!(s.facts.as_ref(), Some(&l.facts));
    assert_eq!(s.parser_version.as_deref(), Some("br_real_estate/2"));
    assert_eq!(
        (s.status, s.extraction_status),
        (ListingStatus::Active, ExtractionStatus::Ok)
    );
    assert_eq!(
        (
            s.first_seen_at,
            s.last_seen_at,
            s.last_verified_at,
            s.last_scraped_at
        ),
        (at(0), at(0), Some(at(0)), Some(at(0)))
    );

    let history = db
        .with_conn(|c| list_observations(c, out.listing_id))
        .unwrap();
    assert_eq!(history.len(), 1);
    let provenance = &history[0].observation.provenance;
    assert_eq!(provenance.extractor, "br_real_estate/2");
    assert_eq!(
        provenance.evidence.as_ref().unwrap().snippet,
        "Valor total R$ 350 mil"
    );
    assert_eq!(scalar(&db, "SELECT COUNT(*) FROM properties"), 1);
}

#[test]
fn a_resolved_location_is_recorded_on_the_new_property() {
    let db = mem();
    let location = db
        .with_conn(|c| {
            seed_regions(c, &parse_seed(DEFAULT_SEED_JSON)?)?;
            Ok(LocationIndex::load(c)?.resolve("Setor Bueno, Goiânia"))
        })
        .unwrap()
        .expect("resolves");
    let mut l = normalized("fake", "https://fake.test/1", FULL, 0);
    l.location_text = Some("Setor Bueno, Goiânia".into());
    let out = db
        .with_conn(|c| {
            upsert_listing(
                c,
                &ListingWrite {
                    listing: &l,
                    source_fields: None,
                    location: Some(location.clone()),
                },
            )
        })
        .unwrap();
    let (region_id, municipality, neighborhood): (i64, String, String) = db
        .with_conn(|c| {
            Ok(c.query_row(
                "SELECT region_id, municipality, neighborhood FROM properties WHERE id = ?1",
                [out.property_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )?)
        })
        .unwrap();
    assert_eq!(region_id, location.region_id);
    assert_eq!(
        (municipality.as_str(), neighborhood.as_str()),
        ("Goiânia", "Setor Bueno")
    );
    assert_eq!(
        stored(&db, out.listing_id).location_text.as_deref(),
        Some("Setor Bueno, Goiânia")
    );
}

#[test]
fn seeing_the_same_advertisement_again_updates_it_in_place_without_inventing_price_changes() {
    let db = mem();
    let first = upsert(&db, &normalized("fake", "https://fake.test/1", FULL, 0));
    let again = upsert(&db, &normalized("fake", "https://fake.test/1", FULL, 3_600));
    assert_eq!(again.listing_id, first.listing_id);
    assert_eq!(again.property_id, first.property_id);
    assert!(!again.listing_created && !again.property_created);
    assert!(matches!(again.price, Some(RecordOutcome::Unchanged { .. })));

    let s = stored(&db, first.listing_id);
    assert_eq!(s.first_seen_at, at(0), "first seen never moves");
    assert_eq!(
        (s.last_seen_at, s.last_scraped_at),
        (at(3_600), Some(at(3_600)))
    );
    assert_eq!(scalar(&db, "SELECT COUNT(*) FROM listing_links"), 1);
    assert_eq!(scalar(&db, "SELECT COUNT(*) FROM properties"), 1);
    assert_eq!(scalar(&db, "SELECT COUNT(*) FROM listing_price_history"), 1);
}

#[test]
fn a_changed_price_or_meaning_is_appended_to_the_history_and_the_listing_follows_it() {
    let db = mem();
    let id = upsert(
        &db,
        &normalized("fake", "https://fake.test/1", "Valor total R$ 350 mil", 0),
    )
    .listing_id;
    let drop = upsert(
        &db,
        &normalized("fake", "https://fake.test/1", "Valor total R$ 340 mil", 100),
    );
    assert!(matches!(drop.price, Some(RecordOutcome::Recorded { .. })));
    assert_eq!(stored(&db, id).price.unwrap().amount.centavos(), 34_000_000);

    upsert(
        &db,
        &normalized(
            "fake",
            "https://fake.test/1",
            "Passo ágio de R$ 340 mil",
            200,
        ),
    );
    let s = stored(&db, id);
    assert_eq!(s.price.unwrap().classification, PriceClassification::Agio);

    let history = db.with_conn(|c| list_observations(c, id)).unwrap();
    let seen: Vec<(i64, PriceClassification)> = history
        .iter()
        .map(|h| {
            (
                h.observation.amount.centavos() / 100,
                h.observation.classification,
            )
        })
        .collect();
    assert_eq!(
        seen,
        vec![
            (350_000, PriceClassification::FullPrice),
            (340_000, PriceClassification::FullPrice),
            (340_000, PriceClassification::Agio),
        ]
    );
}

#[test]
fn the_same_url_on_another_adapter_is_a_different_listing() {
    let db = mem();
    let a = upsert(&db, &normalized("alpha", "https://shared.test/1", FULL, 0));
    let b = upsert(&db, &normalized("beta", "https://shared.test/1", FULL, 0));
    assert_ne!(a.listing_id, b.listing_id);
    assert_ne!(
        a.property_id, b.property_id,
        "each new listing is its own property candidate"
    );
    let found = db
        .with_conn(|c| find_listing(c, "beta", "https://shared.test/1"))
        .unwrap()
        .unwrap();
    assert_eq!(found.id, b.listing_id);
    assert!(
        db.with_conn(|c| find_listing(c, "gamma", "https://shared.test/1"))
            .unwrap()
            .is_none()
    );
}

#[test]
fn an_unavailable_listing_keeps_its_history_and_is_active_again_when_it_returns() {
    let db = mem();
    let first = upsert(&db, &normalized("fake", "https://fake.test/1", FULL, 0));
    let marked = db
        .with_conn(|c| mark_unavailable(c, first.listing_id, at(10), "HTTP 410"))
        .unwrap();
    assert!(marked);
    assert!(
        !db.with_conn(|c| mark_unavailable(c, first.listing_id, at(11), "HTTP 410"))
            .unwrap(),
        "idempotent"
    );
    let s = stored(&db, first.listing_id);
    assert_eq!(s.status, ListingStatus::Unavailable);
    assert_eq!(
        (s.status_changed_at, s.status_reason.as_deref()),
        (Some(at(10)), Some("HTTP 410"))
    );
    assert!(s.price.is_some(), "an unavailable ad keeps its last price");
    assert_eq!(scalar(&db, "SELECT COUNT(*) FROM listing_price_history"), 1);

    let back = upsert(&db, &normalized("fake", "https://fake.test/1", FULL, 20));
    assert!(back.reappeared);
    let s = stored(&db, first.listing_id);
    assert_eq!(s.status, ListingStatus::Active);
    assert_eq!((s.status_changed_at, s.status_reason), (Some(at(20)), None));
    assert_eq!(
        scalar(&db, "SELECT COUNT(*) FROM listing_price_history"),
        1,
        "same price: no new history"
    );
}

#[test]
fn marking_a_missing_listing_unavailable_changes_nothing() {
    let db = mem();
    assert!(
        !db.with_conn(|c| mark_unavailable(c, 999, at(0), "gone"))
            .unwrap()
    );
}

#[test]
fn hiding_survives_rediscovery_through_upsert() {
    let db = mem();
    let first = upsert(&db, &normalized("fake", "https://fake.test/1", FULL, 0));
    db.with_conn(|c| {
        c.execute(
            "UPDATE properties SET user_status = 'hidden', hidden_at = '2026-03-01T00:00:00.000Z' WHERE id = ?1",
            [first.property_id],
        )?;
        Ok(())
    })
    .unwrap();
    upsert(
        &db,
        &normalized("fake", "https://fake.test/1", "Valor total R$ 340 mil", 500),
    );
    let (status, hidden_at): (String, Option<String>) = db
        .with_conn(|c| {
            Ok(c.query_row(
                "SELECT user_status, hidden_at FROM properties WHERE id = ?1",
                [first.property_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?)
        })
        .unwrap();
    assert_eq!(status, "hidden");
    assert_eq!(hidden_at.as_deref(), Some("2026-03-01T00:00:00.000Z"));
}

#[test]
fn a_failed_extraction_keeps_the_known_content_and_records_the_failure() {
    let db = mem();
    let id = upsert(&db, &normalized("fake", "https://fake.test/1", FULL, 0)).listing_id;

    let mut failed = normalized("fake", "https://fake.test/1", "", 50);
    failed.extraction_status = ExtractionStatus::Failed;
    failed.extraction_error = Some("layout changed".into());
    failed.title = None;
    failed.description = None;
    failed.last_verified_at = None;
    let out = upsert(&db, &failed);
    assert_eq!(out.price, None);

    let s = stored(&db, id);
    assert_eq!(s.extraction_status, ExtractionStatus::Failed);
    assert_eq!(s.extraction_error.as_deref(), Some("layout changed"));
    assert_eq!(
        s.raw_description.as_deref(),
        Some(FULL),
        "content must survive a failed parse"
    );
    assert_eq!(s.title.as_deref(), Some("Casa"));
    assert_eq!(s.price.unwrap().amount.centavos(), 35_000_000);
    assert_eq!(s.last_scraped_at, Some(at(50)));
    assert_eq!(
        s.last_verified_at,
        Some(at(0)),
        "a failed parse is not a verification"
    );
    assert_eq!(s.facts.unwrap(), normalized("fake", "x", FULL, 0).facts);
    assert_eq!(scalar(&db, "SELECT COUNT(*) FROM listing_price_history"), 1);
}

#[test]
fn a_brand_new_listing_whose_extraction_failed_is_still_recorded() {
    let db = mem();
    let mut failed = normalized("fake", "https://fake.test/9", "", 0);
    failed.extraction_status = ExtractionStatus::Failed;
    failed.extraction_error = Some("unparseable".into());
    failed.last_verified_at = None;
    let out = upsert(&db, &failed);
    let s = stored(&db, out.listing_id);
    assert_eq!(s.extraction_status, ExtractionStatus::Failed);
    assert!(s.price.is_none());
}

#[test]
fn a_page_without_any_price_keeps_the_last_known_price() {
    let db = mem();
    let id = upsert(&db, &normalized("fake", "https://fake.test/1", FULL, 0)).listing_id;
    let out = upsert(
        &db,
        &normalized("fake", "https://fake.test/1", "Casa linda, consulte", 60),
    );
    assert_eq!(out.price, None);
    assert_eq!(stored(&db, id).price.unwrap().amount.centavos(), 35_000_000);
}

#[test]
fn conflicting_full_prices_clear_the_current_price_but_keep_the_history() {
    let db = mem();
    let id = upsert(&db, &normalized("fake", "https://fake.test/1", FULL, 0)).listing_id;
    let out = upsert(
        &db,
        &normalized(
            "fake",
            "https://fake.test/1",
            "Valor total R$ 350 mil. Preço R$ 330 mil",
            60,
        ),
    );
    assert_eq!(
        out.price, None,
        "no observation when the page contradicts itself"
    );
    assert!(
        stored(&db, id).price.is_none(),
        "an ambiguous price must not look current"
    );
    assert_eq!(scalar(&db, "SELECT COUNT(*) FROM listing_price_history"), 1);
}

#[test]
fn a_failure_while_recording_the_price_rolls_the_whole_write_back() {
    let db = mem();
    let id = upsert(&db, &normalized("fake", "https://fake.test/1", FULL, 100)).listing_id;

    // Older than the latest observation: the price recorder refuses it.
    let mut stale = normalized("fake", "https://fake.test/1", "Valor total R$ 340 mil", 50);
    stale.title = Some("Outro título".into());
    let err = db
        .with_conn(|c| upsert_listing(c, &write(&stale)))
        .unwrap_err();
    assert!(matches!(err, Error::OutOfOrderObservation { .. }), "{err}");

    let s = stored(&db, id);
    assert_eq!(
        s.title.as_deref(),
        Some("Casa"),
        "listing content must roll back with the price"
    );
    assert_eq!(s.last_seen_at, at(100));
    assert_eq!(s.price.unwrap().amount.centavos(), 35_000_000);
    assert_eq!(scalar(&db, "SELECT COUNT(*) FROM listing_price_history"), 1);
}

#[test]
fn raw_source_fields_are_stored_exactly_as_given() {
    let db = mem();
    let l = normalized("fake", "https://fake.test/1", FULL, 0);
    let fields = json!({ "price": "R$ 350.000", "title": "Casa 3 quartos" });
    let out = db
        .with_conn(|c| {
            upsert_listing(
                c,
                &ListingWrite {
                    listing: &l,
                    source_fields: Some(&fields),
                    location: None,
                },
            )
        })
        .unwrap();
    assert_eq!(stored(&db, out.listing_id).source_fields, Some(fields));
}

#[test]
fn a_hinted_price_is_recorded_with_its_field_and_evidence() {
    let db = mem();
    let mut l = normalized("fake", "https://fake.test/1", "Casa 3 quartos", 0);
    l.facts = parse_listing_with_price_hint(
        StructuredText {
            field: "description",
            text: "Casa 3 quartos",
        },
        &PriceFieldHint {
            field: StructuredText {
                field: "price",
                text: "R$ 350.000",
            },
            confidence: Confidence::Medium,
        },
    );
    let out = upsert(&db, &l);
    let history = db
        .with_conn(|c| list_observations(c, out.listing_id))
        .unwrap();
    let provenance = &history[0].observation.provenance;
    assert_eq!(provenance.source_field.as_deref(), Some("price"));
    assert_eq!(provenance.evidence.as_ref().unwrap().snippet, "R$ 350.000");
    assert_eq!(history[0].observation.confidence, Some(Confidence::Medium));
}

#[test]
fn listings_of_a_property_are_only_its_own() {
    let db = mem();
    let a = upsert(&db, &normalized("alpha", "https://a.test/1", FULL, 0));
    let b = upsert(&db, &normalized("beta", "https://b.test/1", FULL, 0));
    assert_eq!(
        db.with_conn(|c| listings_of_property(c, a.property_id))
            .unwrap()
            .len(),
        1
    );
    db.with_conn(|c| {
        c.execute(
            "UPDATE listing_links SET property_id = ?1 WHERE id = ?2",
            [a.property_id, b.listing_id],
        )?;
        Ok(())
    })
    .unwrap();
    let ids: Vec<i64> = db
        .with_conn(|c| listings_of_property(c, a.property_id))
        .unwrap()
        .iter()
        .map(|l| l.id)
        .collect();
    assert_eq!(ids, vec![a.listing_id, b.listing_id]);
}
