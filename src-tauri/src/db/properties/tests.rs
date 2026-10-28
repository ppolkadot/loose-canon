use super::*;
use crate::{
    db::{
        Database,
        listings::{ListingWrite, UpsertOutcome, mark_unavailable, upsert_listing},
        regions::{DEFAULT_SEED_JSON, LocationIndex, parse_seed, seed_regions},
        testkit::{at, mem, normalized},
    },
    models::{confidence::Confidence, listing::UserStatus},
    parser::{PriceFieldHint, StructuredText, parse_listing_with_price_hint},
};

const TS: &str = "2026-03-01T12:00:00.000Z";

fn seeded() -> Database {
    let db = mem();
    db.with_conn(|c| seed_regions(c, &parse_seed(DEFAULT_SEED_JSON)?))
        .unwrap();
    db
}

/// Stores a listing (resolving `location` against the seeded regions) and returns its ids.
fn add(
    db: &Database,
    adapter: &str,
    url: &str,
    text: &str,
    secs: i64,
    location: Option<&str>,
) -> UpsertOutcome {
    let mut listing = normalized(adapter, url, text, secs);
    listing.location_text = location.map(str::to_owned);
    db.with_conn(|c| {
        let resolved = match location {
            Some(text) => LocationIndex::load(c)?.resolve(text),
            None => None,
        };
        upsert_listing(
            c,
            &ListingWrite {
                listing: &listing,
                source_fields: None,
                location: resolved,
            },
        )
    })
    .unwrap()
}

fn merge_sql(db: &Database, absorbed: i64, into: i64) {
    db.with_conn(|c| {
        c.execute(
            "UPDATE properties SET merged_into_property_id = ?1, merged_at = ?2 WHERE id = ?3",
            rusqlite::params![into, TS, absorbed],
        )?;
        Ok(())
    })
    .unwrap();
}

fn unmerge_sql(db: &Database, absorbed: i64) {
    db.with_conn(|c| {
        c.execute(
            "UPDATE properties SET merged_into_property_id = NULL, merged_at = NULL WHERE id = ?1",
            [absorbed],
        )?;
        Ok(())
    })
    .unwrap();
}

type Row = (
    Option<i64>,
    Option<i64>,
    Option<i64>,
    Option<i64>,
    Option<i64>,
    Option<i64>,
);

fn canonical(db: &Database, property_id: i64) -> Row {
    db.with_conn(|c| {
        Ok(c.query_row(
            "SELECT bedrooms, suites, parking_spaces, lot_area_centi_m2, built_area_centi_m2, usable_area_centi_m2
               FROM properties WHERE id = ?1",
            [property_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)),
        )?)
    })
    .unwrap()
}

fn provenance(db: &Database, property_id: i64) -> serde_json::Value {
    let json: String = db
        .with_conn(|c| {
            Ok(c.query_row(
                "SELECT canonical_provenance_json FROM properties WHERE id = ?1",
                [property_id],
                |r| r.get(0),
            )?)
        })
        .unwrap();
    serde_json::from_str(&json).unwrap()
}

fn set_status(db: &Database, property_id: i64, status: UserStatus, secs: i64) -> usize {
    db.with_conn(|c| set_user_status(c, property_id, status, at(secs)))
        .unwrap()
}

fn list(db: &Database, query: &PropertyQuery) -> Page<PropertyView> {
    db.with_conn(|c| list_properties(c, query)).unwrap()
}

fn ids(page: &Page<PropertyView>) -> Vec<i64> {
    page.items.iter().map(|p| p.id).collect()
}

// ---- canonical values ----------------------------------------------------------------------

#[test]
fn canonical_values_come_from_the_listing_and_record_where_they_came_from() {
    let db = mem();
    let out = add(
        &db,
        "fake",
        "https://fake.test/1",
        "Casa 3/4, 1 suíte, 2 vagas, lote 300 m² e 180 m² de área construída. Valor total R$ 350 mil",
        0,
        None,
    );
    assert_eq!(
        canonical(&db, out.property_id),
        (Some(3), Some(1), Some(2), Some(30_000), Some(18_000), None),
        "unknown stays NULL, never 0"
    );
    let p = provenance(&db, out.property_id);
    assert_eq!(p["bedrooms"]["status"], "known");
    assert_eq!(p["bedrooms"]["value"], 3);
    assert_eq!(p["bedrooms"]["sources"][0]["listing_id"], out.listing_id);
    assert_eq!(p["bedrooms"]["sources"][0]["snippet"], "3/4");
    assert_eq!(p["lot_area"]["value"], 30_000);
    assert_eq!(p["usable_area"]["status"], "unknown");
}

#[test]
fn agreeing_listings_keep_the_value_and_disagreeing_ones_leave_it_unknown_with_both_recorded() {
    let db = mem();
    let a = add(
        &db,
        "alpha",
        "https://a.test/1",
        "Casa 3 quartos, lote 300 m². Valor total R$ 350 mil",
        0,
        None,
    );
    let b = add(
        &db,
        "beta",
        "https://b.test/1",
        "Sobrado 3 quartos, lote 360 m². Valor total R$ 345 mil",
        10,
        None,
    );
    merge_sql(&db, b.property_id, a.property_id);
    db.with_conn(|c| refresh_canonical_chain(c, b.property_id))
        .unwrap();

    let row = canonical(&db, a.property_id);
    assert_eq!(row.0, Some(3), "both say 3 bedrooms");
    assert_eq!(
        row.3, None,
        "300 m² vs 360 m² must not be resolved silently"
    );
    let p = provenance(&db, a.property_id);
    assert_eq!(p["bedrooms"]["status"], "known");
    assert_eq!(p["bedrooms"]["sources"].as_array().unwrap().len(), 2);
    assert_eq!(p["lot_area"]["status"], "conflicting");
    let values: Vec<i64> = p["lot_area"]["values"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v["value"].as_i64().unwrap())
        .collect();
    assert_eq!(values, vec![30_000, 36_000]);
    // The absorbed property still reflects only its own listing.
    assert_eq!(canonical(&db, b.property_id).3, Some(36_000));
}

#[test]
fn a_listing_that_contradicts_itself_leaves_the_value_unknown() {
    let db = mem();
    let out = add(
        &db,
        "fake",
        "https://fake.test/1",
        "3 quartos no anúncio e 2 quartos na descrição",
        0,
        None,
    );
    assert_eq!(canonical(&db, out.property_id).0, None);
    let p = provenance(&db, out.property_id);
    assert_eq!(p["bedrooms"]["status"], "conflicting");
}

#[test]
fn undoing_a_merge_restores_each_propertys_own_canonical_values() {
    let db = mem();
    let a = add(&db, "alpha", "https://a.test/1", "Casa 3 quartos", 0, None);
    let b = add(&db, "beta", "https://b.test/1", "Casa 2 quartos", 10, None);
    merge_sql(&db, b.property_id, a.property_id);
    db.with_conn(|c| refresh_canonical_chain(c, b.property_id))
        .unwrap();
    assert_eq!(canonical(&db, a.property_id).0, None, "3 vs 2 bedrooms");

    unmerge_sql(&db, b.property_id);
    db.with_conn(|c| {
        refresh_canonical(c, a.property_id)?;
        refresh_canonical(c, b.property_id)
    })
    .unwrap();
    assert_eq!(canonical(&db, a.property_id).0, Some(3));
    assert_eq!(canonical(&db, b.property_id).0, Some(2));
}

// ---- groups and user status ----------------------------------------------------------------

#[test]
fn root_of_follows_merge_chains() {
    let db = mem();
    let (a, b, c3) = (
        add(&db, "a", "https://a.test/1", "Casa", 0, None).property_id,
        add(&db, "b", "https://b.test/1", "Casa", 1, None).property_id,
        add(&db, "c", "https://c.test/1", "Casa", 2, None).property_id,
    );
    merge_sql(&db, b, a);
    merge_sql(&db, a, c3);
    let root = |id| db.with_conn(|c| root_of(c, id)).unwrap();
    assert_eq!((root(b), root(a), root(c3)), (Some(c3), Some(c3), Some(c3)));
    assert_eq!(root(999), None);
}

#[test]
fn a_user_decision_applies_to_the_whole_group_and_restore_clears_hidden() {
    let db = mem();
    let a = add(&db, "a", "https://a.test/1", "Casa", 0, None).property_id;
    let b = add(&db, "b", "https://b.test/1", "Casa", 1, None).property_id;
    merge_sql(&db, b, a);

    // Acting on the absorbed property acts on its group.
    assert_eq!(set_status(&db, b, UserStatus::Hidden, 100), 2);
    let status = |root| {
        db.with_conn(|c| effective_status(c, root))
            .unwrap()
            .unwrap()
    };
    assert_eq!(
        status(a),
        EffectiveStatus {
            status: UserStatus::Hidden,
            conflict: false
        }
    );

    assert_eq!(set_status(&db, a, UserStatus::Favorite, 200), 2);
    assert_eq!(
        status(a),
        EffectiveStatus {
            status: UserStatus::Favorite,
            conflict: false
        }
    );
    let hidden_at: Vec<Option<String>> = db
        .with_conn(|c| {
            Ok(c.prepare("SELECT hidden_at FROM properties ORDER BY id")?
                .query_map([], |r| r.get(0))?
                .collect::<std::result::Result<_, _>>()?)
        })
        .unwrap();
    assert_eq!(
        hidden_at,
        vec![None, None],
        "restoring clears hidden_at (the CHECK keeps both in step)"
    );

    assert_eq!(set_status(&db, a, UserStatus::New, 300), 2);
    assert_eq!(status(a).status, UserStatus::New);
}

#[test]
fn hiding_again_keeps_the_original_timestamp() {
    let db = mem();
    let a = add(&db, "a", "https://a.test/1", "Casa", 0, None).property_id;
    set_status(&db, a, UserStatus::Hidden, 100);
    set_status(&db, a, UserStatus::Hidden, 900);
    let stored: String = db
        .with_conn(|c| {
            Ok(
                c.query_row("SELECT hidden_at FROM properties WHERE id = ?1", [a], |r| {
                    r.get(0)
                })?,
            )
        })
        .unwrap();
    assert_eq!(stored, crate::db::format_timestamp(at(100)));
}

#[test]
fn a_clash_between_members_is_reported_and_favorite_wins() {
    let db = mem();
    let a = add(&db, "a", "https://a.test/1", "Casa", 0, None).property_id;
    let b = add(&db, "b", "https://b.test/1", "Casa", 1, None).property_id;
    // Each side had its own decision before they were recognised as the same property.
    set_status(&db, a, UserStatus::Favorite, 10);
    set_status(&db, b, UserStatus::Hidden, 11);
    merge_sql(&db, b, a);
    let status = db.with_conn(|c| effective_status(c, a)).unwrap().unwrap();
    assert_eq!(
        status,
        EffectiveStatus {
            status: UserStatus::Favorite,
            conflict: true
        }
    );
    // Neither stored decision was touched by the merge.
    let stored: Vec<String> = db
        .with_conn(|c| {
            Ok(c.prepare("SELECT user_status FROM properties ORDER BY id")?
                .query_map([], |r| r.get(0))?
                .collect::<std::result::Result<_, _>>()?)
        })
        .unwrap();
    assert_eq!(stored, vec!["favorite", "hidden"]);
}

#[test]
fn deciding_on_a_missing_property_changes_nothing() {
    let db = mem();
    assert_eq!(set_status(&db, 999, UserStatus::Hidden, 0), 0);
    assert_eq!(db.with_conn(|c| effective_status(c, 999)).unwrap(), None);
}

// ---- the read model ------------------------------------------------------------------------

struct Fixture {
    db: Database,
    agio: i64,
    full350: i64,
    full500: i64,
    unlabelled: i64,
    low_hint: i64,
    merged_root: i64,
    merged_child: i64,
    hidden: i64,
}

fn fixture() -> Fixture {
    let db = seeded();
    let agio = add(
        &db,
        "a",
        "https://a.test/agio",
        "Passo ágio de R$ 80 mil",
        1,
        Some("Setor Bueno, Goiânia"),
    );
    let full350 = add(
        &db,
        "a",
        "https://a.test/350",
        "Casa 3/4. Valor total R$ 350 mil",
        2,
        Some("Setor Bueno, Goiânia"),
    );
    let full500 = add(
        &db,
        "a",
        "https://a.test/500",
        "Valor total R$ 500 mil",
        3,
        Some("Garavelo"),
    );
    let unlabelled = add(
        &db,
        "a",
        "https://a.test/unl",
        "Casa linda R$ 300.000",
        4,
        Some("Cidade Livre, Aparecida de Goiânia"),
    );

    let mut low = normalized("a", "https://a.test/low", "Casa 2 quartos", 5);
    low.location_text = Some("Vila Brasília".into());
    low.facts = parse_listing_with_price_hint(
        StructuredText {
            field: "description",
            text: "Casa 2 quartos",
        },
        &PriceFieldHint {
            field: StructuredText {
                field: "price",
                text: "R$ 280.000",
            },
            confidence: Confidence::Low,
        },
    );
    let low_hint = db
        .with_conn(|c| {
            let loc = LocationIndex::load(c)?.resolve("Vila Brasília");
            upsert_listing(
                c,
                &ListingWrite {
                    listing: &low,
                    source_fields: None,
                    location: loc,
                },
            )
        })
        .unwrap();

    let m1 = add(
        &db,
        "alpha",
        "https://alpha.test/1",
        "Valor total R$ 330 mil",
        6,
        Some("Setor Bueno, Goiânia"),
    );
    let m2 = add(
        &db,
        "beta",
        "https://beta.test/1",
        "Valor total R$ 345 mil",
        7,
        Some("Setor Bueno, Goiânia"),
    );
    merge_sql(&db, m2.property_id, m1.property_id);

    let hidden = add(
        &db,
        "a",
        "https://a.test/hid",
        "Valor total R$ 360 mil",
        8,
        Some("Setor Marista, Goiânia"),
    );
    set_status(&db, hidden.property_id, UserStatus::Hidden, 9);

    Fixture {
        db,
        agio: agio.property_id,
        full350: full350.property_id,
        full500: full500.property_id,
        unlabelled: unlabelled.property_id,
        low_hint: low_hint.property_id,
        merged_root: m1.property_id,
        merged_child: m2.property_id,
        hidden: hidden.property_id,
    }
}

#[test]
fn only_root_properties_are_listed_newest_activity_first_and_hidden_ones_are_left_out() {
    let f = fixture();
    let page = list(&f.db, &PropertyQuery::default());
    assert_eq!(page.total, 6);
    assert_eq!(
        ids(&page),
        vec![
            f.merged_root,
            f.low_hint,
            f.unlabelled,
            f.full500,
            f.full350,
            f.agio
        ],
        "newest activity first; merged child and hidden property absent"
    );
    assert!(!ids(&page).contains(&f.merged_child));
    assert!(!ids(&page).contains(&f.hidden));
    assert_eq!((page.offset, page.limit), (0, 50));
}

#[test]
fn hidden_and_favorite_filters() {
    let f = fixture();
    let with_hidden = list(
        &f.db,
        &PropertyQuery {
            include_hidden: true,
            ..PropertyQuery::default()
        },
    );
    assert_eq!(with_hidden.total, 7);
    assert!(ids(&with_hidden).contains(&f.hidden));
    set_status(&f.db, f.full350, UserStatus::Favorite, 50);
    let favorites = list(
        &f.db,
        &PropertyQuery {
            favorites_only: true,
            ..PropertyQuery::default()
        },
    );
    assert_eq!(ids(&favorites), vec![f.full350]);
}

#[test]
fn paging_reports_the_total_and_clamps_the_page_size() {
    let f = fixture();
    let first = list(
        &f.db,
        &PropertyQuery {
            limit: 2,
            ..PropertyQuery::default()
        },
    );
    assert_eq!((first.items.len(), first.total, first.limit), (2, 6, 2));
    let second = list(
        &f.db,
        &PropertyQuery {
            limit: 2,
            offset: 2,
            ..PropertyQuery::default()
        },
    );
    assert_eq!(second.items.len(), 2);
    assert!(
        first
            .items
            .iter()
            .all(|a| second.items.iter().all(|b| a.id != b.id))
    );
    let beyond = list(
        &f.db,
        &PropertyQuery {
            limit: 2,
            offset: 50,
            ..PropertyQuery::default()
        },
    );
    assert_eq!(beyond.items.len(), 0, "{:?}", beyond.items);
    assert_eq!(beyond.total, 6);
    assert_eq!(
        PropertyQuery {
            limit: 100_000,
            ..PropertyQuery::default()
        }
        .effective_limit(),
        200
    );
    assert_eq!(
        PropertyQuery {
            limit: 0,
            ..PropertyQuery::default()
        }
        .effective_limit(),
        50
    );
}

#[test]
fn full_price_only_keeps_exactly_the_established_full_prices() {
    let f = fixture();
    let page = list(
        &f.db,
        &PropertyQuery {
            full_price_only: true,
            ..PropertyQuery::default()
        },
    );
    let mut got = ids(&page);
    got.sort_unstable();
    let mut want = vec![f.full350, f.full500, f.merged_root];
    want.sort_unstable();
    assert_eq!(got, want);
    for excluded in [f.agio, f.unlabelled, f.low_hint] {
        assert!(
            !got.contains(&excluded),
            "{excluded} must not pass the full-price-only filter"
        );
    }
}

#[test]
fn price_range_only_matches_established_full_prices() {
    let f = fixture();
    let range = |min: i64, max: i64| {
        list(
            &f.db,
            &PropertyQuery {
                min_price: Some(Money::from_reais(min).unwrap()),
                max_price: Some(Money::from_reais(max).unwrap()),
                ..PropertyQuery::default()
            },
        )
    };
    let mut got = ids(&range(340_000, 400_000));
    got.sort_unstable();
    let mut want = vec![f.full350, f.merged_root];
    want.sort_unstable();
    assert_eq!(got, want, "350k and the 345k listing of the merged group");
    assert!(
        range(50_000, 100_000).items.is_empty(),
        "an ágio of R$ 80 mil is not a price in range"
    );
    assert!(
        range(250_000, 310_000).items.is_empty(),
        "unlabelled and low-confidence amounts do not count"
    );
    assert_eq!(ids(&range(480_000, 600_000)), vec![f.full500]);
}

#[test]
fn region_filter_includes_everything_below_the_region() {
    let f = fixture();
    let in_region = |slug: &str| {
        let mut v = ids(&list(
            &f.db,
            &PropertyQuery {
                region_slug: Some(slug.to_owned()),
                ..PropertyQuery::default()
            },
        ));
        v.sort_unstable();
        v
    };
    let mut goiania = vec![f.agio, f.full350, f.merged_root];
    goiania.sort_unstable();
    assert_eq!(in_region("br-go-goiania"), goiania);
    assert_eq!(in_region("br-go-goiania-setor-bueno"), goiania);
    let mut aparecida = vec![f.full500, f.unlabelled, f.low_hint];
    aparecida.sort_unstable();
    assert_eq!(in_region("br-go-aparecida-de-goiania"), aparecida);
    assert_eq!(
        in_region("br-go-aparecida-de-goiania-garavelo"),
        vec![f.full500]
    );
    let none = in_region("no-such-region");
    assert_eq!(none.len(), 0, "{none:?}");
}

#[test]
fn a_merged_property_shows_every_listing_with_its_own_price_and_the_spread() {
    let f = fixture();
    let view =
        f.db.with_conn(|c| get_property_view(c, f.merged_root))
            .unwrap()
            .unwrap();
    assert_eq!(view.listings.len(), 2);
    let adapters: Vec<&str> = view
        .listings
        .iter()
        .map(|l| l.adapter_id.as_str())
        .collect();
    assert!(adapters.contains(&"alpha") && adapters.contains(&"beta"));
    let mut prices: Vec<i64> = view
        .comparable_prices
        .iter()
        .map(|p| p.amount.centavos() / 100)
        .collect();
    prices.sort_unstable();
    assert_eq!(prices, vec![330_000, 345_000]);
    let (low, high) = view.price_spread.unwrap();
    assert_eq!(
        (low.centavos() / 100, high.centavos() / 100),
        (330_000, 345_000)
    );
    assert_eq!(view.municipality.as_deref(), Some("Goiânia"));
    assert_eq!(view.neighborhood.as_deref(), Some("Setor Bueno"));
    // An absorbed property is not a root: it has no view of its own.
    assert!(
        f.db.with_conn(|c| get_property_view(c, f.merged_child))
            .unwrap()
            .is_none()
    );
}

#[test]
fn unavailable_listings_provide_no_comparable_price_but_stay_listed() {
    let f = fixture();
    let beta =
        f.db.with_conn(|c| crate::db::listings::find_listing(c, "beta", "https://beta.test/1"))
            .unwrap()
            .unwrap();
    f.db.with_conn(|c| mark_unavailable(c, beta.id, at(500), "HTTP 410"))
        .unwrap();
    let view =
        f.db.with_conn(|c| get_property_view(c, f.merged_root))
            .unwrap()
            .unwrap();
    assert_eq!(view.listings.len(), 2, "history stays visible");
    assert_eq!(view.comparable_prices.len(), 1);
    assert_eq!(view.comparable_prices[0].amount.centavos() / 100, 330_000);
    // And a price range no longer matches the unavailable 345k listing.
    let page = list(
        &f.db,
        &PropertyQuery {
            min_price: Some(Money::from_reais(340_000).unwrap()),
            max_price: Some(Money::from_reais(349_000).unwrap()),
            ..PropertyQuery::default()
        },
    );
    assert_eq!(page.items.len(), 0, "{:?}", page.items);
}

#[test]
fn the_view_carries_canonical_fields_and_the_effective_status() {
    let f = fixture();
    let view =
        f.db.with_conn(|c| get_property_view(c, f.full350))
            .unwrap()
            .unwrap();
    assert_eq!(view.bedrooms, Some(3));
    assert_eq!(view.lot_area, None);
    assert_eq!(view.status, UserStatus::New);
    assert_eq!(view.comparable_prices.len(), 1);
    assert_eq!(
        view.listings[0].price.as_ref().unwrap().classification,
        PriceClassification::FullPrice
    );
    let json = serde_json::to_value(&view).unwrap();
    assert_eq!(json["status"], "new");
    assert_eq!(json["bedrooms"], 3);
}

#[test]
fn properties_whose_only_listings_failed_extraction_are_not_listed() {
    let db = mem();
    let mut failed = normalized("a", "https://a.test/f", "", 0);
    failed.extraction_status = crate::models::listing::ExtractionStatus::Failed;
    failed.extraction_error = Some("unparseable".into());
    failed.last_verified_at = None;
    db.with_conn(|c| {
        upsert_listing(
            c,
            &ListingWrite {
                listing: &failed,
                source_fields: None,
                location: None,
            },
        )
    })
    .unwrap();
    add(
        &db,
        "a",
        "https://a.test/ok",
        "Valor total R$ 350 mil",
        1,
        None,
    );
    assert_eq!(list(&db, &PropertyQuery::default()).total, 1);
}

#[test]
fn the_group_status_decides_visibility() {
    let f = fixture();
    // One member of a group is hidden: the whole group disappears from the default list.
    f.db.with_conn(|c| {
        c.execute(
            "UPDATE properties SET user_status = 'hidden', hidden_at = ?1 WHERE id = ?2",
            rusqlite::params![TS, f.merged_child],
        )?;
        Ok(())
    })
    .unwrap();
    let page = list(&f.db, &PropertyQuery::default());
    assert!(!ids(&page).contains(&f.merged_root));
    let all = list(
        &f.db,
        &PropertyQuery {
            include_hidden: true,
            ..PropertyQuery::default()
        },
    );
    assert!(ids(&all).contains(&f.merged_root));
}

// ---- local score in the read model ---------------------------------------------------------

mod scores {
    use super::*;
    use crate::ranking::{
        RANKING_VERSION,
        score::{ConfidenceReason, Ranked, ScoreDetails},
        store::save_ranking,
    };

    fn ranked(property_id: i64, score: Option<f64>, confidence: Confidence) -> Ranked {
        Ranked {
            property_id,
            score,
            confidence,
            details: ScoreDetails {
                ranking_version: RANKING_VERSION.to_owned(),
                comparable_price_centavos: Some(35_000_000),
                components: vec![],
                available_weight_share: 0.6,
                confidence_reasons: vec![ConfidenceReason::PropertyTypeUnknown],
            },
        }
    }

    fn save(db: &Database, ranked: &[Ranked]) {
        db.with_conn(|c| save_ranking(c, ranked, at(500))).unwrap();
    }

    fn sorted_by_score() -> PropertyQuery {
        PropertyQuery {
            sort: PropertySort::Score,
            ..PropertyQuery::default()
        }
    }

    /// Three listings seen at increasing times, so the recent order is `[c, b, a]`.
    fn three(db: &Database) -> [i64; 3] {
        let one = |n: i64| {
            add(
                db,
                "fake",
                &format!("https://fake.test/{n}"),
                "Casa 3 quartos. Valor total R$ 350 mil",
                n * 10,
                None,
            )
            .property_id
        };
        [one(1), one(2), one(3)]
    }

    #[test]
    fn a_property_view_carries_its_stored_score_confidence_and_explanation() {
        let db = mem();
        let [a, ..] = three(&db);
        save(&db, &[ranked(a, Some(72.5), Confidence::Medium)]);
        let view = list(&db, &PropertyQuery::default())
            .items
            .into_iter()
            .find(|p| p.id == a)
            .unwrap();
        assert_eq!(view.local_score, Some(72.5));
        assert_eq!(view.data_confidence, Some(Confidence::Medium));
        assert_eq!(view.scored_at, Some(at(500)));
        let details = view
            .score_details
            .expect("the explanation is part of the view");
        assert_eq!(details.comparable_price_centavos, Some(35_000_000));
        assert_eq!(
            details.confidence_reasons,
            vec![ConfidenceReason::PropertyTypeUnknown]
        );
    }

    #[test]
    fn an_unscored_property_has_no_score_fields_at_all() {
        let db = mem();
        let [a, ..] = three(&db);
        let view = db.with_conn(|c| get_property_view(c, a)).unwrap().unwrap();
        assert_eq!(
            (
                view.local_score,
                view.data_confidence,
                view.score_details,
                view.scored_at
            ),
            (None, None, None, None)
        );
    }

    #[test]
    fn the_default_order_stays_most_recent_first_whatever_the_scores() {
        let db = mem();
        let [a, b, c] = three(&db);
        save(&db, &[ranked(a, Some(90.0), Confidence::High)]);
        assert_eq!(ids(&list(&db, &PropertyQuery::default())), vec![c, b, a]);
    }

    #[test]
    fn sorting_by_score_puts_the_highest_first_and_unscored_properties_last() {
        let db = mem();
        let [a, b, c] = three(&db);
        save(
            &db,
            &[
                ranked(a, Some(40.0), Confidence::Low),
                ranked(b, Some(85.5), Confidence::Low),
            ],
        );
        // `c` has no score: it comes last, not first and not as a zero.
        assert_eq!(ids(&list(&db, &sorted_by_score())), vec![b, a, c]);
    }

    #[test]
    fn equal_scores_fall_back_to_the_most_recent_activity() {
        let db = mem();
        let [a, b, c] = three(&db);
        save(
            &db,
            &[
                ranked(a, Some(50.0), Confidence::Low),
                ranked(b, Some(50.0), Confidence::Low),
                ranked(c, Some(50.0), Confidence::Low),
            ],
        );
        assert_eq!(ids(&list(&db, &sorted_by_score())), vec![c, b, a]);
    }

    #[test]
    fn paging_follows_the_score_order() {
        let db = mem();
        let [a, b, c] = three(&db);
        save(
            &db,
            &[
                ranked(a, Some(10.0), Confidence::Low),
                ranked(b, Some(30.0), Confidence::Low),
                ranked(c, Some(20.0), Confidence::Low),
            ],
        );
        let second = PropertyQuery {
            limit: 1,
            offset: 1,
            ..sorted_by_score()
        };
        let page = list(&db, &second);
        assert_eq!((ids(&page), page.total), (vec![c], 3));
    }
}
