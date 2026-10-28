use super::*;

fn invalid(config: &RankingConfig) -> bool {
    matches!(config.validate(), Err(Error::InvalidConfig { .. }))
}

#[test]
fn the_defaults_are_the_specifications_initial_weights_and_are_valid() {
    let c = RankingConfig::default();
    c.validate().unwrap();
    assert!((c.weight_price_per_m2 - 0.45).abs() < f64::EPSILON);
    assert!((c.weight_cross_listing - 0.25).abs() < f64::EPSILON);
    assert!((c.weight_fit - 0.15).abs() < f64::EPSILON);
    assert!((c.weight_freshness - 0.15).abs() < f64::EPSILON);
    assert!((c.total_weight() - 1.0).abs() < 1e-12);
}

#[test]
fn weights_must_be_finite_non_negative_and_not_all_zero() {
    let base = RankingConfig::default();
    for bad in [-0.1, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        let mut c = base.clone();
        c.weight_fit = bad;
        assert!(invalid(&c), "{bad}");
    }
    let zero = RankingConfig {
        weight_price_per_m2: 0.0,
        weight_cross_listing: 0.0,
        weight_fit: 0.0,
        weight_freshness: 0.0,
        ..base.clone()
    };
    assert!(invalid(&zero));
    // A single non-zero weight is a legitimate configuration.
    let only_freshness = RankingConfig {
        weight_price_per_m2: 0.0,
        weight_cross_listing: 0.0,
        weight_fit: 0.0,
        weight_freshness: 1.0,
        ..base
    };
    only_freshness.validate().unwrap();
}

#[test]
fn thresholds_must_make_sense() {
    type Mutation = fn(&mut RankingConfig);
    let base = RankingConfig::default();
    let cases: Vec<(&str, Mutation)> = vec![
        ("min comparables below 2", |c| c.min_comparables = 1),
        ("solid below min", |c| {
            c.solid_comparables = c.min_comparables - 1;
        }),
        ("cap zero", |c| c.cross_listing_cap_percent = 0.0),
        ("cap above 100", |c| c.cross_listing_cap_percent = 100.5),
        ("cap NaN", |c| c.cross_listing_cap_percent = f64::NAN),
        ("freshness zero", |c| c.freshness_max_age_days = 0),
        ("stale zero", |c| c.stale_scrape_days = 0),
    ];
    for (name, change) in cases {
        let mut c = base.clone();
        change(&mut c);
        assert!(invalid(&c), "{name}");
    }
}

#[test]
fn preferences_must_not_contradict_themselves() {
    SearchPreferences::default().validate().unwrap();
    SearchPreferences {
        bedrooms_min: Some(2),
        bedrooms_max: Some(2),
        ..Default::default()
    }
    .validate()
    .unwrap();
    let bad = SearchPreferences {
        bedrooms_min: Some(4),
        bedrooms_max: Some(2),
        ..Default::default()
    };
    assert!(matches!(bad.validate(), Err(Error::InvalidConfig { .. })));
    assert!(
        SearchPreferences {
            bedrooms_min: Some(101),
            ..Default::default()
        }
        .validate()
        .is_err(),
        "above the schema's limit"
    );
    assert!(
        SearchPreferences {
            suites_min: Some(101),
            ..Default::default()
        }
        .validate()
        .is_err()
    );
    assert!(
        SearchPreferences {
            parking_min: Some(101),
            ..Default::default()
        }
        .validate()
        .is_err()
    );
}

#[test]
fn an_empty_preference_set_says_so() {
    assert!(SearchPreferences::default().is_empty());
    assert!(
        !SearchPreferences {
            parking_min: Some(1),
            ..Default::default()
        }
        .is_empty()
    );
}

#[test]
fn documents_are_read_in_the_stored_shape_and_unknown_fields_are_refused() {
    let prefs: SearchPreferences =
        serde_json::from_str(r#"{ "bedroomsMin": 3, "suitesMin": 1 }"#).unwrap();
    assert_eq!(
        (prefs.bedrooms_min, prefs.suites_min, prefs.bedrooms_max),
        (Some(3), Some(1), None)
    );
    assert!(serde_json::from_str::<SearchPreferences>(r#"{ "bedrooms": 3 }"#).is_err());
    let config: RankingConfig = serde_json::from_str(r#"{ "weightFit": 0.5 }"#).unwrap();
    assert!((config.weight_fit - 0.5).abs() < f64::EPSILON);
    assert_eq!(
        config.min_comparables, 5,
        "omitted fields keep their defaults"
    );
    assert!(serde_json::from_str::<RankingConfig>(r#"{ "weightFits": 0.5 }"#).is_err());
}

#[test]
fn the_configuration_is_written_with_the_same_camel_case_keys_it_is_read_with() {
    let json = serde_json::to_value(RankingConfig::default()).unwrap();
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
            "crossListingCapPercent",
            "freshnessMaxAgeDays",
            "minComparables",
            "solidComparables",
            "staleScrapeDays",
            "weightCrossListing",
            "weightFit",
            "weightFreshness",
            "weightPricePerM2",
        ]
    );
    let back: RankingConfig = serde_json::from_value(json).unwrap();
    assert_eq!(back, RankingConfig::default());
}
