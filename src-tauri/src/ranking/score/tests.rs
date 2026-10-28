use chrono::{Duration, TimeZone};

use super::*;

const ALL: &[ComponentKind] = &[
    ComponentKind::PricePerM2,
    ComponentKind::CrossListing,
    ComponentKind::Fit,
    ComponentKind::Freshness,
];

fn now() -> DateTime<Utc> {
    Utc.timestamp_opt(1_790_000_000, 0).unwrap()
}

fn days_ago(days: i64) -> DateTime<Utc> {
    now() - Duration::days(days)
}

fn config() -> RankingConfig {
    RankingConfig::default()
}

fn no_preferences() -> SearchPreferences {
    SearchPreferences::default()
}

fn wants_three_bedrooms() -> SearchPreferences {
    SearchPreferences {
        bedrooms_min: Some(3),
        ..SearchPreferences::default()
    }
}

fn listing(adapter: &str, price: Option<i64>, first_seen_days_ago: i64) -> ListingInput {
    ListingInput {
        adapter_id: adapter.to_owned(),
        status: ListingStatus::Active,
        established_price: price.map(|p| (p, Confidence::High)),
        first_seen_at: days_ago(first_seen_days_ago),
        last_scraped_at: Some(days_ago(1)),
    }
}

/// A 100 m² house in Setor Bueno, Goiânia: 3 bedrooms, 1 suite, 2 parking spaces, one active
/// listing seen yesterday at `price` centavos.
fn prop(id: i64, price: i64) -> RankInput {
    RankInput {
        property_id: id,
        municipality: Some("Goiânia".into()),
        neighborhood: Some("Setor Bueno".into()),
        property_type: Some(PropertyType::House),
        bedrooms: Some(3),
        suites: Some(1),
        parking_spaces: Some(2),
        built_area_centi_m2: Some(10_000),
        lot_area_centi_m2: None,
        listings: vec![listing("alpha", Some(price), 1)],
        conflicting_fields: vec![],
        verified_merge: false,
    }
}

/// `n` comparable properties, ids 1..=n, priced R$ 300 mil, 310 mil, ... (id 1 is the cheapest).
fn market(n: i64) -> Vec<RankInput> {
    (1..=n)
        .map(|i| prop(i, 30_000_000 + (i - 1) * 1_000_000))
        .collect()
}

fn score_of(inputs: &[RankInput], preferences: &SearchPreferences, id: i64) -> Ranked {
    rank(inputs, preferences, &config(), now())
        .into_iter()
        .find(|r| r.property_id == id)
        .unwrap_or_else(|| panic!("property {id} was not ranked"))
}

fn component(r: &Ranked, kind: ComponentKind) -> &Component {
    r.details
        .components
        .iter()
        .find(|c| c.kind == kind)
        .unwrap()
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 0.06
}

// ------------------------------------------------------------------------------------------
// Structure
// ------------------------------------------------------------------------------------------

#[test]
fn every_property_always_reports_all_four_components_and_the_rules_version() {
    let ranked = score_of(&market(8), &no_preferences(), 3);
    let kinds: Vec<ComponentKind> = ranked.details.components.iter().map(|c| c.kind).collect();
    assert_eq!(kinds, ALL);
    assert_eq!(ranked.details.ranking_version, RANKING_VERSION);
    let weights: Vec<f64> = ranked.details.components.iter().map(|c| c.weight).collect();
    assert_eq!(weights, vec![0.45, 0.25, 0.15, 0.15]);
}

#[test]
fn only_properties_with_an_active_listing_are_ranked() {
    let mut inputs = market(6);
    inputs[0].listings[0].status = ListingStatus::Unavailable;
    inputs[1].listings[0].status = ListingStatus::Unknown;
    let ranked = rank(&inputs, &no_preferences(), &config(), now());
    let ids: Vec<i64> = ranked.iter().map(|r| r.property_id).collect();
    assert!(!ids.contains(&1) && !ids.contains(&2), "{ids:?}");
    assert_eq!(ids.len(), 4);
}

#[test]
fn the_result_is_ordered_by_score_then_id_and_is_deterministic() {
    let inputs = market(9);
    let first = rank(&inputs, &no_preferences(), &config(), now());
    assert_eq!(first, rank(&inputs, &no_preferences(), &config(), now()));
    let scores: Vec<f64> = first.iter().map(|r| r.score.unwrap()).collect();
    assert!(scores.windows(2).all(|w| w[0] >= w[1]), "{scores:?}");
    assert_eq!(first[0].property_id, 1, "the cheapest per m² comes first");
    // Equal scores are ordered by id.
    let twins = vec![prop(5, 31_000_000), prop(2, 31_000_000)];
    let ids: Vec<i64> = rank(&twins, &no_preferences(), &config(), now())
        .iter()
        .map(|r| r.property_id)
        .collect();
    assert_eq!(ids, vec![2, 5]);
}

#[test]
fn scores_stay_within_zero_and_a_hundred_with_one_decimal() {
    for r in rank(&market(12), &wants_three_bedrooms(), &config(), now()) {
        let s = r.score.unwrap();
        assert!((0.0..=100.0).contains(&s), "{s}");
        assert!(
            (s * 10.0 - (s * 10.0).round()).abs() < 1e-9,
            "{s} has more than one decimal"
        );
    }
}

// ------------------------------------------------------------------------------------------
// Unavailable is not zero
// ------------------------------------------------------------------------------------------

#[test]
fn an_unavailable_component_is_left_out_and_the_weights_are_renormalised() {
    // No preferences and no merge: price per m² (0.45) and freshness (0.15) are available.
    let ranked = score_of(&market(11), &no_preferences(), 6);
    let price = component(&ranked, ComponentKind::PricePerM2).score.unwrap();
    let fresh = component(&ranked, ComponentKind::Freshness).score.unwrap();
    assert!(close(price, 50.0), "{price}");
    let expected = (0.45 * price + 0.15 * fresh) / 0.60;
    assert!(
        close(ranked.score.unwrap(), expected),
        "{} vs {expected}",
        ranked.score.unwrap()
    );
    for kind in [ComponentKind::CrossListing, ComponentKind::Fit] {
        let c = component(&ranked, kind);
        assert_eq!(
            (c.score, c.unavailable_reason.is_some()),
            (None, true),
            "{kind:?}"
        );
    }
    assert!(
        close(ranked.details.available_weight_share, 0.60),
        "{}",
        ranked.details.available_weight_share
    );
}

#[test]
fn a_property_with_nothing_computable_has_no_score() {
    let mut p = prop(1, 30_000_000);
    p.built_area_centi_m2 = None;
    p.listings[0].established_price = None;
    let config = RankingConfig {
        weight_freshness: 0.0,
        ..config()
    };
    let ranked = rank(&[p], &no_preferences(), &config, now());
    assert_eq!(ranked[0].score, None);
    assert_eq!(ranked[0].confidence, Confidence::Low);
}

#[test]
fn weights_are_configurable() {
    let only_freshness = RankingConfig {
        weight_price_per_m2: 0.0,
        weight_cross_listing: 0.0,
        weight_fit: 0.0,
        weight_freshness: 1.0,
        ..config()
    };
    let inputs = market(11);
    let ranked = rank(&inputs, &no_preferences(), &only_freshness, now());
    let r = ranked.iter().find(|r| r.property_id == 4).unwrap();
    let fresh = component(r, ComponentKind::Freshness).score.unwrap();
    assert!(
        close(r.score.unwrap(), fresh),
        "{} vs {fresh}",
        r.score.unwrap()
    );
}

// ------------------------------------------------------------------------------------------
// Price per m²
// ------------------------------------------------------------------------------------------

#[test]
fn price_per_m2_is_the_position_inside_the_comparable_group_cheaper_is_higher() {
    let inputs = market(11);
    let cheapest = component(
        &score_of(&inputs, &no_preferences(), 1),
        ComponentKind::PricePerM2,
    )
    .score
    .unwrap();
    let median = component(
        &score_of(&inputs, &no_preferences(), 6),
        ComponentKind::PricePerM2,
    )
    .score
    .unwrap();
    let dearest = component(
        &score_of(&inputs, &no_preferences(), 11),
        ComponentKind::PricePerM2,
    )
    .score
    .unwrap();
    assert!(close(cheapest, 100.0 * (1.0 - 0.5 / 11.0)), "{cheapest}");
    assert!(close(median, 50.0), "{median}");
    assert!(close(dearest, 100.0 * (0.5 / 11.0)), "{dearest}");
}

#[test]
fn price_per_m2_normalises_by_area_not_by_price() {
    // Same price, different built areas: the bigger one is cheaper per m².
    let mut inputs = market(10);
    inputs.push(RankInput {
        built_area_centi_m2: Some(20_000),
        ..prop(11, 35_000_000)
    });
    inputs.push(RankInput {
        built_area_centi_m2: Some(7_000),
        ..prop(12, 35_000_000)
    });
    let big = component(
        &score_of(&inputs, &no_preferences(), 11),
        ComponentKind::PricePerM2,
    )
    .score
    .unwrap();
    let small = component(
        &score_of(&inputs, &no_preferences(), 12),
        ComponentKind::PricePerM2,
    )
    .score
    .unwrap();
    assert!(big > small + 40.0, "{big} vs {small}");
}

#[test]
fn it_needs_enough_comparables_and_says_how_many_there_were() {
    let few = score_of(&market(4), &no_preferences(), 2);
    let c = component(&few, ComponentKind::PricePerM2);
    assert_eq!(
        (c.score, c.unavailable_reason),
        (None, Some(UnavailableReason::TooFewComparables))
    );
    assert_eq!(c.evidence["built"]["comparables"], 4);
    let enough = score_of(&market(5), &no_preferences(), 2);
    let c = component(&enough, ComponentKind::PricePerM2);
    assert!(c.score.is_some());
    assert_eq!(
        c.evidence["built"]["segment"], "neighborhood",
        "exactly the minimum is enough at the neighbourhood level"
    );
}

#[test]
fn built_and_lot_area_are_compared_separately_and_never_mixed() {
    // Five properties known only by lot area, five only by built area: each group is its own segment.
    let mut inputs = Vec::new();
    for i in 1..=5 {
        inputs.push(RankInput {
            built_area_centi_m2: None,
            lot_area_centi_m2: Some(30_000),
            ..prop(i, 30_000_000 + i * 1_000_000)
        });
    }
    for i in 6..=10 {
        inputs.push(prop(i, 80_000_000 + i * 1_000_000));
    }
    let lot_only = score_of(&inputs, &no_preferences(), 3);
    let c = component(&lot_only, ComponentKind::PricePerM2);
    assert!(c.score.is_some());
    assert_eq!(c.evidence["built"]["unavailable"], "area_unknown");
    assert_eq!(
        c.evidence["lot"]["comparables"], 5,
        "the built-area houses are not lot comparables"
    );
    // A property with both kinds gets two sub-scores in two separate segments.
    let mut both = market(8);
    for p in &mut both {
        p.lot_area_centi_m2 = Some(36_000);
    }
    let r = score_of(&both, &no_preferences(), 4);
    let c = component(&r, ComponentKind::PricePerM2);
    assert_eq!(
        (
            c.evidence["built"]["comparables"].as_u64(),
            c.evidence["lot"]["comparables"].as_u64()
        ),
        (Some(8), Some(8))
    );
}

#[test]
fn without_a_known_area_or_a_comparable_price_the_component_says_why() {
    let mut no_area = market(8);
    no_area[2].built_area_centi_m2 = None;
    let c = score_of(&no_area, &no_preferences(), 3);
    assert_eq!(
        component(&c, ComponentKind::PricePerM2).unavailable_reason,
        Some(UnavailableReason::AreaUnknown)
    );
    let mut no_price = market(8);
    no_price[2].listings[0].established_price = None;
    let c = score_of(&no_price, &no_preferences(), 3);
    assert_eq!(
        component(&c, ComponentKind::PricePerM2).unavailable_reason,
        Some(UnavailableReason::NoComparablePrice)
    );
    assert_eq!(c.details.comparable_price_centavos, None);
}

#[test]
fn property_types_are_compared_only_with_their_own_kind() {
    let mut inputs = market(5); // five houses
    for i in 6..=10 {
        inputs.push(RankInput {
            property_type: Some(PropertyType::Apartment),
            ..prop(i, 90_000_000 + i * 1_000_000)
        });
    }
    let house = score_of(&inputs, &no_preferences(), 1);
    assert_eq!(
        component(&house, ComponentKind::PricePerM2).evidence["built"]["comparables"],
        5
    );
    // A property of unknown type is compared only with other unknown-type properties.
    let mut mixed = market(6);
    mixed[0].property_type = None;
    let unknown = score_of(&mixed, &no_preferences(), 1);
    let c = component(&unknown, ComponentKind::PricePerM2);
    assert_eq!(
        c.unavailable_reason,
        Some(UnavailableReason::TooFewComparables)
    );
    assert_eq!(c.evidence["built"]["comparables"], 1);
}

#[test]
fn the_neighbourhood_is_preferred_and_the_municipality_is_the_recorded_fallback() {
    let mut inputs = market(8);
    for p in inputs.iter_mut().skip(3) {
        p.neighborhood = Some("Setor Marista".into());
    }
    // Property 1 shares its neighbourhood with only two others: too few, so the municipality is used.
    let r = score_of(&inputs, &no_preferences(), 1);
    let c = component(&r, ComponentKind::PricePerM2);
    assert!(c.score.is_some());
    assert_eq!(c.evidence["built"]["segment"], "municipality");
    assert_eq!(c.evidence["built"]["comparables"], 8);
    assert!(
        r.details
            .confidence_reasons
            .contains(&ConfidenceReason::BroadSegment)
    );
    // With enough neighbours the neighbourhood is used.
    let r = score_of(&market(8), &no_preferences(), 1);
    assert_eq!(
        component(&r, ComponentKind::PricePerM2).evidence["built"]["segment"],
        "neighborhood"
    );
    // Names are compared folded: accents and case do not split a segment.
    let mut spelled = market(8);
    spelled[0].neighborhood = Some("SETOR BUENO".into());
    spelled[1].municipality = Some("goiania".into());
    let r = score_of(&spelled, &no_preferences(), 3);
    assert_eq!(
        component(&r, ComponentKind::PricePerM2).evidence["built"]["comparables"],
        8
    );
}

#[test]
fn a_neighbourhood_name_without_a_municipality_is_not_a_segment() {
    // "Centro" exists in every city: without a municipality it must not group listings.
    let mut inputs = market(8);
    for p in &mut inputs {
        p.municipality = None;
    }
    let r = score_of(&inputs, &no_preferences(), 2);
    assert_eq!(
        component(&r, ComponentKind::PricePerM2).unavailable_reason,
        Some(UnavailableReason::TooFewComparables)
    );
}

#[test]
fn the_comparable_price_is_the_median_of_active_established_prices() {
    let mut inputs = market(8);
    inputs[0].listings = vec![
        listing("a", Some(30_000_000), 1),
        listing("b", Some(33_000_000), 1),
        listing("c", Some(150_000_000), 1), // an outlier listing cannot pick the price
        ListingInput {
            status: ListingStatus::Unavailable,
            ..listing("d", Some(1_000_000), 1)
        },
        listing("e", None, 1), // an ágio or unknown price is not an established price
    ];
    let r = score_of(&inputs, &no_preferences(), 1);
    assert_eq!(r.details.comparable_price_centavos, Some(33_000_000));
    // An even count averages the two middle prices.
    inputs[0].listings = vec![
        listing("a", Some(30_000_000), 1),
        listing("b", Some(34_000_000), 1),
    ];
    let r = score_of(&inputs, &no_preferences(), 1);
    assert_eq!(r.details.comparable_price_centavos, Some(32_000_000));
}

// ------------------------------------------------------------------------------------------
// Cross-listing difference
// ------------------------------------------------------------------------------------------

fn merged(prices: &[(&str, i64)]) -> RankInput {
    let mut p = prop(1, 30_000_000);
    p.listings = prices
        .iter()
        .map(|(adapter, price)| listing(adapter, Some(*price), 1))
        .collect();
    p.verified_merge = true;
    p
}

#[test]
fn the_cross_listing_component_needs_a_user_merge_and_two_sources() {
    let unmerged = RankInput {
        verified_merge: false,
        ..merged(&[("a", 30_000_000), ("b", 31_000_000)])
    };
    let c = score_of(&[unmerged], &no_preferences(), 1);
    assert_eq!(
        component(&c, ComponentKind::CrossListing).unavailable_reason,
        Some(UnavailableReason::NotVerified)
    );
    let one_source = merged(&[("a", 30_000_000), ("a", 31_000_000)]);
    let c = score_of(&[one_source], &no_preferences(), 1);
    assert_eq!(
        component(&c, ComponentKind::CrossListing).unavailable_reason,
        Some(UnavailableReason::SingleSource)
    );
    let lone = merged(&[("a", 30_000_000)]);
    let c = score_of(&[lone], &no_preferences(), 1);
    assert_eq!(
        component(&c, ComponentKind::CrossListing).unavailable_reason,
        Some(UnavailableReason::SingleSource)
    );
}

#[test]
fn the_cross_listing_score_is_the_spread_against_the_cap() {
    let spread = |low: i64, high: i64| {
        let p = merged(&[("a", low), ("b", high)]);
        component(
            &score_of(&[p], &no_preferences(), 1),
            ComponentKind::CrossListing,
        )
        .score
    };
    // Default cap: a 10 % spread scores 100; spread is (high - low) / high.
    assert!(close(spread(95_000_000, 100_000_000).unwrap(), 50.0));
    assert!(close(spread(90_000_000, 100_000_000).unwrap(), 100.0));
    assert!(
        close(spread(50_000_000, 100_000_000).unwrap(), 100.0),
        "capped"
    );
    assert!(
        close(spread(100_000_000, 100_000_000).unwrap(), 0.0),
        "equal prices are a real zero, not unavailable"
    );
}

#[test]
fn only_active_established_prices_of_different_sources_count_for_the_spread() {
    let mut p = merged(&[("a", 95_000_000), ("b", 100_000_000)]);
    p.listings.push(ListingInput {
        status: ListingStatus::Unavailable,
        ..listing("c", Some(10_000_000), 1)
    });
    p.listings.push(listing("d", None, 1));
    let r = score_of(&[p], &no_preferences(), 1);
    let c = component(&r, ComponentKind::CrossListing);
    assert!(close(c.score.unwrap(), 50.0));
    assert_eq!(c.evidence["prices"].as_array().unwrap().len(), 2);
}

// ------------------------------------------------------------------------------------------
// Fit
// ------------------------------------------------------------------------------------------

fn fit_of(p: RankInput, prefs: &SearchPreferences) -> Component {
    component(&score_of(&[p], prefs, 1), ComponentKind::Fit).clone()
}

#[test]
fn with_no_preference_stated_fit_is_unavailable() {
    let c = fit_of(prop(1, 30_000_000), &no_preferences());
    assert_eq!(
        (c.score, c.unavailable_reason),
        (None, Some(UnavailableReason::NoPreferences))
    );
}

#[test]
fn meeting_a_criterion_scores_a_hundred_and_more_is_not_rewarded() {
    for bedrooms in [3_u8, 4, 7] {
        let p = RankInput {
            bedrooms: Some(bedrooms),
            ..prop(1, 30_000_000)
        };
        assert!(
            close(fit_of(p, &wants_three_bedrooms()).score.unwrap(), 100.0),
            "{bedrooms}"
        );
    }
}

#[test]
fn a_shortfall_or_an_excess_over_a_maximum_lowers_the_score() {
    let two = RankInput {
        bedrooms: Some(2),
        ..prop(1, 30_000_000)
    };
    assert!(close(
        fit_of(two, &wants_three_bedrooms()).score.unwrap(),
        100.0 * (1.0 - 1.0 / 3.0)
    ));
    let none = RankInput {
        bedrooms: Some(0),
        ..prop(1, 30_000_000)
    };
    assert!(close(
        fit_of(none, &wants_three_bedrooms()).score.unwrap(),
        0.0
    ));
    let range = SearchPreferences {
        bedrooms_min: Some(2),
        bedrooms_max: Some(3),
        ..Default::default()
    };
    let five = RankInput {
        bedrooms: Some(5),
        ..prop(1, 30_000_000)
    };
    assert!(close(
        fit_of(five, &range).score.unwrap(),
        100.0 * (1.0 - 2.0 / 3.0)
    ));
    let three = RankInput {
        bedrooms: Some(3),
        ..prop(1, 30_000_000)
    };
    assert!(close(fit_of(three, &range).score.unwrap(), 100.0));
}

#[test]
fn several_criteria_are_averaged_and_an_unknown_value_is_left_out_not_zeroed() {
    let prefs = SearchPreferences {
        bedrooms_min: Some(3),
        suites_min: Some(2),
        parking_min: Some(2),
        ..Default::default()
    };
    // bedrooms 3 (meets), suites 1 of 2 (50), parking 2 (meets) -> (100 + 50 + 100) / 3
    let c = fit_of(prop(1, 30_000_000), &prefs);
    assert!(close(c.score.unwrap(), 250.0 / 3.0), "{:?}", c.score);
    // Unknown suites: averaged over the other two only.
    let unknown_suites = RankInput {
        suites: None,
        ..prop(1, 30_000_000)
    };
    let c = fit_of(unknown_suites, &prefs);
    assert!(close(c.score.unwrap(), 100.0), "{:?}", c.score);
    assert_eq!(c.evidence["criteria"].as_array().unwrap().len(), 3);
    // Everything unknown: unavailable.
    let blank = RankInput {
        bedrooms: None,
        suites: None,
        parking_spaces: None,
        ..prop(1, 30_000_000)
    };
    let c = fit_of(blank, &prefs);
    assert_eq!(
        (c.score, c.unavailable_reason),
        (None, Some(UnavailableReason::ValueUnknown))
    );
}

// ------------------------------------------------------------------------------------------
// Freshness
// ------------------------------------------------------------------------------------------

fn freshness_of(first_seen_days_ago: i64, scraped_days_ago: i64) -> f64 {
    let mut p = prop(1, 30_000_000);
    p.listings[0].first_seen_at = days_ago(first_seen_days_ago);
    p.listings[0].last_scraped_at = Some(days_ago(scraped_days_ago));
    component(
        &score_of(&[p], &no_preferences(), 1),
        ComponentKind::Freshness,
    )
    .score
    .unwrap()
}

#[test]
fn freshness_decays_linearly_over_the_configured_age() {
    assert!(close(freshness_of(0, 0), 100.0));
    assert!(close(freshness_of(30, 0), 50.0));
    assert!(close(freshness_of(60, 0), 0.0));
    assert!(close(freshness_of(400, 0), 0.0), "never negative");
}

#[test]
fn freshness_is_about_first_seen_not_last_scraped() {
    // Scraped today does not make an ad that was first seen 60 days ago fresh.
    assert!(close(freshness_of(60, 0), 0.0));
    assert!(
        close(freshness_of(0, 59), freshness_of(0, 0)),
        "an old scrape does not age a new listing"
    );
}

#[test]
fn freshness_uses_the_earliest_first_seen_among_active_listings() {
    let mut p = prop(1, 30_000_000);
    p.listings = vec![
        listing("a", Some(30_000_000), 30),
        listing("b", Some(30_000_000), 2),
        ListingInput {
            status: ListingStatus::Unavailable,
            ..listing("c", Some(30_000_000), 200)
        },
    ];
    let c = component(
        &score_of(&[p], &no_preferences(), 1),
        ComponentKind::Freshness,
    )
    .clone();
    assert!(close(c.score.unwrap(), 50.0), "{:?}", c.score);
    assert!(c.evidence["first_seen_at"].is_string());
}

// ------------------------------------------------------------------------------------------
// Data confidence
// ------------------------------------------------------------------------------------------

/// Twelve comparable houses of known type with high-confidence prices and a stated preference:
/// price, fit and freshness are available (75 % of the weight) and nothing is thin.
fn solid_case() -> Vec<RankInput> {
    market(12)
}

fn reasons_of(
    inputs: &[RankInput],
    prefs: &SearchPreferences,
) -> (Confidence, Vec<ConfidenceReason>) {
    let r = score_of(inputs, prefs, 1);
    (r.confidence, r.details.confidence_reasons)
}

#[test]
fn solid_data_is_high_confidence_with_no_reasons() {
    let (confidence, reasons) = reasons_of(&solid_case(), &wants_three_bedrooms());
    assert_eq!((confidence, reasons), (Confidence::High, vec![]));
}

/// A named way the data can be thin: how to degrade the solid case, the reason that must be
/// recorded, and the highest confidence that is then allowed.
type Weakness = (
    &'static str,
    fn(&mut Vec<RankInput>),
    ConfidenceReason,
    Confidence,
);

#[test]
fn each_weakness_lowers_the_confidence_and_is_recorded() {
    let cases: [Weakness; 10] = [
        (
            "unknown property type",
            |v| {
                for p in v.iter_mut() {
                    p.property_type = None;
                }
            },
            ConfidenceReason::PropertyTypeUnknown,
            Confidence::Medium,
        ),
        (
            "conflicting values",
            |v| v[0].conflicting_fields = vec!["bedrooms".into()],
            ConfidenceReason::ConflictingValues,
            Confidence::Medium,
        ),
        (
            "few comparables",
            |v| *v = market(7),
            ConfidenceReason::FewComparables,
            Confidence::Medium,
        ),
        (
            "municipality segment",
            |v| v[0].neighborhood = Some("Outro Setor".into()),
            ConfidenceReason::BroadSegment,
            Confidence::Medium,
        ),
        (
            "medium-confidence price",
            |v| v[0].listings[0].established_price = Some((30_000_000, Confidence::Medium)),
            ConfidenceReason::PriceConfidenceMedium,
            Confidence::Medium,
        ),
        (
            "stale scrape",
            |v| v[0].listings[0].last_scraped_at = Some(days_ago(20)),
            ConfidenceReason::StaleData,
            Confidence::Medium,
        ),
        (
            "never scraped",
            |v| v[0].listings[0].last_scraped_at = None,
            ConfidenceReason::StaleData,
            Confidence::Medium,
        ),
        (
            "fit value unknown",
            |v| v[0].bedrooms = None,
            ConfidenceReason::FitValuesUnknown,
            Confidence::Medium,
        ),
        (
            "no price",
            |v| v[0].listings[0].established_price = None,
            ConfidenceReason::NoPrice,
            Confidence::Low,
        ),
        (
            "no area",
            |v| v[0].built_area_centi_m2 = None,
            ConfidenceReason::NoArea,
            Confidence::Low,
        ),
    ];
    let prefs = wants_three_bedrooms();
    for (name, degrade, reason, level) in cases {
        let mut inputs = solid_case();
        degrade(&mut inputs);
        let (confidence, reasons) = reasons_of(&inputs, &prefs);
        assert!(reasons.contains(&reason), "{name}: {reasons:?}");
        assert_eq!(confidence, level, "{name}: {reasons:?}");
    }
}

#[test]
fn thin_component_coverage_lowers_the_confidence() {
    // Price and freshness only (60 % of the weight): at most medium.
    let (confidence, reasons) = reasons_of(&solid_case(), &no_preferences());
    assert!(
        reasons.contains(&ConfidenceReason::PartialComponentCoverage),
        "{reasons:?}"
    );
    assert_eq!(confidence, Confidence::Medium);
    // Freshness only (15 %): low.
    let mut p = prop(1, 30_000_000);
    p.built_area_centi_m2 = None;
    let r = score_of(&[p], &no_preferences(), 1);
    assert!(
        r.details
            .confidence_reasons
            .contains(&ConfidenceReason::LowComponentCoverage)
    );
    assert_eq!(r.confidence, Confidence::Low);
}

#[test]
fn the_lowest_cap_wins_when_several_reasons_apply() {
    let mut v = solid_case();
    for p in &mut v {
        p.property_type = None; // medium
    }
    v[0].built_area_centi_m2 = None; // low
    let r = score_of(&v, &wants_three_bedrooms(), 1);
    assert_eq!(r.confidence, Confidence::Low);
    assert!(r.details.confidence_reasons.len() >= 2);
}

#[test]
fn the_reasons_have_stable_codes_for_the_interface() {
    let codes: Vec<String> = [
        ConfidenceReason::NoPrice,
        ConfidenceReason::PartialComponentCoverage,
        ConfidenceReason::BroadSegment,
    ]
    .iter()
    .map(|r| serde_json::to_string(r).unwrap())
    .collect();
    assert_eq!(
        codes,
        vec![
            r#""no_price""#,
            r#""partial_component_coverage""#,
            r#""broad_segment""#
        ]
    );
    assert_eq!(ConfidenceReason::StaleData.cap(), Confidence::Medium);
    assert_eq!(ConfidenceReason::NoArea.cap(), Confidence::Low);
}

#[test]
fn the_details_round_trip_as_a_stored_document() {
    let r = score_of(&solid_case(), &wants_three_bedrooms(), 2);
    let json = serde_json::to_string(&r.details).unwrap();
    let back: ScoreDetails = serde_json::from_str(&json).unwrap();
    assert_eq!(back, r.details);
    assert!(json.contains("\"ranking_version\"") && json.contains("\"price_per_m2\""));
}
