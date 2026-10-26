use super::*;
use crate::models::{
    confidence::Confidence,
    evidence::{DiscardReason, WarningKind},
    price::{FullPriceStatus, PriceClassification, PriceMention},
};

use PriceClassification::{
    Agio, DownPayment, FullPrice, MonthlyPayment, OutstandingBalance, Unknown,
};

// ---- helpers -------------------------------------------------------------------------

const fn reais(n: i64) -> i64 {
    n * 100
}

fn facts(text: &str) -> ExtractedFacts {
    parse_listing_text(text)
}

/// `(classification, centavos)` for every price mention, in text order.
fn amounts(f: &ExtractedFacts) -> Vec<(PriceClassification, i64)> {
    f.prices
        .iter()
        .map(|m| (m.price.classification(), m.price.money().centavos()))
        .collect()
}

fn only_price(f: &ExtractedFacts) -> &PriceMention {
    assert_eq!(
        f.prices.len(),
        1,
        "expected exactly one price mention, got {:#?}",
        f.prices
    );
    &f.prices[0]
}

fn has_warning(m: &PriceMention, kind: WarningKind) -> bool {
    m.warnings.iter().any(|w| w.kind == kind)
}

fn bedrooms(f: &ExtractedFacts) -> Option<u8> {
    f.characteristics.bedrooms.known().map(|e| e.value)
}

fn suites(f: &ExtractedFacts) -> Option<u8> {
    f.characteristics.suites.known().map(|e| e.value)
}

fn parking(f: &ExtractedFacts) -> Option<u8> {
    f.characteristics.parking_spaces.known().map(|e| e.value)
}

fn lot_centi(f: &ExtractedFacts) -> Option<u32> {
    f.characteristics
        .lot_area
        .known()
        .map(|e| e.value.area().centi_square_meters())
}

fn built_centi(f: &ExtractedFacts) -> Option<u32> {
    f.characteristics
        .built_area
        .known()
        .map(|e| e.value.area().centi_square_meters())
}

fn usable_centi(f: &ExtractedFacts) -> Option<u32> {
    f.characteristics
        .usable_area
        .known()
        .map(|e| e.value.area().centi_square_meters())
}

fn unspecified_centi(f: &ExtractedFacts) -> Vec<u32> {
    f.characteristics
        .unspecified_areas
        .iter()
        .map(|e| e.value.area().centi_square_meters())
        .collect()
}

fn discarded_reasons(f: &ExtractedFacts) -> Vec<DiscardReason> {
    f.discarded.iter().map(|d| d.reason).collect()
}

// ---- 4.1 currency normalisation --------------------------------------------------------

#[test]
fn parses_brl_amount_with_symbol_and_keeps_it_unlabelled() {
    let f = facts("R$ 350.000,00");
    let m = only_price(&f);
    assert_eq!(m.price.money().centavos(), reais(350_000));
    // A bare amount is parsed exactly, but nothing says it is the full price.
    assert_eq!(m.price.classification(), Unknown);
    assert!(has_warning(m, WarningKind::UnlabelledAmount));
    assert_eq!(m.evidence.snippet, "R$ 350.000,00");
}

#[test]
fn parses_mil_with_price_context() {
    let f = facts("Casa à venda por 350 mil");
    let m = only_price(&f);
    assert_eq!(
        (m.price.classification(), m.price.money().centavos()),
        (FullPrice, reais(350_000))
    );
    assert_eq!(m.confidence, Confidence::Medium);
}

#[test]
fn parses_350_mil_reais_and_k_suffix() {
    assert_eq!(
        amounts(&facts("valor 350 mil reais")),
        vec![(FullPrice, reais(350_000))]
    );
    assert_eq!(
        amounts(&facts("valor 350k")),
        vec![(FullPrice, reais(350_000))]
    );
    assert_eq!(
        amounts(&facts("Valor: R$350K")),
        vec![(FullPrice, reais(350_000))]
    );
}

#[test]
fn parses_milhao() {
    assert_eq!(
        amounts(&facts("valor total R$ 1,2 milhão")),
        vec![(FullPrice, reais(1_200_000))]
    );
    assert_eq!(
        amounts(&facts("preço R$ 2 milhões")),
        vec![(FullPrice, reais(2_000_000))]
    );
}

#[test]
fn bare_formatted_number_is_a_price_only_with_an_adjacent_label() {
    assert_eq!(
        amounts(&facts("valor 1.200.000")),
        vec![(FullPrice, reais(1_200_000))]
    );
    assert_eq!(
        amounts(&facts("ágio de 80000")),
        vec![(Agio, reais(80_000))]
    );
    assert_eq!(
        amounts(&facts("parcelas de 2.500")),
        vec![(MonthlyPayment, reais(2_500))]
    );
    // Same digits, no label next to them: not an amount at all.
    assert_eq!(facts("casa com 3 quartos 1.200.000").prices, []);
    // A label far away from the digits does not count.
    assert_eq!(facts("valor atualizado em 2024").prices, []);
}

#[test]
fn numbers_that_are_not_money_are_not_amounts() {
    let f = facts("3 quartos, 2 vagas, 300 m², tel (62) 99999-9999, lote 12 quadra 5, 2024");
    assert!(f.prices.is_empty(), "{:#?}", f.prices);
}

#[test]
fn amount_range_with_missing_magnitude_is_discarded_not_guessed() {
    let f = facts("de R$ 300 a 400 mil");
    assert!(discarded_reasons(&f).contains(&DiscardReason::AmbiguousAmountRange));
    assert!(
        f.prices
            .iter()
            .all(|m| m.price.money().centavos() != reais(300)),
        "the bare 300 must not become R$ 300: {:#?}",
        f.prices
    );
}

#[test]
fn unrepresentable_amounts_are_discarded() {
    let f = facts("R$ 99999999999999999999");
    assert_eq!(f.prices, []);
    assert!(discarded_reasons(&f).contains(&DiscardReason::UnparseableNumber));
}

// ---- 4.2 price classification ----------------------------------------------------------

#[test]
fn agio_is_never_a_full_price() {
    for text in [
        "ágio de R$ 80 mil",
        "passo ágio R$ 80.000",
        "valor do ágio: R$ 80 mil",
        "ÁGIO DE R$ 80 MIL",
    ] {
        let f = facts(text);
        let m = only_price(&f);
        assert_eq!(m.price.classification(), Agio, "{text}");
        assert_eq!(m.price.money().centavos(), reais(80_000), "{text}");
        assert!(
            !f.full_price_status().passes_full_price_only_filter(),
            "{text}"
        );
    }
}

#[test]
fn agio_evidence_quotes_the_original_text() {
    let f = facts("Vendo. ÁGIO DE R$ 80 MIL, aceita carro");
    let m = only_price(&f);
    assert_eq!(m.evidence.snippet, "ÁGIO DE R$ 80 MIL");
    assert_eq!(m.confidence, Confidence::High);
}

#[test]
fn down_payment_is_not_a_full_price() {
    let f = facts("entrada de R$ 80 mil");
    let m = only_price(&f);
    assert_eq!(
        (m.price.classification(), m.price.money().centavos()),
        (DownPayment, reais(80_000))
    );
    assert_eq!(f.full_price_status(), FullPriceStatus::NotEstablished);
}

#[test]
fn instalments_are_monthly_payments_not_prices() {
    for text in [
        "parcelas de R$ 2.500",
        "assumir parcelas de R$ 2.500",
        "R$ 2.500 por mês",
        "R$ 2.500/mês",
        "R$ 2.500 mensais",
        "prestação R$ 2.500",
    ] {
        assert_eq!(
            amounts(&facts(text)),
            vec![(MonthlyPayment, reais(2_500))],
            "{text}"
        );
    }
}

#[test]
fn saldo_devedor_is_an_outstanding_balance() {
    let f = facts("saldo devedor de R$ 120 mil");
    assert_eq!(amounts(&f), vec![(OutstandingBalance, reais(120_000))]);
    assert!(!f.full_price_status().passes_full_price_only_filter());
}

#[test]
fn explicit_total_wording_gives_a_high_confidence_full_price() {
    for text in ["valor total R$ 350 mil", "Preço total: R$ 350.000,00"] {
        let f = facts(text);
        let m = only_price(&f);
        assert_eq!(m.price.classification(), FullPrice, "{text}");
        assert_eq!(m.confidence, Confidence::High, "{text}");
        assert!(
            f.full_price_status().passes_full_price_only_filter(),
            "{text}"
        );
    }
}

#[test]
fn full_price_candidate_and_down_payment_are_kept_separate() {
    let f = facts("R$ 350 mil ou entrada de R$ 80 mil");
    assert_eq!(
        amounts(&f),
        vec![(FullPrice, reais(350_000)), (DownPayment, reais(80_000))]
    );
    // The unlabelled 350 mil is only a candidate: low confidence, flagged, not comparable.
    assert_eq!(f.prices[0].confidence, Confidence::Low);
    assert!(has_warning(&f.prices[0], WarningKind::AssumedFullPrice));
    assert_eq!(f.prices[1].confidence, Confidence::High);
    assert_eq!(f.full_price_status(), FullPriceStatus::NotEstablished);
}

#[test]
fn a_full_financing_description_is_split_into_its_components() {
    let f = facts(
        "Passo ágio de R$ 80 mil. Saldo devedor de R$ 270 mil, parcelas de R$ 2.500. Valor total R$ 350 mil",
    );
    assert_eq!(
        amounts(&f),
        vec![
            (Agio, reais(80_000)),
            (OutstandingBalance, reais(270_000)),
            (MonthlyPayment, reais(2_500)),
            (FullPrice, reais(350_000)),
        ]
    );
    let status = f.full_price_status();
    assert_eq!(
        status.established_price().unwrap().money().centavos(),
        reais(350_000)
    );
}

#[test]
fn listing_with_only_agio_has_no_full_price() {
    let f = facts("Passo ágio R$ 80 mil");
    assert_eq!(f.full_price_status(), FullPriceStatus::NotEstablished);
    assert!(!f.full_price_status().passes_full_price_only_filter());
}

#[test]
fn unlabelled_amount_is_never_silently_a_full_price() {
    let f = facts("Casa linda R$ 350.000");
    let m = only_price(&f);
    assert_eq!(m.price.classification(), Unknown);
    assert_eq!(m.confidence, Confidence::Low);
    assert!(!f.full_price_status().passes_full_price_only_filter());
}

#[test]
fn amounts_labelled_as_something_else_are_not_sale_prices() {
    let f = facts("Condomínio R$ 450, IPTU R$ 120, aluguel R$ 1.500");
    assert_eq!(f.prices.len(), 3);
    for m in &f.prices {
        assert_eq!(m.price.classification(), Unknown);
        assert!(has_warning(m, WarningKind::NonSaleAmount));
    }
}

#[test]
fn financing_amount_is_not_promoted_to_a_full_price() {
    let f = facts("entrada de R$ 80 mil, financiamento R$ 270 mil");
    assert_eq!(
        amounts(&f),
        vec![(DownPayment, reais(80_000)), (Unknown, reais(270_000))]
    );
    assert_eq!(f.full_price_status(), FullPriceStatus::NotEstablished);
}

#[test]
fn nearest_label_wins_over_an_earlier_unrelated_one() {
    let f = facts("aceita financiamento, valor R$ 350 mil");
    assert_eq!(amounts(&f), vec![(FullPrice, reais(350_000))]);
}

#[test]
fn label_attached_after_the_amount_is_honoured() {
    assert_eq!(
        amounts(&facts("80 mil de ágio")),
        vec![(Agio, reais(80_000))]
    );
    let f = facts("R$ 80 mil de entrada");
    assert_eq!(amounts(&f), vec![(DownPayment, reais(80_000))]);
    assert_eq!(f.prices[0].evidence.snippet, "R$ 80 mil de entrada");
}

#[test]
fn trailing_label_does_not_steal_the_next_amounts_label() {
    let f = facts("R$ 350 mil entrada de R$ 80 mil");
    assert_eq!(
        amounts(&f),
        vec![(FullPrice, reais(350_000)), (DownPayment, reais(80_000))]
    );
    assert_eq!(f.prices[0].confidence, Confidence::Low);
}

#[test]
fn contradicting_labels_make_the_amount_unknown() {
    let f = facts("ágio R$ 80 mil de entrada");
    let m = only_price(&f);
    assert_eq!(m.price.classification(), Unknown);
    assert_eq!(m.confidence, Confidence::Low);
    assert!(has_warning(m, WarningKind::ConflictingLabels));
}

#[test]
fn entrada_as_a_door_is_not_a_down_payment() {
    let f = facts("casa com entrada independente R$ 350 mil");
    assert!(
        f.prices
            .iter()
            .all(|m| m.price.classification() != DownPayment),
        "{:#?}",
        f.prices
    );
}

#[test]
fn disagreeing_full_prices_are_a_conflict_and_fail_the_filter() {
    let f = facts("valor total R$ 350 mil. Preço R$ 330 mil");
    assert!(matches!(
        f.full_price_status(),
        FullPriceStatus::Conflicting { .. }
    ));
    assert!(!f.full_price_status().passes_full_price_only_filter());
}

#[test]
fn magnitude_amounts_are_parsed_but_stay_unlabelled_on_their_own() {
    for text in ["350 mil", "350k", "350 mil reais"] {
        let f = facts(text);
        let m = only_price(&f);
        assert_eq!(
            (m.price.classification(), m.price.money().centavos()),
            (Unknown, reais(350_000)),
            "{text}"
        );
    }
}

#[test]
fn bare_saldo_is_a_balance_never_a_price() {
    let f = facts("Ágio R$ 80 mil, parcelas R$ 2.500, saldo R$ 150 mil");
    assert_eq!(
        amounts(&f),
        vec![
            (Agio, reais(80_000)),
            (MonthlyPayment, reais(2_500)),
            (OutstandingBalance, reais(150_000))
        ]
    );
    assert!(
        f.prices[2].confidence <= Confidence::Medium,
        "bare `saldo` is weaker than `saldo devedor`"
    );
    assert_eq!(f.full_price_status(), FullPriceStatus::NotEstablished);
}

#[test]
fn bare_number_after_label_with_punctuation() {
    assert_eq!(
        amounts(&facts("valor: 350.000")),
        vec![(FullPrice, reais(350_000))]
    );
    assert_eq!(
        amounts(&facts("Preço - 350.000")),
        vec![(FullPrice, reais(350_000))]
    );
}

#[test]
fn instalment_count_is_not_mistaken_for_an_amount() {
    let f = facts("Entrada R$ 80.000 + 120 parcelas de R$ 2.500");
    assert_eq!(
        amounts(&f),
        vec![(DownPayment, reais(80_000)), (MonthlyPayment, reais(2_500))]
    );
}

#[test]
fn digits_followed_by_a_unit_are_not_money_even_after_a_label() {
    assert_eq!(facts("valor 1.250 m²").prices, []);
    assert_eq!(facts("preço 2.000 metros do centro").prices, []);
}

#[test]
fn labelled_full_price_next_to_a_parenthesised_down_payment() {
    let f = facts("valor R$ 350.000 (entrada R$ 80.000)");
    assert_eq!(
        amounts(&f),
        vec![(FullPrice, reais(350_000)), (DownPayment, reais(80_000))]
    );
    assert!(f.full_price_status().passes_full_price_only_filter());
}

#[test]
fn spelled_out_amount_in_words_is_not_double_counted() {
    let f = facts("valor R$ 350.000,00 (trezentos e cinquenta mil reais)");
    assert_eq!(amounts(&f), vec![(FullPrice, reais(350_000))]);
}

// ---- robustness of amount scanning -------------------------------------------------------

#[test]
fn tolerates_missing_spaces_punctuation_unicode_and_case() {
    assert_eq!(amounts(&facts("R$350mil")), vec![(Unknown, reais(350_000))]);
    assert_eq!(
        amounts(&facts("ágio:R$80.000")),
        vec![(Agio, reais(80_000))]
    );
    assert_eq!(
        amounts(&facts("Valor: R$ 350.000,00.")),
        vec![(FullPrice, reais(350_000))]
    );
    assert_eq!(
        amounts(&facts("ágio\u{a0}de R$\u{a0}80 mil!!")),
        vec![(Agio, reais(80_000))]
    );
    assert_eq!(
        amounts(&facts("A\u{301}GIO de R$ 80 mil")),
        vec![(Agio, reais(80_000))]
    );
    assert_eq!(
        amounts(&facts("🏠 ágio de R$ 80 mil 🔥")),
        vec![(Agio, reais(80_000))]
    );
}

#[test]
fn sentence_boundaries_stop_labels_from_leaking() {
    let f = facts("Entrada de R$ 80 mil.\nValor R$ 350 mil");
    assert_eq!(
        amounts(&f),
        vec![(DownPayment, reais(80_000)), (FullPrice, reais(350_000))]
    );
}

// ---- 4.3 characteristics ---------------------------------------------------------------

#[test]
fn shorthand_bedrooms_suite_and_parking() {
    let f = facts("3/4, 1 suíte, 2 vagas");
    assert_eq!(bedrooms(&f), Some(3));
    assert_eq!(suites(&f), Some(1));
    assert_eq!(parking(&f), Some(2));
    assert_eq!(
        f.characteristics.bedrooms.known().unwrap().evidence.snippet,
        "3/4"
    );
}

#[test]
fn spelled_out_counts() {
    let f = facts("3 quartos, 1 suíte, 2 vagas de garagem");
    assert_eq!(
        (bedrooms(&f), suites(&f), parking(&f)),
        (Some(3), Some(1), Some(2))
    );
    let f = facts("três quartos, duas suítes, garagem para 2 carros");
    assert_eq!(
        (bedrooms(&f), suites(&f), parking(&f)),
        (Some(3), Some(2), Some(2))
    );
    let f = facts("02 vagas");
    assert_eq!(parking(&f), Some(2));
}

#[test]
fn suites_are_not_bedrooms() {
    let f = facts("1 suíte");
    assert_eq!(suites(&f), Some(1));
    assert!(f.characteristics.bedrooms.is_unknown());
    let f = facts("3 quartos sendo 1 suíte");
    assert_eq!((bedrooms(&f), suites(&f)), (Some(3), Some(1)));
}

#[test]
fn bare_singular_suite_counts_as_one_but_says_so() {
    let f = facts("casa com suíte e closet");
    let suite = f.characteristics.suites.known().unwrap();
    assert_eq!(suite.value, 1);
    assert_eq!(suite.confidence, Confidence::Medium);
    assert!(
        suite
            .warnings
            .iter()
            .any(|w| w.kind == WarningKind::CountImpliedBySingular)
    );
}

#[test]
fn bare_plural_suites_has_no_count() {
    let f = facts("casa com suítes");
    assert!(f.characteristics.suites.is_unknown());
    assert!(discarded_reasons(&f).contains(&DiscardReason::PluralWithoutCount));
}

#[test]
fn two_quartos_plus_suite_flags_that_the_total_is_unclear() {
    let f = facts("2 quartos + 1 suíte");
    assert_eq!((bedrooms(&f), suites(&f)), (Some(2), Some(1)));
    let suite = f.characteristics.suites.known().unwrap();
    assert!(
        suite
            .warnings
            .iter()
            .any(|w| w.kind == WarningKind::SuitesMayBeAdditionalToBedrooms)
    );
}

#[test]
fn fraction_of_lot_is_not_a_bedroom_count() {
    let f = facts("3/4 de lote");
    assert!(f.characteristics.bedrooms.is_unknown());
    assert!(discarded_reasons(&f).contains(&DiscardReason::FractionOfLot));
}

#[test]
fn other_fractions_are_not_bedroom_counts_either() {
    let f = facts("aceito 3/4 do valor em carro");
    assert!(f.characteristics.bedrooms.is_unknown());
    assert!(discarded_reasons(&f).contains(&DiscardReason::FractionNotBedrooms));
}

#[test]
fn shorthand_needs_bedroom_context() {
    let f = facts("casa 3/4");
    assert_eq!(bedrooms(&f), Some(3));
    let f = facts("3/4");
    assert!(f.characteristics.bedrooms.is_unknown());
    assert!(discarded_reasons(&f).contains(&DiscardReason::BedroomShorthandWithoutContext));
}

#[test]
fn shorthand_followed_by_quartos_is_ambiguous() {
    let f = facts("casa com 3/4 quartos");
    assert!(f.characteristics.bedrooms.is_unknown());
    assert!(discarded_reasons(&f).contains(&DiscardReason::AmbiguousBedroomRange));
}

#[test]
fn dates_are_not_bedroom_shorthand() {
    let f = facts("casa publicada em 03/04/2024 com 2 vagas");
    assert!(f.characteristics.bedrooms.is_unknown());
    assert_eq!(parking(&f), Some(2));
}

#[test]
fn agreeing_bedroom_mentions_merge_and_disagreeing_ones_conflict() {
    let f = facts("casa 3/4, 3 quartos");
    assert_eq!(bedrooms(&f), Some(3));
    let f = facts("3 quartos no anúncio e 2 quartos na descrição");
    assert!(f.characteristics.bedrooms.is_conflicting());
    assert_eq!(
        bedrooms(&f),
        None,
        "a conflict must not be resolved silently"
    );
}

#[test]
fn implausible_counts_are_discarded() {
    let f = facts("99 quartos");
    assert!(f.characteristics.bedrooms.is_unknown());
    assert!(discarded_reasons(&f).contains(&DiscardReason::ImplausibleCount));
}

#[test]
fn half_lot_wording_next_to_bedroom_shorthand() {
    let f = facts("casa 3/4 com 1/2 lote");
    assert_eq!(bedrooms(&f), Some(3));
    assert_eq!(
        f.characteristics.lot_share.known().unwrap().value.as_str(),
        "half"
    );
    let f = facts("1/4 de lote");
    assert!(
        f.characteristics.bedrooms.is_unknown(),
        "1/4 of a lot is not one bedroom"
    );
    assert!(discarded_reasons(&f).contains(&DiscardReason::FractionOfLot));
}

// ---- areas ---------------------------------------------------------------------------

#[test]
fn lot_area_is_stored_as_lot_area() {
    for text in [
        "lote 300 m²",
        "lote de 300 m²",
        "300 m² de lote",
        "300m² de terreno",
        "terreno: 300 m2",
    ] {
        let f = facts(text);
        assert_eq!(lot_centi(&f), Some(30_000), "{text}");
        assert_eq!(built_centi(&f), None, "{text}");
        assert!(f.characteristics.unspecified_areas.is_empty(), "{text}");
    }
}

#[test]
fn built_area_is_stored_as_built_area() {
    for text in [
        "180 m² de área construída",
        "área construída de 180 m²",
        "180m2 construídos",
        "construção: 180 m²",
        "180 m² de construção",
    ] {
        let f = facts(text);
        assert_eq!(built_centi(&f), Some(18_000), "{text}");
        assert_eq!(lot_centi(&f), None, "{text}");
    }
    let f = facts("180 m² de área construída");
    assert_eq!(
        f.characteristics
            .built_area
            .known()
            .unwrap()
            .evidence
            .snippet,
        "180 m² de área construída"
    );
}

#[test]
fn usable_area_is_its_own_kind() {
    let f = facts("área útil 90 m²");
    assert_eq!(usable_centi(&f), Some(9_000));
    assert_eq!((lot_centi(&f), built_centi(&f)), (None, None));
    assert_eq!(usable_centi(&facts("80 m² de área privativa")), Some(8_000));
}

#[test]
fn unlabelled_area_is_never_promoted_to_lot_or_built() {
    for text in ["150 m²", "área 150 m²", "área total 150 m²"] {
        let f = facts(text);
        assert_eq!(unspecified_centi(&f), vec![15_000], "{text}");
        assert_eq!(
            (lot_centi(&f), built_centi(&f), usable_centi(&f)),
            (None, None, None),
            "{text}"
        );
    }
    let f = facts("área total 150 m²");
    assert!(
        f.characteristics.unspecified_areas[0]
            .warnings
            .iter()
            .any(|w| w.kind == WarningKind::UnspecifiedAreaKind)
    );
}

#[test]
fn lot_and_built_in_the_same_text_are_never_swapped() {
    for text in [
        "lote 300 m² e 180 m² de área construída",
        "Terreno 300 m2 construção 180 m2",
        "180 m² de construção em lote de 300 m²",
        "casa de 180 m² de construção e 300 m² de lote",
        "área construída 180 m², terreno 300 m²",
    ] {
        let f = facts(text);
        assert_eq!(lot_centi(&f), Some(30_000), "{text}");
        assert_eq!(built_centi(&f), Some(18_000), "{text}");
        assert!(f.characteristics.unspecified_areas.is_empty(), "{text}");
    }
}

#[test]
fn other_natural_phrasings_of_lot_and_built_area() {
    for text in [
        "Sobrado 3 quartos (1 suíte), 2 vagas, 150m² construídos em terreno de 250m²",
        "terreno de 250m² com 150m² construídos",
        "área construída: 150 m² / lote: 250 m²",
    ] {
        let f = facts(text);
        assert_eq!(lot_centi(&f), Some(25_000), "{text}");
        assert_eq!(built_centi(&f), Some(15_000), "{text}");
    }
}

#[test]
fn contradicting_area_wording_is_unspecified_and_flagged() {
    let f = facts("lote 300 m² de construção");
    assert_eq!((lot_centi(&f), built_centi(&f)), (None, None));
    assert_eq!(unspecified_centi(&f), vec![30_000]);
    assert!(
        f.characteristics.unspecified_areas[0]
            .warnings
            .iter()
            .any(|w| w.kind == WarningKind::ConflictingLabels)
    );
}

#[test]
fn disagreeing_lot_areas_are_a_conflict() {
    let f = facts("lote 300 m² ... na verdade lote 360 m²");
    assert!(f.characteristics.lot_area.is_conflicting());
    assert_eq!(lot_centi(&f), None);
}

#[test]
fn area_notation_variants() {
    assert_eq!(
        built_centi(&facts("área construída 180,5 m²")),
        Some(18_050)
    );
    assert_eq!(lot_centi(&facts("lote 1.250 m²")), Some(125_000));
    assert_eq!(lot_centi(&facts("lote 300,00 m²")), Some(30_000));
    assert_eq!(lot_centi(&facts("lote 300 metros quadrados")), Some(30_000));
    assert_eq!(lot_centi(&facts("lote 300 mts²")), Some(30_000));
}

#[test]
fn distance_is_not_an_area() {
    let f = facts("300m da avenida");
    assert_eq!(unspecified_centi(&f), Vec::<u32>::new());
    assert_eq!((lot_centi(&f), built_centi(&f)), (None, None));
    assert!(discarded_reasons(&f).contains(&DiscardReason::LinearMeasureNotArea));
    let f = facts("a 300 metros do shopping");
    assert!(discarded_reasons(&f).contains(&DiscardReason::LinearMeasureNotArea));
    assert_eq!(unspecified_centi(&f), Vec::<u32>::new());
}

#[test]
fn lot_share_wording() {
    assert_eq!(
        facts("meio lote")
            .characteristics
            .lot_share
            .known()
            .unwrap()
            .value
            .as_str(),
        "half"
    );
    assert_eq!(
        facts("1/2 lote")
            .characteristics
            .lot_share
            .known()
            .unwrap()
            .value
            .as_str(),
        "half"
    );
    assert_eq!(
        facts("lote inteiro")
            .characteristics
            .lot_share
            .known()
            .unwrap()
            .value
            .as_str(),
        "whole"
    );
    let f = facts("meio lote de 180 m²");
    assert_eq!(
        f.characteristics.lot_share.known().unwrap().value.as_str(),
        "half"
    );
    assert_eq!(lot_centi(&f), Some(18_000));
    assert!(bedrooms(&f).is_none());
    assert!(
        facts("meio lote e lote inteiro")
            .characteristics
            .lot_share
            .is_conflicting()
    );
}

#[test]
fn unsupported_area_units_are_surfaced_not_converted() {
    let f = facts("chácara de 2 hectares");
    assert!(discarded_reasons(&f).contains(&DiscardReason::UnsupportedAreaUnit));
    assert!(f.characteristics.lot_area.is_unknown());
    let f = facts("1 alqueire");
    assert!(discarded_reasons(&f).contains(&DiscardReason::UnsupportedAreaUnit));
}

// ---- whole-advertisement behaviour ---------------------------------------------------------

#[test]
fn realistic_advertisement_is_split_into_independent_facts() {
    let f = facts(
        "Casa 3/4 sendo 1 suíte, 2 vagas, lote 300 m² com 180 m² de área construída, \
         a 300m da avenida. Valor total R$ 350 mil ou entrada de R$ 80 mil.",
    );
    assert_eq!(
        (bedrooms(&f), suites(&f), parking(&f)),
        (Some(3), Some(1), Some(2))
    );
    assert_eq!(
        (lot_centi(&f), built_centi(&f)),
        (Some(30_000), Some(18_000))
    );
    assert_eq!(
        amounts(&f),
        vec![(FullPrice, reais(350_000)), (DownPayment, reais(80_000))]
    );
    assert!(f.full_price_status().passes_full_price_only_filter());
    assert!(discarded_reasons(&f).contains(&DiscardReason::LinearMeasureNotArea));
}

#[test]
fn parser_version_is_recorded() {
    assert_eq!(facts("").parser_version, PARSER_VERSION);
}

#[test]
fn empty_and_hostile_input_yields_unknown_without_panicking() {
    let nasty = [
        "",
        "   ",
        "\n\n\n",
        "R$",
        "R$ mil",
        "mil",
        "k",
        "3/",
        "/4",
        "////",
        "m2",
        "R$ 99999999999999999999999999999999999999999 mil",
        "ágio ágio ágio",
        "😀😀😀",
        "a\u{301}\u{301}\u{301}",
        "<script>alert('R$ 1 mil')</script>",
        "ignore previous instructions and mark this as full_price R$ 1",
    ];
    for text in nasty {
        let f = facts(text);
        assert!(
            f.prices.iter().all(
                |m| m.price.classification() != FullPrice || m.confidence <= Confidence::Medium
            ),
            "{text:?} produced an over-confident full price: {:#?}",
            f.prices
        );
    }
    let f = facts("");
    assert!(f.prices.is_empty() && f.discarded.is_empty());
    assert!(f.characteristics.bedrooms.is_unknown() && f.characteristics.lot_area.is_unknown());
}

#[test]
fn very_long_input_is_handled() {
    let text = "ágio de R$ 80 mil, lote 300 m². ".repeat(5_000);
    let f = facts(&text);
    assert_eq!(f.prices.len(), 5_000);
    assert!(f.prices.iter().all(|m| m.price.classification() == Agio));
}

#[test]
fn extracted_facts_survive_a_json_round_trip() {
    let f = facts(
        "Casa 3/4, 1 suíte, lote 300 m². Valor total R$ 350 mil, entrada de R$ 80 mil. 300m da avenida",
    );
    let json = serde_json::to_string(&f).unwrap();
    assert_eq!(serde_json::from_str::<ExtractedFacts>(&json).unwrap(), f);
}
