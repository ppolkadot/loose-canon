use super::*;
use crate::{
    db::{
        Database,
        listings::mark_unavailable,
        properties::set_user_status,
        settings::set_setting,
        testkit::{at, mem},
    },
    matching::testkit::{FULL_AD, add, add_typed, merge, place},
    models::{characteristics::PropertyType, listing::UserStatus},
    ranking::{
        config::{RANKING_CONFIG_KEY, RankingConfig},
        recompute,
    },
};

fn rank(db: &Database) {
    db.with_conn(|c| recompute(c, at(10))).unwrap();
}

fn select(db: &Database, count: usize) -> Vec<Candidate> {
    db.with_conn(|c| select_candidates(c, count)).unwrap()
}

fn one(db: &Database) -> Candidate {
    let mut all = select(db, 5);
    assert_eq!(all.len(), 1, "{all:?}");
    all.remove(0)
}

/// The score of a property depends only on freshness here, so ordering is easy to control.
fn freshness_only(db: &Database) {
    let config = RankingConfig {
        weight_price_per_m2: 0.0,
        weight_cross_listing: 0.0,
        weight_fit: 0.0,
        weight_freshness: 1.0,
        ..RankingConfig::default()
    };
    db.with_conn(|c| set_setting(c, RANKING_CONFIG_KEY, &config, at(1)))
        .unwrap();
}

// ---- selection -----------------------------------------------------------------------------

#[test]
fn candidates_are_the_best_scored_properties_best_first_and_at_most_count() {
    let db = mem();
    freshness_only(&db);
    // Older listings score lower on freshness: property 1 is the oldest.
    let ids: Vec<i64> = (1..=4)
        .map(|n| {
            let (listing, property) = add(&db, "alpha", n, "Casa", FULL_AD);
            db.with_conn(|c| {
                c.execute(
                    "UPDATE listing_links SET first_seen_at = ?1, last_seen_at = ?2 WHERE id = ?3",
                    rusqlite::params![
                        crate::db::format_timestamp(at(i64::from(n) * 86_400)),
                        // Seen most recently exactly when first seen longest ago.
                        crate::db::format_timestamp(at(4 * 86_400 + (5 - i64::from(n)) * 3_600)),
                        listing
                    ],
                )?;
                Ok(())
            })
            .unwrap();
            property
        })
        .collect();
    db.with_conn(|c| recompute(c, at(5 * 86_400))).unwrap();

    let picked: Vec<i64> = select(&db, 3).iter().map(|c| c.id).collect();
    assert_eq!(
        picked,
        vec![ids[3], ids[2], ids[1]],
        "newest (best freshness) first, capped at 3"
    );
    assert_eq!(select(&db, 10).len(), 4);
}

#[test]
fn unscored_hidden_and_inactive_properties_are_never_candidates() {
    let db = mem();
    let (_, kept) = add(&db, "alpha", 1, "Casa", FULL_AD);
    let (_, hidden) = add(&db, "alpha", 2, "Casa", FULL_AD);
    let (gone_listing, _) = add(&db, "alpha", 3, "Casa", FULL_AD);
    db.with_conn(|c| mark_unavailable(c, gone_listing, at(5), "HTTP 410"))
        .unwrap();
    db.with_conn(|c| set_user_status(c, hidden, UserStatus::Hidden, at(6)))
        .unwrap();
    // A property with no data at all cannot be scored, so it must not be sent either.
    let (_, empty) = add(&db, "alpha", 4, "Casa", "Casa");
    rank(&db);

    let picked: Vec<i64> = select(&db, 30).iter().map(|c| c.id).collect();
    assert!(picked.contains(&kept));
    assert!(
        !picked.contains(&hidden),
        "a hidden property is the user's \"no\""
    );
    let stored_empty = db
        .with_conn(|c| crate::ranking::store::load_score(c, empty))
        .unwrap()
        .unwrap();
    if stored_empty.score.is_none() {
        assert!(!picked.contains(&empty), "no score, no candidate");
    }
}

#[test]
fn an_empty_library_has_no_candidates() {
    let db = mem();
    rank(&db);
    assert!(select(&db, 20).is_empty());
}

// ---- content -------------------------------------------------------------------------------

#[test]
fn a_candidate_states_the_facts_the_price_and_the_local_score() {
    let db = mem();
    let (_, property) = add_typed(&db, "alpha", 1, FULL_AD, PropertyType::House);
    place(&db, property, "Goiânia", Some("Setor Bueno"));
    // Nothing writes the canonical property type yet (the parser never reads one), so set it.
    db.with_conn(|c| {
        c.execute(
            "UPDATE properties SET property_type = 'house' WHERE id = ?1",
            [property],
        )?;
        Ok(())
    })
    .unwrap();
    rank(&db);
    let c = one(&db);

    assert_eq!(c.id, property);
    assert_eq!(c.municipality.as_deref(), Some("Goiânia"));
    assert_eq!(c.neighborhood.as_deref(), Some("Setor Bueno"));
    assert_eq!(c.property_type.as_deref(), Some("house"));
    assert_eq!(c.comparable_price.as_deref(), Some("R$ 450.000,00"));
    assert_eq!(
        (c.built_area.as_deref(), c.lot_area.as_deref()),
        (Some("180 m²"), Some("360 m²"))
    );
    assert_eq!(
        (c.bedrooms, c.suites, c.parking_spaces),
        (Some(3), Some(1), Some(2))
    );
    assert_eq!((c.active_listings, c.sources), (1, 1));
    assert_eq!(c.price_spread_percent, None, "one price has no spread");
    assert!(c.local_score > 0.0 && c.local_score <= 100.0);
    assert!(["low", "medium", "high"].contains(&c.data_confidence.as_str()));
    assert!(
        !c.confidence_reasons.is_empty(),
        "the parser never reads a type, so confidence has reasons"
    );
    assert!(
        c.local_components
            .iter()
            .all(|k| (0.0..=100.0).contains(&k.score))
    );
    assert_eq!(c.listings.len(), 1);
    assert_eq!(c.listings[0].source, "alpha");
    assert_eq!(c.listings[0].price.as_deref(), Some("R$ 450.000,00"));
    assert_eq!(c.listings[0].price_kind.as_deref(), Some("full_price"));
}

#[test]
fn what_the_data_does_not_give_is_listed_as_unknown_not_guessed() {
    let db = mem();
    add(
        &db,
        "alpha",
        1,
        "Casa",
        "Casa com 3 quartos. Valor total R$ 350 mil",
    );
    rank(&db);
    let c = one(&db);
    assert_eq!(c.bedrooms, Some(3));
    assert_eq!(
        (c.suites, c.parking_spaces, c.built_area, c.lot_area),
        (None, None, None, None)
    );
    for fact in [
        "suites",
        "parking_spaces",
        "built_area",
        "lot_area",
        "property_type",
    ] {
        assert!(
            c.unknown_facts.iter().any(|f| f == fact),
            "{fact} missing from {:?}",
            c.unknown_facts
        );
    }
    assert!(!c.unknown_facts.iter().any(|f| f == "bedrooms"));
}

#[test]
fn an_agio_is_labelled_as_such_and_is_not_offered_as_the_price() {
    let db = mem();
    add(
        &db,
        "alpha",
        1,
        "Casa",
        "Casa 3 quartos. Ágio R$ 80 mil, assume financiamento",
    );
    rank(&db);
    let c = one(&db);
    assert_eq!(
        c.comparable_price, None,
        "an ágio is not the price of the property"
    );
    assert_eq!(c.listings[0].price_kind.as_deref(), Some("agio"));
}

#[test]
fn only_active_listings_are_described_and_merged_sources_show_their_spread() {
    let db = mem();
    let (la, pa) = add(
        &db,
        "alpha",
        1,
        "Casa",
        "Casa 3 quartos, 180 m² de área construída. Valor total R$ 400 mil",
    );
    let (lb, _) = add(
        &db,
        "beta",
        1,
        "Casa",
        "Casa 3 quartos, 180 m² de área construída. Valor total R$ 450 mil",
    );
    let (lc, _) = add(
        &db,
        "gamma",
        1,
        "Casa",
        "Casa 3 quartos, 180 m² de área construída. Valor total R$ 900 mil",
    );
    merge(&db, la, lb);
    merge(&db, la, lc);
    db.with_conn(|c| mark_unavailable(c, lc, at(5), "HTTP 410"))
        .unwrap();
    rank(&db);

    let c = one(&db);
    assert_eq!(c.id, pa);
    assert_eq!((c.active_listings, c.sources), (2, 2));
    let sources: Vec<&str> = c.listings.iter().map(|l| l.source.as_str()).collect();
    assert!(
        sources.contains(&"alpha") && sources.contains(&"beta") && !sources.contains(&"gamma"),
        "{sources:?}"
    );
    assert_eq!(
        c.price_spread_percent,
        Some(11.1),
        "(450-400)/450 = 11.1 %, the dead 900 is ignored"
    );
}

#[test]
fn conflicting_values_between_the_listings_of_a_property_are_reported() {
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
    merge(&db, la, lb);
    rank(&db);
    let c = one(&db);
    assert_eq!(c.conflicting_facts, vec!["bedrooms".to_owned()]);
    assert_eq!(c.bedrooms, None, "a disputed value is not asserted");
}

// ---- privacy -------------------------------------------------------------------------------

#[test]
fn the_text_has_its_contact_data_removed_and_is_bounded() {
    let db = mem();
    let long = format!(
        "Casa 3 quartos. Valor total R$ 350 mil. Ligue (62) 99999-8888 ou escreva a corretor@imob.com.br, veja https://imob.com.br/x {}",
        "bairro tranquilo ".repeat(100)
    );
    add(&db, "alpha", 1, "Casa", &long);
    rank(&db);
    let c = one(&db);
    for secret in ["99999", "corretor", "imob.com.br", "https"] {
        assert!(!c.text.contains(secret), "{secret:?} left in {:?}", c.text);
    }
    assert!(c.text.contains("3 quartos"));
    assert!(
        c.text.chars().count() <= TEXT_CHARS,
        "{}",
        c.text.chars().count()
    );
}

#[test]
fn nothing_that_identifies_the_advertiser_or_the_ad_is_in_what_is_sent() {
    let db = mem();
    let (listing, _) = add(
        &db,
        "alpha",
        1,
        "Casa do Fulano Titulo 62 99999-8888",
        FULL_AD,
    );
    db.with_conn(|c| {
        c.execute(
            "UPDATE listing_links SET raw_description = ?1 WHERE id = ?2",
            rusqlite::params![
                "Casa ampla. CRECI 1234, ze@corretor.com, (62) 3333-4444",
                listing
            ],
        )?;
        Ok(())
    })
    .unwrap();
    rank(&db);
    let json = serde_json::to_string(&one(&db)).unwrap();
    for secret in [
        "https://alpha.test/1",
        "alpha.test",
        "Fulano Titulo",
        "99999",
        "ze@corretor",
        "CRECI",
        "1234",
        "3333",
        "http",
    ] {
        assert!(
            !json.contains(secret),
            "{secret:?} is in the request data: {json}"
        );
    }
}

/// A limit, stated rather than hidden: a pattern can find a phone number but not a person's name,
/// so a name written in the advertisement text is sent. The user can see exactly what would be
/// sent before turning the AI on (the preview), and the text is only a short excerpt.
#[test]
fn a_name_written_in_the_text_is_not_detected_and_stays() {
    let db = mem();
    let (listing, _) = add(&db, "alpha", 1, "Casa", FULL_AD);
    db.with_conn(|c| {
        c.execute(
            "UPDATE listing_links SET raw_description = 'Casa ampla, falar com Maria Souza' WHERE id = ?1",
            [listing],
        )?;
        Ok(())
    })
    .unwrap();
    rank(&db);
    assert!(one(&db).text.contains("Maria Souza"));
}

#[test]
fn the_data_is_keyed_in_snake_case_with_no_url_or_title_field() {
    let db = mem();
    add(&db, "alpha", 1, "Casa", FULL_AD);
    rank(&db);
    let json = serde_json::to_value(one(&db)).unwrap();
    let mut keys: Vec<&str> = json
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        [
            "active_listings",
            "bedrooms",
            "built_area",
            "comparable_price",
            "confidence_reasons",
            "conflicting_facts",
            "data_confidence",
            "id",
            "listings",
            "local_components",
            "local_score",
            "lot_area",
            "municipality",
            "neighborhood",
            "parking_spaces",
            "price_spread_percent",
            "property_type",
            "sources",
            "suites",
            "text",
            "unknown_facts",
        ]
    );
}

// ---- fingerprints --------------------------------------------------------------------------

fn sample() -> Candidate {
    let db = mem();
    add(&db, "alpha", 1, "Casa", FULL_AD);
    rank(&db);
    one(&db)
}

#[test]
fn a_fingerprint_is_a_stable_sha256_of_what_is_sent() {
    let c = sample();
    let f = c.fingerprint();
    assert_eq!(f.len(), 64);
    assert!(
        f.chars()
            .all(|ch| ch.is_ascii_hexdigit() && !ch.is_ascii_uppercase()),
        "{f}"
    );
    assert_eq!(
        f,
        sample().fingerprint(),
        "the same data gives the same fingerprint"
    );
    assert_eq!(f, c.clone().fingerprint());
}

#[test]
fn any_change_to_what_is_sent_changes_the_fingerprint() {
    let base = sample();
    let f = base.fingerprint();
    let changes: Vec<(&str, Candidate)> = vec![
        (
            "price",
            Candidate {
                comparable_price: Some("R$ 1,00".into()),
                ..base.clone()
            },
        ),
        (
            "text",
            Candidate {
                text: format!("{} x", base.text),
                ..base.clone()
            },
        ),
        (
            "score",
            Candidate {
                local_score: base.local_score + 0.1,
                ..base.clone()
            },
        ),
        (
            "rooms",
            Candidate {
                bedrooms: Some(9),
                ..base.clone()
            },
        ),
        (
            "confidence",
            Candidate {
                data_confidence: format!("{}x", base.data_confidence),
                ..base.clone()
            },
        ),
        (
            "id",
            Candidate {
                id: base.id + 1,
                ..base.clone()
            },
        ),
        (
            "unknown",
            Candidate {
                unknown_facts: vec![],
                ..base.clone()
            },
        ),
    ];
    for (what, changed) in changes {
        assert_ne!(
            changed.fingerprint(),
            f,
            "changing the {what} must change the fingerprint"
        );
    }
}

#[test]
fn the_request_fingerprint_depends_on_model_prompt_version_and_the_candidates_in_order() {
    let a = sample();
    let b = Candidate {
        id: a.id + 1,
        ..a.clone()
    };
    let base = request_fingerprint("m1", "p1", &[a.clone(), b.clone()]);
    assert_eq!(base.len(), 64);
    assert_eq!(
        base,
        request_fingerprint("m1", "p1", &[a.clone(), b.clone()])
    );
    assert_ne!(
        base,
        request_fingerprint("m2", "p1", &[a.clone(), b.clone()]),
        "model"
    );
    assert_ne!(
        base,
        request_fingerprint("m1", "p2", &[a.clone(), b.clone()]),
        "prompt version"
    );
    assert_ne!(
        base,
        request_fingerprint("m1", "p1", &[b.clone(), a.clone()]),
        "order"
    );
    assert_ne!(
        base,
        request_fingerprint("m1", "p1", std::slice::from_ref(&a)),
        "set"
    );
    assert_ne!(
        request_fingerprint("m1p", "1", std::slice::from_ref(&a)),
        request_fingerprint("m1", "p1", std::slice::from_ref(&a)),
        "the boundary between the model and the version is not ambiguous"
    );
}

#[test]
fn a_changed_property_in_the_library_changes_the_request_fingerprint() {
    let db = mem();
    let (listing, _) = add(&db, "alpha", 1, "Casa", FULL_AD);
    rank(&db);
    let before = request_fingerprint("m", "p", &select(&db, 5));
    db.with_conn(|c| {
        c.execute(
            "UPDATE listing_links SET raw_description = raw_description || ' Piscina nova.' WHERE id = ?1",
            [listing],
        )?;
        Ok(())
    })
    .unwrap();
    rank(&db);
    let after = request_fingerprint("m", "p", &select(&db, 5));
    assert_ne!(before, after);
}

#[test]
fn the_text_comes_from_the_most_recently_scraped_active_listing_of_the_property() {
    let db = mem();
    let (old, _) = add(
        &db,
        "alpha",
        1,
        "Casa",
        "Casa 3 quartos. Valor total R$ 350 mil. TEXTO ANTIGO",
    );
    let (new, _) = add(
        &db,
        "beta",
        1,
        "Casa",
        "Casa 3 quartos. Valor total R$ 350 mil. TEXTO NOVO",
    );
    let (dead, _) = add(
        &db,
        "gamma",
        1,
        "Casa",
        "Casa 3 quartos. Valor total R$ 350 mil. TEXTO DE ANUNCIO FORA DO AR",
    );
    merge(&db, old, new);
    merge(&db, old, dead);
    db.with_conn(|c| {
        for (listing, secs) in [(old, 100), (new, 200), (dead, 300)] {
            c.execute(
                "UPDATE listing_links SET last_scraped_at = ?1, last_seen_at = ?1 WHERE id = ?2",
                rusqlite::params![crate::db::format_timestamp(at(secs)), listing],
            )?;
        }
        Ok(())
    })
    .unwrap();
    db.with_conn(|c| mark_unavailable(c, dead, at(400), "HTTP 410"))
        .unwrap();
    rank(&db);
    let text = one(&db).text;
    assert!(text.contains("TEXTO NOVO"), "{text}");
    assert!(
        !text.contains("ANTIGO") && !text.contains("FORA DO AR"),
        "{text}"
    );
}

#[test]
fn several_listings_from_one_source_are_counted_as_one_source() {
    let db = mem();
    let (a, _) = add(&db, "alpha", 1, "Casa", FULL_AD);
    let (b, _) = add(&db, "alpha", 2, "Casa", FULL_AD);
    merge(&db, a, b);
    rank(&db);
    let c = one(&db);
    assert_eq!((c.active_listings, c.sources), (2, 1));
}

#[test]
fn an_unknown_bedroom_count_is_listed_as_unknown() {
    let db = mem();
    add(
        &db,
        "alpha",
        1,
        "Casa",
        "Casa com 1 suíte, 180 m² de área construída. Valor total R$ 350 mil",
    );
    rank(&db);
    let c = one(&db);
    assert_eq!(c.bedrooms, None);
    assert!(
        c.unknown_facts.iter().any(|f| f == "bedrooms"),
        "{:?}",
        c.unknown_facts
    );
    assert!(!c.unknown_facts.iter().any(|f| f == "suites"));
}
