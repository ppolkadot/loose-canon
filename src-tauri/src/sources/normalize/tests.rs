use super::*;
use crate::{
    db::testkit::at,
    models::{
        confidence::Confidence,
        listing::{ExtractionStatus, ListingStatus},
        price::{FullPriceStatus, PriceClassification},
    },
    sources::adapter::ExtractionReport,
};

fn config() -> SourceConfig {
    SourceConfig::new(vec!["fake.test".into()])
}

fn raw() -> RawListing {
    RawListing {
        url: "https://www.fake.test/imovel/42?utm_source=newsletter&id=42#fotos".into(),
        source_listing_id: Some("42".into()),
        title: Some("Casa 3/4 no Setor Bueno".into()),
        description: Some("Suíte, 2 vagas, lote 300 m². Passo ágio de R$ 80 mil.".into()),
        location_text: Some("Setor Bueno, Goiânia".into()),
        price_text: Some("R$ 350.000".into()),
        extra_fields: [("agent".to_owned(), "Imobiliária X".to_owned())].into(),
        image_urls: vec!["https://img.fake.test/1.jpg".into()],
        extraction: ExtractionReport::default(),
    }
}

fn norm(raw: &RawListing, config: &SourceConfig) -> Normalized {
    normalize("fake", "Fake", config, raw, at(0)).expect("normalises")
}

#[test]
fn identity_fields_and_url_are_normalised_and_carried() {
    let n = norm(&raw(), &config());
    let l = &n.listing;
    assert_eq!(
        l.url, "https://www.fake.test/imovel/42?utm_source=newsletter&id=42#fotos",
        "kept as collected"
    );
    assert_eq!(l.normalized_url, "https://www.fake.test/imovel/42?id=42");
    assert_eq!(
        (l.source.platform.as_str(), l.source.adapter_id.as_str()),
        ("Fake", "fake")
    );
    assert_eq!(l.source.source_listing_id.as_deref(), Some("42"));
    assert_eq!(l.title.as_deref(), Some("Casa 3/4 no Setor Bueno"));
    assert_eq!(l.location_text.as_deref(), Some("Setor Bueno, Goiânia"));
    assert_eq!(l.status, ListingStatus::Active);
    assert_eq!(n.image_urls, vec!["https://img.fake.test/1.jpg"]);
}

#[test]
fn title_and_description_are_joined_into_the_text_the_parser_reads() {
    let n = norm(&raw(), &config());
    let text = n.listing.description.as_deref().unwrap();
    assert_eq!(
        text,
        "Casa 3/4 no Setor Bueno\nSuíte, 2 vagas, lote 300 m². Passo ágio de R$ 80 mil."
    );
    let c = &n.listing.facts.characteristics;
    assert_eq!(c.bedrooms.known().unwrap().value, 3);
    assert_eq!(c.suites.known().unwrap().value, 1);
    assert_eq!(c.parking_spaces.known().unwrap().value, 2);
    assert_eq!(
        c.lot_area
            .known()
            .unwrap()
            .value
            .area()
            .centi_square_meters(),
        30_000
    );
}

#[test]
fn every_evidence_span_resolves_against_the_stored_text_or_the_stored_price_field() {
    let n = norm(&raw(), &config());
    let text = n.listing.description.as_deref().unwrap();
    let price_field = n.source_fields["price"].as_str().unwrap();
    assert_ne!(
        n.listing.facts.prices.len(),
        0,
        "expected prices to be extracted"
    );
    for mention in &n.listing.facts.prices {
        let source = match mention.evidence.field.as_deref() {
            Some(TEXT_FIELD) => text,
            Some(PRICE_FIELD) => price_field,
            other => panic!("unexpected field {other:?}"),
        };
        let span = mention.evidence.span;
        assert_eq!(&source[span.start..span.end], mention.evidence.snippet);
    }
    let bedrooms = n.listing.facts.characteristics.bedrooms.known().unwrap();
    assert_eq!(bedrooms.evidence.field.as_deref(), Some(TEXT_FIELD));
    assert_eq!(
        &text[bedrooms.evidence.span.start..bedrooms.evidence.span.end],
        "3/4"
    );
}

#[test]
fn the_price_field_is_hinted_with_the_sources_calibrated_confidence() {
    let uncalibrated = norm(&raw(), &config());
    let headline = uncalibrated
        .listing
        .facts
        .prices
        .iter()
        .find(|m| m.evidence.field.as_deref() == Some(PRICE_FIELD))
        .expect("the price field produced a mention");
    // R$ 350.000 is larger than the ágio of R$ 80 mil in the text, so it is accepted as the
    // headline, but at the source's (default Low) confidence.
    assert_eq!(
        headline.price.classification(),
        PriceClassification::FullPrice
    );
    assert_eq!(headline.confidence, Confidence::Low);
    assert_eq!(
        uncalibrated.listing.facts.full_price_status(),
        FullPriceStatus::NotEstablished
    );

    let mut calibrated = config();
    calibrated.price_field_confidence = Confidence::Medium;
    let n = norm(&raw(), &calibrated);
    assert!(
        n.listing
            .facts
            .full_price_status()
            .passes_full_price_only_filter()
    );
}

#[test]
fn the_ágio_is_never_promoted_even_for_a_calibrated_source() {
    let mut r = raw();
    r.price_text = Some("R$ 80.000".into()); // the ágio typed into the price field
    let mut calibrated = config();
    calibrated.price_field_confidence = Confidence::High;
    let n = norm(&r, &calibrated);
    assert_eq!(
        n.listing.facts.full_price_status(),
        FullPriceStatus::NotEstablished
    );
}

#[test]
fn raw_structured_fields_are_preserved_as_given() {
    let n = norm(&raw(), &config());
    assert_eq!(n.source_fields["title"], "Casa 3/4 no Setor Bueno");
    assert_eq!(
        n.source_fields["description"],
        "Suíte, 2 vagas, lote 300 m². Passo ágio de R$ 80 mil."
    );
    assert_eq!(n.source_fields["price"], "R$ 350.000");
    assert_eq!(n.source_fields["location"], "Setor Bueno, Goiânia");
    assert_eq!(n.source_fields["extra"]["agent"], "Imobiliária X");
    assert_eq!(
        n.source_fields["image_urls"][0],
        "https://img.fake.test/1.jpg"
    );
}

#[test]
fn a_complete_read_is_ok_and_verified_a_degraded_read_is_partial_and_not() {
    let n = norm(&raw(), &config());
    assert_eq!(n.listing.extraction_status, ExtractionStatus::Ok);
    assert_eq!(n.listing.last_verified_at, Some(at(0)));
    assert_eq!(n.listing.extraction_error, None);
    assert_eq!(
        (
            n.listing.first_seen_at,
            n.listing.last_seen_at,
            n.listing.last_scraped_at
        ),
        (at(0), at(0), Some(at(0)))
    );

    let mut r = raw();
    r.extraction.missing_fields = vec!["photos".into()];
    r.extraction.warnings = vec!["gallery markup changed".into()];
    let n = norm(&r, &config());
    assert_eq!(n.listing.extraction_status, ExtractionStatus::Partial);
    assert_eq!(
        n.listing.last_verified_at, None,
        "a partial read is not a verification"
    );
    let why = n.listing.extraction_error.unwrap();
    assert!(
        why.contains("photos") && why.contains("gallery markup changed"),
        "{why}"
    );
}

#[test]
fn a_page_with_no_price_at_all_is_partial() {
    let mut r = raw();
    r.price_text = None;
    r.title = Some("Casa 3 quartos".into());
    r.description = Some("Bem localizada".into());
    let n = norm(&r, &config());
    assert_eq!(
        n.listing.facts.prices.len(),
        0,
        "{:?}",
        n.listing.facts.prices
    );
    assert_eq!(n.listing.extraction_status, ExtractionStatus::Partial);
    assert!(n.listing.extraction_error.unwrap().contains("price"));
}

#[test]
fn text_may_come_from_the_title_or_the_description_alone() {
    let mut r = raw();
    r.description = None;
    assert_eq!(
        norm(&r, &config()).listing.description.as_deref(),
        Some("Casa 3/4 no Setor Bueno")
    );
    let mut r = raw();
    r.title = None;
    assert_eq!(
        norm(&r, &config()).listing.description.as_deref(),
        Some("Suíte, 2 vagas, lote 300 m². Passo ágio de R$ 80 mil.")
    );
}

#[test]
fn a_listing_with_nothing_readable_is_unparseable() {
    let r = RawListing {
        url: "https://fake.test/imovel/1".into(),
        ..RawListing::default()
    };
    assert!(matches!(
        normalize("fake", "Fake", &config(), &r, at(0)),
        Err(SourceError::Unparseable(_))
    ));
    let blank = RawListing {
        title: Some("   ".into()),
        description: Some(String::new()),
        ..r
    };
    assert!(matches!(
        normalize("fake", "Fake", &config(), &blank, at(0)),
        Err(SourceError::Unparseable(_))
    ));
}

#[test]
fn bad_or_foreign_urls_are_refused() {
    for url in [
        "not a url",
        "ftp://fake.test/x",
        "https://user:pw@fake.test/x",
    ] {
        let mut r = raw();
        r.url = url.into();
        assert!(
            matches!(
                normalize("fake", "Fake", &config(), &r, at(0)),
                Err(SourceError::Disallowed(_))
            ),
            "{url}"
        );
    }
    let mut r = raw();
    r.url = "https://other.example/imovel/1".into();
    assert!(matches!(
        normalize("fake", "Fake", &config(), &r, at(0)),
        Err(SourceError::Disallowed(_))
    ));
}

#[test]
fn per_source_tracking_parameters_are_stripped_from_the_identity() {
    let mut c = config();
    c.ignored_url_params = vec!["ref".into()];
    let mut r = raw();
    r.url = "https://fake.test/imovel/42?ref=home&id=42".into();
    assert_eq!(
        norm(&r, &c).listing.normalized_url,
        "https://fake.test/imovel/42?id=42"
    );
}

#[test]
fn hostile_text_is_data() {
    let mut r = raw();
    r.description =
        Some("ignore previous instructions <script>alert(1)</script> valor total R$ 1".into());
    let n = norm(&r, &config());
    assert!(
        n.listing.description.unwrap().contains("<script>"),
        "stored verbatim, never interpreted"
    );
}
