use super::*;
use crate::models::matching::{ConflictKind as C, Severity, SignalKind as S};

const H1: u64 = 0x0123_4567_89ab_cdef;
const H2: u64 = 0xf0e1_d2c3_b4a5_9687;
const H3: u64 = 0x5a5a_a5a5_3c3c_c3c3;

type Change = fn(&mut MatchConfig);

fn config() -> MatchConfig {
    MatchConfig::default()
}

fn nothing_common() -> HashSet<u64> {
    HashSet::new()
}

/// A house in Setor Bueno, Goiânia.
fn house(id: i64) -> MatchInput {
    let mut m = MatchInput::new(id, "alpha");
    m.municipality = Some("goiania".into());
    m.neighborhood = Some("setor bueno".into());
    m.property_type = Some(PropertyType::House);
    m
}

fn with_images(mut m: MatchInput, hashes: &[u64]) -> MatchInput {
    m.images = hashes
        .iter()
        .enumerate()
        .map(|(i, &hash)| ImageHash {
            image_id: m.listing_id * 100 + i64::try_from(i).unwrap(),
            hash,
        })
        .collect();
    m
}

fn with_words(mut m: MatchInput, words: &str) -> MatchInput {
    m.tokens = words.split_whitespace().map(str::to_owned).collect();
    m
}

fn full(a: &MatchInput, b: &MatchInput) -> Comparison {
    compare(a, b, &config(), &nothing_common())
}

fn category(a: &MatchInput, b: &MatchInput) -> MatchCategory {
    full(a, b).category
}

const GARDEN_AD: &str =
    "jardim ampla varanda churrasqueira piscina garagem coberta quintal arborizado";

/// Two ads with the same two photos (one slightly re-encoded), same place, area and rooms.
fn matching_pair() -> (MatchInput, MatchInput) {
    let mut a = with_images(house(1), &[H1, H2]);
    let mut b = with_images(house(2), &[H1 ^ 0b1, H2 ^ 0b111]);
    for m in [&mut a, &mut b] {
        m.built_area_centi_m2 = Some(12_000);
        m.bedrooms = Some(3);
        m.suites = Some(1);
    }
    (a, b)
}

#[test]
fn the_default_configuration_is_valid_and_contradictory_ones_are_rejected() {
    config().validate().unwrap();
    let cases: Vec<(&str, Change)> = vec![
        ("image distance", |c| c.image_distance = 17),
        ("no strong pairs", |c| c.strong_image_pairs = 0),
        ("common hash of one", |c| c.common_hash_listings = 1),
        ("area bands inverted", |c| {
            c.area_match_permille = c.area_conflict_permille;
        }),
        ("price bands inverted", |c| {
            c.price_close_permille = c.price_far_permille;
        }),
        ("jaccard zero", |c| c.text_min_jaccard_percent = 0),
        ("jaccard above 100", |c| c.text_min_jaccard_percent = 101),
        ("no tokens", |c| c.text_min_tokens = 0),
        ("tiny blocks", |c| c.max_block_size = 1),
    ];
    for (name, change) in cases {
        let mut c = config();
        change(&mut c);
        let error = c.validate().expect_err(name);
        assert!(
            matches!(error, Error::InvalidConfig { .. }),
            "{name}: {error}"
        );
    }
}

#[test]
fn strong_images_with_agreeing_place_area_and_rooms_and_no_conflict_are_high() {
    let (a, b) = matching_pair();
    let result = full(&a, &b);
    assert_eq!(result.category, MatchCategory::High);
    let e = &result.evidence;
    assert_eq!(e.rule_version, RULE_VERSION);
    for kind in [
        S::NearDuplicateImages,
        S::SameMunicipality,
        S::SameNeighborhood,
        S::BuiltAreaMatch,
        S::RoomsMatch,
    ] {
        assert!(e.has_signal(kind), "{kind} missing from {:?}", e.signals);
    }
    assert_eq!(e.conflicts.len(), 0, "{:?}", e.conflicts);
    assert_eq!(e.image_pairs.len(), 2);
    assert_eq!(result.min_image_distance, Some(1));
}

#[test]
fn photos_alone_are_possible_never_high() {
    let a = with_images(MatchInput::new(1, "alpha"), &[H1, H2]);
    let b = with_images(MatchInput::new(2, "beta"), &[H1, H2]);
    assert_eq!(category(&a, &b), MatchCategory::Possible);
}

#[test]
fn a_single_matching_photo_is_possible_but_not_high_even_with_everything_else_agreeing() {
    let (mut a, mut b) = matching_pair();
    a.images.truncate(1);
    b.images.truncate(1);
    assert_eq!(category(&a, &b), MatchCategory::Possible);
}

#[test]
fn strong_photos_without_a_known_neighbourhood_are_not_high() {
    let (mut a, mut b) = matching_pair();
    a.neighborhood = None;
    b.neighborhood = None;
    assert_eq!(category(&a, &b), MatchCategory::Possible);
}

#[test]
fn strong_photos_and_place_without_area_or_rooms_are_not_high() {
    let a = with_images(house(1), &[H1, H2]);
    let b = with_images(house(2), &[H1, H2]);
    assert_eq!(category(&a, &b), MatchCategory::Possible);
}

#[test]
fn metadata_alone_never_reaches_high() {
    let mut a = with_words(house(1), GARDEN_AD);
    let mut b = with_words(house(2), GARDEN_AD);
    for m in [&mut a, &mut b] {
        m.lot_area_centi_m2 = Some(36_000);
        m.bedrooms = Some(3);
        m.suites = Some(1);
        m.full_price_centavos = Some(45_000_000);
    }
    assert_eq!(
        category(&a, &b),
        MatchCategory::Possible,
        "everything agrees, yet no photos"
    );
}

#[test]
fn place_type_area_rooms_and_price_without_matching_text_are_not_even_possible() {
    // Standardised plots and developer houses share all of these.
    let mut a = house(1);
    let mut b = house(2);
    for m in [&mut a, &mut b] {
        m.lot_area_centi_m2 = Some(36_000);
        m.bedrooms = Some(3);
        m.suites = Some(1);
        m.full_price_centavos = Some(45_000_000);
    }
    assert_eq!(category(&a, &b), MatchCategory::Low);
}

#[test]
fn metadata_only_matches_are_never_possible_for_apartments_or_unknown_types() {
    for property_type in [Some(PropertyType::Apartment), None] {
        let mut a = with_words(house(1), GARDEN_AD);
        let mut b = with_words(house(2), GARDEN_AD);
        for m in [&mut a, &mut b] {
            m.property_type = property_type;
            m.built_area_centi_m2 = Some(8_000);
            m.bedrooms = Some(2);
            m.suites = Some(1);
        }
        assert_eq!(category(&a, &b), MatchCategory::Low, "{property_type:?}");
    }
}

#[test]
fn identical_photos_in_different_municipalities_are_low() {
    let mut a = with_images(house(1), &[H1, H2]);
    let b = with_images(house(2), &[H1, H2]);
    a.municipality = Some("aparecida de goiania".into());
    let result = full(&a, &b);
    assert_eq!(result.category, MatchCategory::Low);
    assert!(result.evidence.has_conflict(C::Municipality));
    assert!(result.evidence.has_hard_conflict());
}

#[test]
fn a_hard_conflict_with_one_matching_photo_is_low() {
    let (mut a, mut b) = matching_pair();
    a.images.truncate(1);
    b.images.truncate(1);
    b.built_area_centi_m2 = Some(24_000);
    assert_eq!(category(&a, &b), MatchCategory::Low);
}

#[test]
fn same_building_photos_with_different_units_stay_for_review_with_the_conflict_recorded() {
    let (a, mut b) = matching_pair();
    b.built_area_centi_m2 = Some(24_000);
    b.bedrooms = Some(2);
    let result = full(&a, &b);
    assert_eq!(result.category, MatchCategory::Possible, "never High");
    assert!(result.evidence.has_conflict(C::BuiltArea));
    assert!(result.evidence.has_conflict(C::Bedrooms));
    assert!(
        result
            .evidence
            .conflicts
            .iter()
            .all(|c| c.severity == Severity::Hard)
    );
}

#[test]
fn a_soft_conflict_keeps_a_photo_match_out_of_high() {
    let (mut a, mut b) = matching_pair();
    a.full_price_centavos = Some(30_000_000);
    b.full_price_centavos = Some(50_000_000);
    let result = full(&a, &b);
    assert_eq!(result.category, MatchCategory::Possible);
    assert!(result.evidence.has_conflict(C::Price));
    assert!(!result.evidence.has_hard_conflict());
}

#[test]
fn a_soft_conflict_without_photos_is_low() {
    let mut a = house(1);
    let mut b = house(2);
    a.full_price_centavos = Some(30_000_000);
    b.full_price_centavos = Some(50_000_000);
    assert_eq!(category(&a, &b), MatchCategory::Low);
}

#[test]
fn differing_neighbourhoods_are_a_soft_conflict_and_block_high() {
    let (a, mut b) = matching_pair();
    b.neighborhood = Some("setor marista".into());
    let result = full(&a, &b);
    assert_eq!(result.category, MatchCategory::Possible);
    let conflict = result
        .evidence
        .conflicts
        .iter()
        .find(|c| c.kind == C::Neighborhood)
        .unwrap();
    assert_eq!(conflict.severity, Severity::Soft);
    assert!(!result.evidence.has_signal(S::SameNeighborhood));
}

#[test]
fn matching_neighbourhood_names_mean_nothing_without_a_known_municipality() {
    // "Centro" exists in every city.
    let mut a = house(1);
    let mut b = house(2);
    a.neighborhood = Some("centro".into());
    b.neighborhood = Some("centro".into());
    a.municipality = None;
    let result = full(&a, &b);
    assert!(!result.evidence.has_signal(S::SameNeighborhood));
    assert!(
        result
            .evidence
            .notes
            .iter()
            .any(|n| n.contains("municipality is unknown")),
        "{:?}",
        result.evidence.notes
    );
}

#[test]
fn photo_pairs_are_one_to_one() {
    // Two photos of A that both resemble the single photo of B are one pair, not two.
    let a = with_images(MatchInput::new(1, "alpha"), &[H1, H1 ^ 0b10]);
    let b = with_images(MatchInput::new(2, "alpha"), &[H1 ^ 0b1]);
    let result = full(&a, &b);
    assert_eq!(result.evidence.image_pairs.len(), 1);
    assert_eq!(result.category, MatchCategory::Possible);
    let strong_a = with_images(MatchInput::new(1, "alpha"), &[H1, H2]);
    let strong_b = with_images(MatchInput::new(2, "alpha"), &[H1, H2]);
    assert_eq!(full(&strong_a, &strong_b).evidence.image_pairs.len(), 2);
}

#[test]
fn photos_beyond_the_distance_threshold_do_not_pair() {
    let a = with_images(MatchInput::new(1, "alpha"), &[H1]);
    let at_limit = with_images(MatchInput::new(2, "alpha"), &[H1 ^ 0b11111]); // 5 bits
    let beyond = with_images(MatchInput::new(3, "alpha"), &[H1 ^ 0b11_1111]); // 6 bits
    assert_eq!(full(&a, &at_limit).evidence.image_pairs.len(), 1);
    let beyond_pairs = full(&a, &beyond).evidence.image_pairs;
    assert_eq!(beyond_pairs.len(), 0, "{beyond_pairs:?}");
    assert_eq!(category(&a, &beyond), MatchCategory::Low);
}

#[test]
fn hashes_shared_by_many_listings_are_ignored_and_say_so() {
    let a = with_images(MatchInput::new(1, "alpha"), &[H3, H1]);
    let b = with_images(MatchInput::new(2, "alpha"), &[H3, H2]);
    let common: HashSet<u64> = [H3].into_iter().collect();
    let result = compare(&a, &b, &config(), &common);
    assert!(
        result.evidence.image_pairs.is_empty(),
        "the placeholder is not evidence"
    );
    assert_eq!(result.category, MatchCategory::Low);
    assert!(
        result
            .evidence
            .notes
            .iter()
            .any(|n| n.contains("shared by many listings")),
        "{:?}",
        result.evidence.notes
    );
    assert_eq!(
        compare(&a, &b, &config(), &nothing_common())
            .evidence
            .image_pairs
            .len(),
        1
    );
}

#[test]
fn areas_are_only_compared_within_their_own_kind() {
    let mut a = house(1);
    let mut b = house(2);
    a.lot_area_centi_m2 = Some(36_000);
    b.built_area_centi_m2 = Some(36_000); // the same number, but a different thing
    let result = full(&a, &b);
    assert!(!result.evidence.has_signal(S::LotAreaMatch));
    assert!(!result.evidence.has_signal(S::BuiltAreaMatch));
    assert!(!result.evidence.has_signal(S::UsableAreaMatch));
    assert_eq!(
        result.evidence.conflicts.len(),
        0,
        "{:?}",
        result.evidence.conflicts
    );
    assert!(
        result
            .evidence
            .notes
            .iter()
            .any(|n| n.contains("different kinds")),
        "{:?}",
        result.evidence.notes
    );
}

#[test]
fn area_bands_match_within_two_percent_conflict_beyond_ten_and_are_neutral_between() {
    let area = |centi: u32| {
        let mut a = house(1);
        let mut b = house(2);
        a.lot_area_centi_m2 = Some(10_000);
        b.lot_area_centi_m2 = Some(centi);
        full(&a, &b).evidence
    };
    assert!(area(10_000).has_signal(S::LotAreaMatch));
    assert!(area(10_200).has_signal(S::LotAreaMatch), "exactly 2%");
    let between = area(10_500);
    assert!(!between.has_signal(S::LotAreaMatch) && !between.has_conflict(C::LotArea));
    assert!(!area(11_000).has_conflict(C::LotArea), "exactly 10%");
    assert!(area(11_200).has_conflict(C::LotArea));
}

#[test]
fn an_area_stated_by_one_side_only_is_unknown_not_a_difference() {
    let mut a = house(1);
    a.built_area_centi_m2 = Some(9_000);
    let result = full(&a, &house(2));
    assert_eq!(
        result.evidence.conflicts.len(),
        0,
        "{:?}",
        result.evidence.conflicts
    );
    assert!(
        result
            .evidence
            .unknown
            .iter()
            .any(|u| u.contains("built area")),
        "{:?}",
        result.evidence.unknown
    );
}

#[test]
fn rooms_match_needs_two_known_equal_fields_and_any_difference_is_a_conflict() {
    let rooms = |a_values: (Option<u8>, Option<u8>, Option<u8>),
                 b_values: (Option<u8>, Option<u8>, Option<u8>)| {
        let mut a = house(1);
        let mut b = house(2);
        (a.bedrooms, a.suites, a.parking_spaces) = a_values;
        (b.bedrooms, b.suites, b.parking_spaces) = b_values;
        full(&a, &b).evidence
    };
    assert!(rooms((Some(3), Some(1), None), (Some(3), Some(1), None)).has_signal(S::RoomsMatch));
    assert!(rooms((Some(3), Some(1), Some(2)), (Some(3), None, Some(2))).has_signal(S::RoomsMatch));
    assert!(
        !rooms((Some(3), None, None), (Some(3), None, None)).has_signal(S::RoomsMatch),
        "one field is not enough"
    );
    let differing = rooms((Some(3), Some(1), None), (Some(2), Some(1), None));
    assert!(differing.has_conflict(C::Bedrooms) && !differing.has_signal(S::RoomsMatch));
    assert!(rooms((Some(3), Some(1), None), (Some(3), Some(2), None)).has_conflict(C::Suites));
    assert!(rooms((None, None, Some(1)), (None, None, Some(2))).has_conflict(C::ParkingSpaces));
}

#[test]
fn property_types_house_labels_are_compatible_but_other_kinds_conflict() {
    let types = |x: PropertyType, y: PropertyType| {
        let mut a = house(1);
        let mut b = house(2);
        a.property_type = Some(x);
        b.property_type = Some(y);
        full(&a, &b).evidence
    };
    assert!(types(PropertyType::House, PropertyType::House).has_signal(S::SamePropertyType));
    let labels = types(PropertyType::House, PropertyType::Sobrado);
    assert!(!labels.has_signal(S::SamePropertyType) && !labels.has_conflict(C::PropertyType));
    assert!(
        labels.notes.iter().any(|n| n.contains("compatible")),
        "{:?}",
        labels.notes
    );
    assert!(types(PropertyType::House, PropertyType::Apartment).has_conflict(C::PropertyType));
    assert!(types(PropertyType::Apartment, PropertyType::Lot).has_hard_conflict());
    let vague = types(PropertyType::Other, PropertyType::Apartment);
    assert!(!vague.has_conflict(C::PropertyType) && !vague.has_signal(S::SamePropertyType));
}

#[test]
fn price_bands_are_close_within_five_percent_far_beyond_thirty_and_neutral_between() {
    let price = |centavos: i64| {
        let mut a = house(1);
        let mut b = house(2);
        a.full_price_centavos = Some(10_000_000);
        b.full_price_centavos = Some(centavos);
        full(&a, &b).evidence
    };
    assert!(price(10_000_000).has_signal(S::PriceClose));
    assert!(price(10_500_000).has_signal(S::PriceClose), "exactly 5%");
    let between = price(12_000_000);
    assert!(!between.has_signal(S::PriceClose) && !between.has_conflict(C::Price));
    assert!(!price(13_000_000).has_conflict(C::Price), "exactly 30%");
    assert!(price(13_100_000).has_conflict(C::Price));
}

#[test]
fn text_needs_enough_distinct_terms_and_enough_overlap() {
    let text = |a_words: &str, b_words: &str| {
        let a = with_words(house(1), a_words);
        let b = with_words(house(2), b_words);
        full(&a, &b).evidence
    };
    assert!(text(GARDEN_AD, GARDEN_AD).has_signal(S::TextSimilar));
    assert!(
        !text("jardim ampla varanda", "jardim ampla varanda").has_signal(S::TextSimilar),
        "too short to mean anything"
    );
    let other = "lote plano escriturado financiamento aceita permuta documentacao regular";
    assert!(!text(GARDEN_AD, other).has_signal(S::TextSimilar));
    // 6 shared of 10 distinct terms is exactly the 60% boundary.
    let a = "um dois tres quatro cinco seis sete oito";
    let b = "um dois tres quatro cinco seis nove dez";
    assert!(text(a, b).has_signal(S::TextSimilar), "6/10 = 60%");
    let c = "um dois tres quatro cinco onze doze treze";
    assert!(!text(a, c).has_signal(S::TextSimilar), "5/11 is below 60%");
}

#[test]
fn what_is_unknown_is_recorded_not_guessed() {
    let result = full(&MatchInput::new(1, "alpha"), &MatchInput::new(2, "alpha"));
    assert_eq!(result.category, MatchCategory::Low);
    assert!(result.evidence.signals.is_empty() && result.evidence.conflicts.is_empty());
    for field in ["municipality", "neighbourhood", "property type", "bedrooms"] {
        assert!(
            result.evidence.unknown.iter().any(|u| u.contains(field)),
            "{field}: {:?}",
            result.evidence.unknown
        );
    }
}

#[test]
fn the_category_does_not_depend_on_which_listing_is_a() {
    let (a, b) = matching_pair();
    let mut different_unit = b.clone();
    different_unit.built_area_centi_m2 = Some(24_000);
    let mut elsewhere = b.clone();
    elsewhere.municipality = Some("aparecida de goiania".into());
    let mut repriced = b.clone();
    repriced.full_price_centavos = Some(90_000_000);
    let mut lone = a.clone();
    lone.full_price_centavos = Some(30_000_000);
    for (x, y) in [
        (&a, &b),
        (&a, &different_unit),
        (&a, &elsewhere),
        (&lone, &repriced),
        (&a, &MatchInput::new(9, "beta")),
    ] {
        assert_eq!(
            category(x, y),
            category(y, x),
            "{} vs {}",
            x.listing_id,
            y.listing_id
        );
    }
}

#[test]
fn comparing_is_deterministic() {
    let (a, b) = matching_pair();
    assert_eq!(full(&a, &b), full(&a, &b));
}

#[test]
fn image_pairs_report_the_ids_of_a_and_b_in_the_order_given() {
    let (a, b) = matching_pair();
    let result = full(&a, &b);
    assert!(
        result
            .evidence
            .image_pairs
            .iter()
            .all(|p| (100..200).contains(&p.image_a) && (200..300).contains(&p.image_b))
    );
    let reversed = full(&b, &a);
    assert!(
        reversed
            .evidence
            .image_pairs
            .iter()
            .all(|p| (200..300).contains(&p.image_a) && (100..200).contains(&p.image_b))
    );
}

#[test]
fn evidence_never_contains_the_words_of_the_text() {
    let a = with_words(
        house(1),
        "telefone 62999990000 maria jardim ampla varanda churrasqueira piscina",
    );
    let b = with_words(
        house(2),
        "telefone 62999990000 maria jardim ampla varanda churrasqueira piscina",
    );
    let json = serde_json::to_string(&full(&a, &b).evidence).unwrap();
    assert!(
        !json.contains("maria") && !json.contains("62999990000") && !json.contains("churrasqueira"),
        "{json}"
    );
}

#[test]
fn a_listing_that_contradicts_itself_is_reported_as_such_not_as_merely_unknown() {
    let mut a = house(1);
    a.contradictory = vec!["bedrooms".to_owned(), "lot area".to_owned()];
    let result = full(&a, &house(2));
    for label in ["bedrooms", "lot area"] {
        let expected = format!("listing A states conflicting values for {label}");
        assert!(
            result.evidence.notes.contains(&expected),
            "{expected}: {:?}",
            result.evidence.notes
        );
    }
    let mut b = house(2);
    b.contradictory = vec!["suites".to_owned()];
    let both = full(&a, &b);
    assert!(
        both.evidence
            .notes
            .iter()
            .any(|n| n == "listing B states conflicting values for suites")
    );
}

#[test]
fn the_stored_summary_describes_the_comparison_inputs_without_words_or_ids() {
    let mut m = with_words(
        house(1),
        "telefone maria jardim ampla varanda churrasqueira piscina garagem",
    );
    m.bedrooms = Some(3);
    m.built_area_centi_m2 = Some(12_000);
    m.full_price_centavos = Some(35_000_000);
    m.images = with_images(house(1), &[H1, H2]).images;
    let summary = m.summary();
    assert_eq!(summary["municipality"], "goiania");
    assert_eq!(summary["neighborhood"], "setor bueno");
    assert_eq!(summary["property_type"], "house");
    assert_eq!(summary["bedrooms"], 3);
    assert_eq!(summary["built_area_centi_m2"], 12_000);
    assert_eq!(summary["full_price_centavos"], 35_000_000);
    assert_eq!(summary["images"], 2);
    assert_eq!(summary["description_terms"], 8);
    assert!(summary["suites"].is_null());
    let json = summary.to_string();
    assert!(
        !json.contains("maria") && !json.contains("churrasqueira"),
        "{json}"
    );
}

#[test]
fn identical_units_of_one_building_are_never_high() {
    // Same marketing photos, same address, same floorplan: nothing tells two flats apart.
    let (mut a, mut b) = matching_pair();
    a.property_type = Some(PropertyType::Apartment);
    b.property_type = Some(PropertyType::Apartment);
    let result = full(&a, &b);
    assert_eq!(result.category, MatchCategory::Possible);
    assert!(
        result.evidence.has_signal(S::SamePropertyType) && result.evidence.conflicts.is_empty()
    );
    // One side being an apartment is enough to be careful; the other side's type may be unknown.
    b.property_type = None;
    assert_eq!(category(&a, &b), MatchCategory::Possible);
    // The same evidence for houses is High.
    a.property_type = Some(PropertyType::House);
    b.property_type = Some(PropertyType::House);
    assert_eq!(category(&a, &b), MatchCategory::High);
}
