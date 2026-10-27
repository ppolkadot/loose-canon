use super::*;
use crate::{matching::compare::ImageHash, models::characteristics::PropertyType};

const TEXT_A: &str =
    "jardim ampla varanda churrasqueira piscina garagem coberta quintal arborizado";
const TEXT_B: &str = "lote plano escriturado financiamento aceita permuta documentacao regular";

/// Well-spread 64-bit values (splitmix64), so unrelated photos are never accidentally close.
fn spread(n: usize) -> Vec<u64> {
    let mut state = 0x1234_5678_9ABC_DEF0_u64;
    let values: Vec<u64> = (0..n)
        .map(|_| {
            state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut z = state;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            z ^ (z >> 31)
        })
        .collect();
    for (i, a) in values.iter().enumerate() {
        for b in &values[i + 1..] {
            assert!((a ^ b).count_ones() > 12, "test hashes must be far apart");
        }
    }
    values
}

/// A house in the given neighbourhood of Goiânia: 120 m² built, 3 bedrooms, 1 suite.
fn house(id: i64, neighborhood: &str) -> MatchInput {
    let mut m = MatchInput::new(id, "alpha");
    m.municipality = Some("goiania".into());
    m.neighborhood = Some(neighborhood.into());
    m.property_type = Some(PropertyType::House);
    m.built_area_centi_m2 = Some(12_000);
    m.bedrooms = Some(3);
    m.suites = Some(1);
    m
}

fn photos(mut m: MatchInput, hashes: &[u64]) -> MatchInput {
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

fn words(mut m: MatchInput, text: &str) -> MatchInput {
    m.tokens = text.split_whitespace().map(str::to_owned).collect();
    m
}

fn flat(mut m: MatchInput, built_centi_m2: u32) -> MatchInput {
    m.property_type = Some(PropertyType::Apartment);
    m.built_area_centi_m2 = Some(built_centi_m2);
    m.bedrooms = Some(2);
    m
}

fn pair(id: &str, scenario: Scenario, same_property: bool, a: i64, b: i64) -> LabeledPair {
    LabeledPair {
        id: id.to_owned(),
        scenario,
        same_property,
        listing_a: a,
        listing_b: b,
    }
}

const BUENO: &str = "setor bueno";

type Change = fn(&mut LabeledSet);

/// A hand-made set that includes the rule's known weak spots on purpose (see each pair's note).
#[expect(
    clippy::too_many_lines,
    reason = "one long table of labelled cases; splitting it would only scatter it"
)]
fn synthetic_set() -> LabeledSet {
    use Scenario as S;
    let h = spread(24);
    let near = |hash: u64, bits: u32| hash ^ ((1_u64 << bits) - 1);
    let mut listings = Vec::new();
    let mut pairs = Vec::new();
    let mut extra = Vec::new(); // listings that only populate the library
    let mut add = |id: &str, scenario: Scenario, same: bool, a: MatchInput, b: MatchInput| {
        pairs.push(pair(id, scenario, same, a.listing_id, b.listing_id));
        listings.push(a);
        listings.push(b);
    };

    // --- the same property ---------------------------------------------------------------
    // Two platforms, the same photos (one re-encoded): the easy case.
    add(
        "same-cross-source",
        S::SameCrossSource,
        true,
        photos(house(11, BUENO), &[h[0], h[1]]),
        photos(house(12, BUENO), &[near(h[0], 1), near(h[1], 3)]),
    );
    // Only one photo in common: the rest are other angles.
    add(
        "same-other-angle",
        S::SameDifferentAngle,
        true,
        photos(house(21, BUENO), &[h[2], h[3]]),
        photos(house(22, BUENO), &[near(h[2], 1), h[4]]),
    );
    // Re-encoded and slightly cropped photos, at the edge of the threshold.
    add(
        "same-transformed",
        S::ImageTransformation,
        true,
        photos(house(31, BUENO), &[h[5], h[6]]),
        photos(house(32, BUENO), &[near(h[5], 4), near(h[6], 5)]),
    );
    // Heavily cropped photos: too far apart to count, and the texts differ. Missed.
    add(
        "same-heavily-cropped",
        S::ImageTransformation,
        true,
        words(photos(house(41, BUENO), &[h[7], h[8]]), TEXT_A),
        words(
            photos(house(42, BUENO), &[near(h[7], 9), near(h[8], 10)]),
            TEXT_B,
        ),
    );
    // No photos, and the two platforms label the neighbourhood differently. Never proposed.
    add(
        "same-no-photos-other-label",
        S::SameCrossSource,
        true,
        house(51, BUENO),
        house(52, "setor marista"),
    );
    // One ad has a typo in the bedroom count.
    let mut typo = photos(house(62, BUENO), &[near(h[9], 1), near(h[10], 1)]);
    typo.bedrooms = Some(2);
    add(
        "same-with-typo",
        S::ConflictingMetadata,
        true,
        photos(house(61, BUENO), &[h[9], h[10]]),
        typo,
    );

    // --- different properties ------------------------------------------------------------
    // Neighbouring houses of one developer, same copy-pasted text, no photos in common.
    add(
        "similar-houses-same-copy",
        S::SimilarProperties,
        false,
        words(house(71, BUENO), TEXT_A),
        words(house(72, BUENO), TEXT_A),
    );
    // Two identical flats of one building, the developer's photos on both ads.
    add(
        "flats-same-floorplan",
        S::Condominium,
        false,
        photos(flat(house(81, BUENO), 8_000), &[h[11], h[12]]),
        photos(flat(house(82, BUENO), 8_000), &[h[11], h[12]]),
    );
    // Two flats of one building with different floorplans, same photos.
    add(
        "flats-different-floorplan",
        S::Condominium,
        false,
        photos(flat(house(91, BUENO), 6_000), &[h[13], h[14]]),
        photos(flat(house(92, BUENO), 9_000), &[h[13], h[14]]),
    );
    // One stock photo in common, very different houses.
    let mut big = photos(house(102, BUENO), &[h[15], h[17]]);
    big.built_area_centi_m2 = Some(24_000);
    add(
        "similar-houses-one-stock-photo",
        S::SimilarProperties,
        false,
        photos(house(101, BUENO), &[h[15], h[16]]),
        big,
    );
    // Identical photos, different cities (a stock catalogue picture).
    let mut elsewhere = photos(house(112, BUENO), &[h[18], h[19]]);
    elsewhere.municipality = Some("aparecida de goiania".into());
    add(
        "same-photos-other-city",
        S::ConflictingMetadata,
        false,
        photos(house(111, BUENO), &[h[18], h[19]]),
        elsewhere,
    );
    // Same neighbourhood, nothing else alike.
    let mut other = words(house(122, BUENO), TEXT_B);
    other.built_area_centi_m2 = Some(5_000);
    other.bedrooms = Some(2);
    add(
        "same-neighbourhood-nothing-alike",
        S::SimilarProperties,
        false,
        words(house(121, BUENO), TEXT_A),
        other,
    );
    // A placeholder photo ("no photo available") on four unrelated ads.
    add(
        "placeholder-photo",
        S::Other,
        false,
        photos(house(131, "jardim america"), &[h[20]]),
        photos(house(132, "setor oeste"), &[h[20]]),
    );
    extra.push(photos(house(133, "setor sul"), &[h[20]]));
    extra.push(photos(house(134, "vila nova"), &[h[20]]));
    // KNOWN WEAK SPOT: twin houses of one developer, with the same marketing photos on both ads.
    add(
        "twin-houses-shared-marketing-photos",
        S::SimilarProperties,
        false,
        photos(house(141, BUENO), &[h[21], h[22]]),
        photos(house(142, BUENO), &[h[21], h[22]]),
    );

    listings.extend(extra);

    LabeledSet {
        description: "hand-written pairs exercising the rule, including its known weak spots"
            .to_owned(),
        provenance: Provenance::Synthetic,
        listings,
        pairs,
    }
}

fn report() -> EvaluationReport {
    evaluate(&synthetic_set(), &MatchConfig::default()).unwrap()
}

#[test]
fn the_report_counts_each_kind_of_error_separately_with_denominators() {
    let r = report();
    assert_eq!(r.pairs, 14);
    assert_eq!(r.rule_version, RULE_VERSION);
    assert_eq!(
        r.same_property,
        CategoryCounts {
            high: 2,
            possible: 2,
            low: 1,
            not_proposed: 1
        }
    );
    assert_eq!(
        r.different_property,
        CategoryCounts {
            high: 1,
            possible: 3,
            low: 3,
            not_proposed: 1
        }
    );
    assert_eq!(r.false_merges, Ratio { count: 1, of: 8 });
    assert_eq!(
        r.false_merge_ids,
        vec!["twin-houses-shared-marketing-photos"]
    );
    assert_eq!(r.false_review_items, Ratio { count: 4, of: 8 });
    assert_eq!(r.missed_duplicates, Ratio { count: 2, of: 6 });
    assert_eq!(
        r.missed_duplicate_ids,
        vec!["same-heavily-cropped", "same-no-photos-other-label"]
    );
    assert_eq!(r.duplicates_not_high, Ratio { count: 4, of: 6 });
}

#[test]
fn each_pair_outcome_is_listed_so_nothing_is_hidden_in_an_aggregate() {
    let r = report();
    assert_eq!(r.outcomes.len(), 14);
    let find = |id: &str| r.outcomes.iter().find(|o| o.id == id).unwrap();
    assert_eq!(
        find("same-cross-source").category,
        Some(MatchCategory::High)
    );
    assert_eq!(
        find("flats-same-floorplan").category,
        Some(MatchCategory::Possible),
        "an apartment is never high"
    );
    assert_eq!(
        find("same-photos-other-city").category,
        Some(MatchCategory::Low)
    );
    let unproposed = find("same-no-photos-other-label");
    assert_eq!((unproposed.proposed, unproposed.category), (false, None));
    let placeholder = find("placeholder-photo");
    assert_eq!(
        (placeholder.proposed, placeholder.category),
        (false, None),
        "a shared placeholder links nothing"
    );
}

#[test]
fn every_scenario_the_plan_names_is_represented_and_counted() {
    let r = report();
    for scenario in [
        Scenario::SameCrossSource,
        Scenario::SameDifferentAngle,
        Scenario::SimilarProperties,
        Scenario::Condominium,
        Scenario::ImageTransformation,
        Scenario::ConflictingMetadata,
    ] {
        let stats = r
            .by_scenario
            .get(&scenario)
            .unwrap_or_else(|| panic!("{scenario:?} is not covered"));
        assert!(stats.same_property.total() + stats.different_property.total() > 0);
    }
    let condominiums = r.by_scenario[&Scenario::Condominium];
    assert_eq!(
        condominiums.different_property,
        CategoryCounts {
            high: 0,
            possible: 2,
            low: 0,
            not_proposed: 0
        }
    );
    let transformed = r.by_scenario[&Scenario::ImageTransformation];
    assert_eq!(
        transformed.same_property,
        CategoryCounts {
            high: 1,
            possible: 0,
            low: 1,
            not_proposed: 0
        }
    );
    let total: usize = r
        .by_scenario
        .values()
        .map(|s| s.same_property.total() + s.different_property.total())
        .sum();
    assert_eq!(
        total, r.pairs,
        "no pair is left out of the scenario breakdown"
    );
}

#[test]
fn a_synthetic_report_says_it_proves_nothing_about_real_advertisements() {
    let r = report();
    assert!(
        r.limitations
            .iter()
            .any(|l| l.contains("Synthetic") && l.contains("real advertisements")),
        "{:?}",
        r.limitations
    );
    assert!(
        r.limitations
            .iter()
            .any(|l| l.contains("experimental") && l.contains("automatic merging")),
        "{:?}",
        r.limitations
    );
}

#[test]
fn a_real_sample_is_reported_with_its_size_limits_and_not_as_synthetic() {
    let mut set = synthetic_set();
    set.provenance = Provenance::Real;
    let r = evaluate(&set, &MatchConfig::default()).unwrap();
    assert!(
        !r.limitations.iter().any(|l| l.contains("Synthetic")),
        "{:?}",
        r.limitations
    );
    assert!(
        r.limitations.iter().any(|l| l.contains("labelled sample")),
        "{:?}",
        r.limitations
    );
    assert!(
        r.limitations.iter().any(|l| l.contains("fewer than 30")),
        "{:?}",
        r.limitations
    );
}

#[test]
fn zero_false_merges_are_reported_with_the_bound_the_sample_actually_allows() {
    let h = spread(2);
    let mut set = synthetic_set();
    // Keep only different-property pairs that the rule keeps apart: 5 pairs, no false merge.
    set.pairs.retain(|p| {
        [
            "same-photos-other-city",
            "same-neighbourhood-nothing-alike",
            "similar-houses-one-stock-photo",
            "placeholder-photo",
        ]
        .contains(&p.id.as_str())
    });
    set.pairs
        .push(pair("extra", Scenario::Other, false, 11, 22));
    let _ = h;
    let r = evaluate(&set, &MatchConfig::default()).unwrap();
    assert_eq!(r.false_merges, Ratio { count: 0, of: 5 });
    let line = r
        .limitations
        .iter()
        .find(|l| l.contains("rule of three"))
        .unwrap_or_else(|| panic!("{:?}", r.limitations));
    assert!(
        line.contains("5 different-property pairs") && line.contains("600"),
        "{line}"
    );
}

#[test]
fn a_kind_of_pair_that_is_absent_is_said_to_be_undefined_not_reported_as_zero_errors() {
    let mut set = synthetic_set();
    set.pairs
        .retain(|p| !p.id.starts_with("same-") || p.id == "same-photos-other-city");
    set.pairs.retain(|p| p.scenario == Scenario::Condominium);
    let r = evaluate(&set, &MatchConfig::default()).unwrap();
    assert_eq!(r.missed_duplicates, Ratio { count: 0, of: 0 });
    assert!(
        r.limitations
            .iter()
            .any(|l| l.contains("no same-property pairs")),
        "{:?}",
        r.limitations
    );
    let empty = LabeledSet {
        pairs: vec![],
        ..synthetic_set()
    };
    let none = evaluate(&empty, &MatchConfig::default()).unwrap();
    assert_eq!(
        (none.pairs, none.false_merges.of, none.missed_duplicates.of),
        (0, 0, 0)
    );
    assert!(
        none.limitations
            .iter()
            .any(|l| l.contains("no different-property pairs"))
    );
}

#[test]
fn an_inconsistent_set_is_refused_instead_of_reported_on() {
    let refuse = |change: fn(&mut LabeledSet)| {
        let mut set = synthetic_set();
        change(&mut set);
        evaluate(&set, &MatchConfig::default()).unwrap_err()
    };
    let cases: Vec<(&str, Change)> = vec![
        ("duplicate listing id", |s| {
            s.listings.push(s.listings[0].clone());
        }),
        ("unknown listing", |s| s.pairs[0].listing_b = 9_999),
        ("a listing paired with itself", |s| {
            s.pairs[0].listing_b = s.pairs[0].listing_a;
        }),
        ("duplicate pair id", |s| {
            s.pairs[1].id = s.pairs[0].id.clone();
        }),
        ("the same pair twice", |s| {
            let again = LabeledPair {
                id: "again".into(),
                listing_a: s.pairs[0].listing_b,
                listing_b: s.pairs[0].listing_a,
                ..s.pairs[0].clone()
            };
            s.pairs.push(again);
        }),
    ];
    for (name, change) in cases {
        let error = refuse(change);
        assert!(
            matches!(error, Error::InvalidConfig { .. }),
            "{name}: {error}"
        );
    }
    let bad_config = MatchConfig {
        max_block_size: 0,
        ..MatchConfig::default()
    };
    assert!(evaluate(&synthetic_set(), &bad_config).is_err());
}

#[test]
fn the_labels_never_influence_the_rule() {
    let original = report();
    let mut flipped = synthetic_set();
    for p in &mut flipped.pairs {
        p.same_property = !p.same_property;
    }
    let flipped = evaluate(&flipped, &MatchConfig::default()).unwrap();
    let categories = |r: &EvaluationReport| {
        r.outcomes
            .iter()
            .map(|o| (o.id.clone(), o.category))
            .collect::<Vec<_>>()
    };
    assert_eq!(categories(&original), categories(&flipped));
    assert_eq!(original.same_property, flipped.different_property);
    assert_eq!(original.different_property, flipped.same_property);
}

#[test]
fn the_result_does_not_depend_on_pair_order_or_orientation() {
    let original = report();
    let mut shuffled = synthetic_set();
    shuffled.pairs.reverse();
    for p in &mut shuffled.pairs {
        std::mem::swap(&mut p.listing_a, &mut p.listing_b);
    }
    shuffled.listings.reverse();
    let again = evaluate(&shuffled, &MatchConfig::default()).unwrap();
    assert_eq!(again.same_property, original.same_property);
    assert_eq!(again.different_property, original.different_property);
    assert_eq!(again.false_merge_ids, original.false_merge_ids);
    assert_eq!(
        again.missed_duplicate_ids, original.missed_duplicate_ids,
        "ids are reported in a stable order"
    );
    let by_id = |r: &EvaluationReport| {
        let mut v: Vec<_> = r
            .outcomes
            .iter()
            .map(|o| (o.id.clone(), o.category))
            .collect();
        v.sort_by(|x, y| x.0.cmp(&y.0));
        v
    };
    assert_eq!(by_id(&again), by_id(&original));
}

#[test]
fn a_set_round_trips_through_json_and_group_roots_may_be_left_out() {
    let set = synthetic_set();
    let json = serde_json::to_string(&set).unwrap();
    assert_eq!(LabeledSet::from_json(&json).unwrap(), set);
    let mut value: serde_json::Value = serde_json::from_str(&json).unwrap();
    for listing in value["listings"].as_array_mut().unwrap() {
        listing.as_object_mut().unwrap().remove("group_root");
    }
    let loaded = LabeledSet::from_json(&value.to_string()).unwrap();
    assert_eq!(
        evaluate(&loaded, &MatchConfig::default()).unwrap(),
        report()
    );
    assert!(LabeledSet::from_json("{ nope").is_err());
}

#[test]
fn the_report_serialises_for_display_and_storage() {
    let json = serde_json::to_value(report()).unwrap();
    assert_eq!(json["false_merges"]["count"], 1);
    assert_eq!(json["false_merges"]["of"], 8);
    assert_eq!(json["provenance"], "synthetic");
    assert!(json["by_scenario"]["condominium"].is_object());
}
