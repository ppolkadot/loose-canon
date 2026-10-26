use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::{
    characteristics::PropertyCharacteristics,
    evidence::DiscardedMention,
    money::Currency,
    price::{
        ClassifiedPrice, FullPriceStatus, PriceClassification, PriceMention, assess_full_price,
    },
};

string_enum! {
    /// Whether an advertisement is still believed to be live.
    pub enum ListingStatus {
        Active => "active",
        /// The ad disappeared or was reported sold. The record is kept for history.
        Unavailable => "unavailable",
        Unknown => "unknown",
    }
}

string_enum! {
    /// Outcome of extracting facts from a listing's text.
    pub enum ExtractionStatus {
        Pending => "pending",
        Ok => "ok",
        /// Some fields extracted, others missing or unreliable.
        Partial => "partial",
        Failed => "failed",
    }
}

string_enum! {
    /// User decision about a consolidated property. Hiding is sticky across re-scrapes.
    pub enum UserStatus {
        New => "new",
        Favorite => "favorite",
        Hidden => "hidden",
    }
}

/// Everything the text parser extracted from one advertisement. Serialised into
/// `listing_links.extracted_facts_json` so the evidence is preserved next to the listing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExtractedFacts {
    /// Extractor name and version that produced these facts (`br_real_estate/1`).
    pub parser_version: String,
    /// Every amount found, each separately classified. Never collapsed into one price.
    pub prices: Vec<PriceMention>,
    pub characteristics: PropertyCharacteristics,
    /// Numbers that looked like facts but were deliberately not extracted.
    pub discarded: Vec<DiscardedMention>,
}

impl ExtractedFacts {
    /// Whether these facts establish a comparable full price.
    #[must_use]
    pub fn full_price_status(&self) -> FullPriceStatus {
        assess_full_price(&self.prices)
    }
}

/// Identifies where an advertisement came from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceRef {
    /// Human-facing platform name, e.g. `OLX`.
    pub platform: String,
    /// Stable identifier of the adapter that collected it.
    pub adapter_id: String,
    /// The platform's own listing identifier, when it exposes one.
    pub source_listing_id: Option<String>,
}

/// One advertisement after collection and parsing, ready to be stored as a `listing_links`
/// row. It is a single ad, never a consolidated real-world property.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NormalizedListing {
    pub source: SourceRef,
    /// URL exactly as collected.
    pub url: String,
    /// URL with tracking noise removed, used to recognise the same ad when it is seen again.
    pub normalized_url: String,
    pub title: Option<String>,
    pub description: Option<String>,
    /// Location text as the source wrote it (neighbourhood, municipality...), unresolved.
    pub location_text: Option<String>,
    pub currency: Currency,
    pub facts: ExtractedFacts,
    pub status: ListingStatus,
    pub extraction_status: ExtractionStatus,
    pub extraction_error: Option<String>,
    pub first_seen_at: DateTime<Utc>,
    pub last_seen_at: DateTime<Utc>,
    /// Last time the ad was confirmed to still exist and parse correctly.
    pub last_verified_at: Option<DateTime<Utc>>,
    /// Last time the page was fetched at all (a fetch is not a verification).
    pub last_scraped_at: Option<DateTime<Utc>>,
}

impl NormalizedListing {
    /// The price to show as this ad's headline, with its classification attached so a
    /// non-full price can never be mistaken for one.
    ///
    /// Precedence:
    /// 1. an established full price;
    /// 2. if established full prices **disagree**, nothing (no silent winner);
    /// 3. otherwise the most confident full-price *candidate* (low confidence, flagged);
    /// 4. otherwise the most confident other amount (ágio, down payment...), earliest wins ties.
    #[must_use]
    pub fn headline_price(&self) -> Option<&PriceMention> {
        let prices = &self.facts.prices;
        let is_full =
            |m: &&PriceMention| m.price.classification() == PriceClassification::FullPrice;
        match self.facts.full_price_status() {
            FullPriceStatus::Established { price, .. } => most_confident(
                prices
                    .iter()
                    .filter(|m| matches!(m.price, ClassifiedPrice::FullPrice(p) if p == price)),
            ),
            FullPriceStatus::Conflicting { .. } => None,
            FullPriceStatus::NotEstablished => most_confident(prices.iter().filter(is_full))
                .or_else(|| most_confident(prices.iter())),
        }
    }
}

/// Highest confidence wins; the earliest mention wins ties.
fn most_confident<'a>(
    candidates: impl Iterator<Item = &'a PriceMention>,
) -> Option<&'a PriceMention> {
    let mut best: Option<&PriceMention> = None;
    for candidate in candidates {
        if best.is_none_or(|b| candidate.confidence > b.confidence) {
            best = Some(candidate);
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;
    use crate::{
        models::{confidence::Confidence, price::PriceClassification},
        parser::parse_listing_text,
    };

    fn listing(text: &str) -> NormalizedListing {
        let now = Utc.timestamp_opt(1_780_000_000, 0).unwrap();
        NormalizedListing {
            source: SourceRef {
                platform: "Test".into(),
                adapter_id: "test".into(),
                source_listing_id: None,
            },
            url: "https://example.test/1".into(),
            normalized_url: "https://example.test/1".into(),
            title: None,
            description: Some(text.into()),
            location_text: None,
            currency: Currency::Brl,
            facts: parse_listing_text(text),
            status: ListingStatus::Active,
            extraction_status: ExtractionStatus::Ok,
            extraction_error: None,
            first_seen_at: now,
            last_seen_at: now,
            last_verified_at: None,
            last_scraped_at: None,
        }
    }

    fn headline(text: &str) -> Option<(PriceClassification, i64, Confidence)> {
        listing(text).headline_price().map(|m| {
            (
                m.price.classification(),
                m.price.money().centavos() / 100,
                m.confidence,
            )
        })
    }

    #[test]
    fn an_established_full_price_beats_an_earlier_agio() {
        assert_eq!(
            headline("Passo ágio de R$ 80 mil. Valor total R$ 350 mil"),
            Some((PriceClassification::FullPrice, 350_000, Confidence::High))
        );
    }

    #[test]
    fn disagreeing_full_prices_have_no_headline() {
        assert_eq!(headline("valor total R$ 350 mil. Preço R$ 330 mil"), None);
    }

    #[test]
    fn an_unresolved_full_price_candidate_is_preferred_over_a_down_payment_but_stays_low_confidence()
     {
        let l = listing("R$ 350 mil ou entrada de R$ 80 mil");
        assert!(!l.facts.full_price_status().passes_full_price_only_filter());
        assert_eq!(
            headline("R$ 350 mil ou entrada de R$ 80 mil"),
            Some((PriceClassification::FullPrice, 350_000, Confidence::Low))
        );
    }

    #[test]
    fn without_any_full_price_the_headline_keeps_its_real_classification() {
        assert_eq!(
            headline("Passo ágio de R$ 80 mil"),
            Some((PriceClassification::Agio, 80_000, Confidence::High))
        );
    }

    #[test]
    fn equally_confident_non_full_mentions_keep_text_order() {
        assert_eq!(
            headline("ágio de R$ 80 mil, parcelas de R$ 2.500"),
            Some((PriceClassification::Agio, 80_000, Confidence::High))
        );
    }

    #[test]
    fn no_amounts_means_no_headline() {
        assert_eq!(headline("Casa linda, 3 quartos"), None);
    }
}
