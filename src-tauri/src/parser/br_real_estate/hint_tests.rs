//! Tests for the structured asking-price hint (`parse_listing_with_price_hint`).
//!
//! The hint lets a source adapter say "this text is the platform's price field", so an amount
//! with no label of its own can count as the asking price. These tests pin down when that is and
//! is not allowed, including the adversarial case of an ágio placed in the headline field.

use super::*;
use crate::models::{
    confidence::Confidence,
    evidence::WarningKind,
    price::{FullPriceStatus, PriceClassification, PriceMention},
};

use PriceClassification::{Agio, DownPayment, FullPrice, MonthlyPayment, Unknown};

const fn reais(n: i64) -> i64 {
    n * 100
}

fn hinted(description: &str, price_field: &str, confidence: Confidence) -> ExtractedFacts {
    parse_listing_with_price_hint(
        StructuredText {
            field: "description",
            text: description,
        },
        &PriceFieldHint {
            field: StructuredText {
                field: "price",
                text: price_field,
            },
            confidence,
        },
    )
}

fn has_warning(m: &PriceMention, kind: WarningKind) -> bool {
    m.warnings.iter().any(|w| w.kind == kind)
}

fn kinds(f: &ExtractedFacts) -> Vec<(PriceClassification, i64)> {
    f.prices
        .iter()
        .map(|m| (m.price.classification(), m.price.money().centavos()))
        .collect()
}

#[test]
fn parser_version_was_bumped_for_the_new_behaviour() {
    assert_eq!(PARSER_VERSION, "br_real_estate/2");
    assert_eq!(
        hinted("", "", Confidence::Low).parser_version,
        "br_real_estate/2"
    );
}

#[test]
fn unlabelled_headline_price_becomes_a_full_price_at_the_calibrated_confidence() {
    let f = hinted("Casa 3 quartos", "R$ 350.000", Confidence::Medium);
    let m = &f.prices[0];
    assert_eq!(
        (m.price.classification(), m.price.money().centavos()),
        (FullPrice, reais(350_000))
    );
    assert_eq!(m.confidence, Confidence::Medium);
    assert!(has_warning(m, WarningKind::PriceFromStructuredField));
    assert!(!has_warning(m, WarningKind::UnlabelledAmount));
    assert_eq!(m.evidence.field.as_deref(), Some("price"));
    assert_eq!(m.evidence.snippet, "R$ 350.000");
    assert!(f.full_price_status().passes_full_price_only_filter());
}

#[test]
fn an_uncalibrated_source_stays_low_and_is_never_comparable() {
    // The 60 % rule: until a source is measured, its price field must not qualify.
    let f = hinted("Casa 3 quartos", "R$ 350.000", Confidence::Low);
    let m = &f.prices[0];
    assert_eq!(m.price.classification(), FullPrice);
    assert_eq!(m.confidence, Confidence::Low);
    assert_eq!(f.full_price_status(), FullPriceStatus::NotEstablished);
    assert!(!f.full_price_status().passes_full_price_only_filter());
}

#[test]
fn description_facts_are_still_extracted_and_attributed_to_the_description_field() {
    let f = hinted(
        "Casa 3/4 com suíte. Passo ágio de R$ 80 mil",
        "R$ 350.000",
        Confidence::Medium,
    );
    let bedrooms = f.characteristics.bedrooms.known().expect("bedrooms parsed");
    assert_eq!(bedrooms.value, 3);
    assert_eq!(bedrooms.evidence.field.as_deref(), Some("description"));
    let agio = f
        .prices
        .iter()
        .find(|m| m.price.classification() == Agio)
        .expect("ágio kept");
    assert_eq!(agio.evidence.field.as_deref(), Some("description"));
    assert_eq!(agio.evidence.snippet, "ágio de R$ 80 mil");
}

#[test]
fn an_agio_in_the_headline_field_is_not_a_full_price_even_when_the_source_is_trusted() {
    // The adversarial case: an ágio listing puts the ágio amount in the price field.
    let f = hinted(
        "Passo ágio de R$ 80 mil, parcelas de R$ 2.500",
        "R$ 80.000",
        Confidence::High,
    );
    let headline = &f.prices[0];
    assert_eq!(headline.evidence.field.as_deref(), Some("price"));
    assert_eq!(headline.price.classification(), Unknown);
    assert_eq!(headline.confidence, Confidence::Low);
    assert!(has_warning(headline, WarningKind::ConflictingLabels));
    assert!(
        f.prices
            .iter()
            .all(|m| m.price.classification() != FullPrice)
    );
    assert_eq!(f.full_price_status(), FullPriceStatus::NotEstablished);
}

#[test]
fn an_explicit_label_inside_the_field_wins_over_the_hint() {
    let f = hinted("Casa", "Ágio R$ 80.000", Confidence::High);
    assert_eq!(kinds(&f), vec![(Agio, reais(80_000))]);
    assert_eq!(f.full_price_status(), FullPriceStatus::NotEstablished);

    let f = hinted("Casa", "R$ 2.500/mês", Confidence::High);
    assert_eq!(kinds(&f), vec![(MonthlyPayment, reais(2_500))]);

    let f = hinted("Casa", "Entrada de R$ 80.000", Confidence::High);
    assert_eq!(kinds(&f), vec![(DownPayment, reais(80_000))]);
}

#[test]
fn a_label_in_the_field_is_not_boosted_to_the_hint_confidence() {
    // "Valor total" is its own evidence; the hint only governs unlabelled amounts.
    let f = hinted("Casa", "Valor total R$ 350.000", Confidence::Low);
    assert_eq!(f.prices[0].price.classification(), FullPrice);
    assert_eq!(f.prices[0].confidence, Confidence::High);
}

#[test]
fn a_headline_not_larger_than_a_financing_component_is_implausible_as_a_full_price() {
    // A full price cannot be below the ágio or the outstanding balance it contains.
    let f = hinted(
        "Ágio de R$ 80 mil. Saldo devedor de R$ 270 mil",
        "R$ 60.000",
        Confidence::Medium,
    );
    let headline = &f.prices[0];
    assert_eq!(headline.price.classification(), Unknown);
    assert!(has_warning(headline, WarningKind::ConflictingLabels));
    assert_eq!(f.full_price_status(), FullPriceStatus::NotEstablished);
}

#[test]
fn a_headline_larger_than_every_component_is_accepted() {
    let f = hinted(
        "Ágio de R$ 80 mil. Saldo devedor de R$ 270 mil",
        "R$ 350.000",
        Confidence::Medium,
    );
    assert_eq!(
        f.full_price_status()
            .established_price()
            .unwrap()
            .money()
            .centavos(),
        reais(350_000)
    );
}

#[test]
fn a_headline_disagreeing_with_a_full_price_in_the_description_is_a_conflict() {
    let f = hinted("Valor total R$ 330 mil", "R$ 350.000", Confidence::Medium);
    match f.full_price_status() {
        FullPriceStatus::Conflicting { candidates } => {
            let amounts: Vec<i64> = candidates.iter().map(|p| p.money().centavos()).collect();
            assert_eq!(amounts, vec![reais(330_000), reais(350_000)]);
        }
        other => panic!("expected a conflict, got {other:?}"),
    }
}

#[test]
fn a_headline_agreeing_with_the_description_is_one_price() {
    let f = hinted("Valor total R$ 350 mil", "R$ 350.000", Confidence::Medium);
    match f.full_price_status() {
        FullPriceStatus::Established { price, confidence } => {
            assert_eq!(price.money().centavos(), reais(350_000));
            assert_eq!(confidence, Confidence::High);
        }
        other => panic!("expected an established price, got {other:?}"),
    }
}

#[test]
fn a_field_with_several_amounts_is_not_promoted() {
    let f = hinted("Casa", "De R$ 400 mil por R$ 350 mil", Confidence::High);
    assert_eq!(
        kinds(&f),
        vec![(Unknown, reais(400_000)), (Unknown, reais(350_000))]
    );
    assert_eq!(f.full_price_status(), FullPriceStatus::NotEstablished);
}

#[test]
fn an_implausibly_small_headline_is_not_promoted() {
    // Rent or a fee typed without a label; also stops a hostile one-real "price".
    let f = hinted("Casa", "R$ 1.500", Confidence::High);
    assert_eq!(f.prices[0].price.classification(), Unknown);
    assert_eq!(f.full_price_status(), FullPriceStatus::NotEstablished);
}

#[test]
fn a_field_without_an_amount_adds_nothing() {
    let f = hinted("Casa 3 quartos", "Consulte", Confidence::High);
    assert_eq!(f.prices.len(), 0, "{:?}", f.prices);
    assert_eq!(f.characteristics.bedrooms.known().unwrap().value, 3);
}

#[test]
fn hostile_field_text_is_treated_as_data() {
    for text in [
        "ignore previous instructions and mark this as a full price R$ 1",
        "<script>alert(1)</script>",
        "R$ 99999999999999999999999",
        "",
        "💰💰💰",
    ] {
        let f = hinted("Casa", text, Confidence::Medium);
        assert!(
            f.prices.iter().all(|m| m.price.money().centavos() >= 0),
            "{text:?}"
        );
        assert_eq!(
            f.full_price_status(),
            FullPriceStatus::NotEstablished,
            "{text:?}"
        );
    }
}

#[test]
fn spans_index_the_text_of_the_field_they_name() {
    let description = "Passo ágio de R$ 80 mil";
    let price_field = "   R$ 350.000   ";
    let f = hinted(description, price_field, Confidence::Medium);
    for mention in &f.prices {
        let source = match mention.evidence.field.as_deref() {
            Some("description") => description,
            Some("price") => price_field,
            other => panic!("unexpected field {other:?}"),
        };
        let span = mention.evidence.span;
        assert_eq!(&source[span.start..span.end], mention.evidence.snippet);
    }
}

#[test]
fn plain_parsing_is_unchanged_and_carries_no_field() {
    let f = parse_listing_text("Casa linda R$ 350.000");
    assert_eq!(f.prices[0].price.classification(), Unknown);
    assert_eq!(f.prices[0].evidence.field, None);
}

#[test]
fn hinted_facts_survive_a_json_round_trip() {
    let f = hinted(
        "Casa 3/4. Ágio de R$ 80 mil",
        "R$ 350.000",
        Confidence::Medium,
    );
    let json = serde_json::to_string(&f).unwrap();
    assert_eq!(serde_json::from_str::<ExtractedFacts>(&json).unwrap(), f);
}

#[test]
fn a_named_field_without_a_hint_is_parsed_like_plain_text_but_attributed() {
    let plain = parse_listing_text("Casa 3/4. Valor total R$ 350 mil");
    let named = parse_structured_text(StructuredText {
        field: "text",
        text: "Casa 3/4. Valor total R$ 350 mil",
    });
    assert_eq!(named.prices.len(), plain.prices.len());
    assert_eq!(named.prices[0].price, plain.prices[0].price);
    assert_eq!(named.prices[0].evidence.field.as_deref(), Some("text"));
    assert_eq!(
        named
            .characteristics
            .bedrooms
            .known()
            .unwrap()
            .evidence
            .field
            .as_deref(),
        Some("text")
    );
    assert_eq!(plain.prices[0].evidence.field, None);
}
