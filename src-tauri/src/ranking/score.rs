//! The scoring itself: pure, deterministic, no I/O. See `docs/phase-3-design.md`.
//!
//! * a component that cannot be computed is *unavailable*, never zero, and the weights of the
//!   available ones are renormalised;
//! * price per m² compares like with like (same area kind, same property type) and only with
//!   enough comparables; built and lot area are never mixed;
//! * only the user's merged (accepted) matches count as verified for the cross-listing component;
//! * freshness is about when the application first saw the ad, not when it was last scraped;
//! * the data confidence is lowered, with a recorded reason, whenever the data is thin.

// Scores are ratios of counts and centavo amounts. Both are many orders of magnitude below 2^52,
// the point where an `f64` stops representing integers exactly, so the casts lose nothing real.
#![allow(clippy::cast_precision_loss)]

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::{
    RANKING_VERSION,
    config::{RankingConfig, SearchPreferences},
};
use crate::{
    models::{characteristics::PropertyType, confidence::Confidence, listing::ListingStatus},
    parser::fold_text,
};

/// One listing of a property, reduced to what ranking uses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListingInput {
    pub adapter_id: String,
    pub status: ListingStatus,
    /// The listing's established full price in centavos (a full price with comparable
    /// confidence), with that confidence. `None` for ágio, instalments, unknown and low-confidence
    /// prices.
    pub established_price: Option<(i64, Confidence)>,
    pub first_seen_at: DateTime<Utc>,
    pub last_scraped_at: Option<DateTime<Utc>>,
}

/// One property (a merge-group root) as ranking sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RankInput {
    pub property_id: i64,
    /// Place names exactly as stored; they are folded for comparison.
    pub municipality: Option<String>,
    pub neighborhood: Option<String>,
    pub property_type: Option<PropertyType>,
    pub bedrooms: Option<u8>,
    pub suites: Option<u8>,
    pub parking_spaces: Option<u8>,
    pub built_area_centi_m2: Option<u32>,
    pub lot_area_centi_m2: Option<u32>,
    /// Every listing of the whole merge group.
    pub listings: Vec<ListingInput>,
    /// Canonical values on which the group's listings contradict each other.
    pub conflicting_fields: Vec<String>,
    /// The group contains a match the user accepted (a merge).
    pub verified_merge: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ComponentKind {
    PricePerM2,
    CrossListing,
    Fit,
    Freshness,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UnavailableReason {
    NoComparablePrice,
    AreaUnknown,
    TooFewComparables,
    /// No accepted merge links several listings of this property.
    NotVerified,
    SingleSource,
    NoPreferences,
    ValueUnknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfidenceReason {
    NoPrice,
    NoArea,
    LowComponentCoverage,
    PartialComponentCoverage,
    FewComparables,
    BroadSegment,
    PropertyTypeUnknown,
    ConflictingValues,
    PriceConfidenceMedium,
    StaleData,
    FitValuesUnknown,
}

impl ConfidenceReason {
    /// The highest confidence the data can have while this reason applies.
    #[must_use]
    pub const fn cap(self) -> Confidence {
        match self {
            Self::NoPrice | Self::NoArea | Self::LowComponentCoverage => Confidence::Low,
            _ => Confidence::Medium,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Component {
    pub kind: ComponentKind,
    /// The configured weight (before renormalisation).
    pub weight: f64,
    /// 0-100, or `None` when the component is unavailable.
    pub score: Option<f64>,
    pub unavailable_reason: Option<UnavailableReason>,
    /// What the score rests on (values, comparables, segment). A stored document.
    pub evidence: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScoreDetails {
    pub ranking_version: String,
    pub comparable_price_centavos: Option<i64>,
    /// Always the four components, available or not.
    pub components: Vec<Component>,
    /// Share (0-1) of the total weight that was available.
    pub available_weight_share: f64,
    pub confidence_reasons: Vec<ConfidenceReason>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Ranked {
    pub property_id: i64,
    /// 0-100 with one decimal, or `None` when no component could be computed.
    pub score: Option<f64>,
    pub confidence: Confidence,
    pub details: ScoreDetails,
}

/// Scores every property that has at least one active listing. The result is ordered by score
/// (highest first, unscored last), then by property id.
#[must_use]
pub fn rank(
    inputs: &[RankInput],
    preferences: &SearchPreferences,
    config: &RankingConfig,
    now: DateTime<Utc>,
) -> Vec<Ranked> {
    let market: Vec<MarketEntry<'_>> = inputs
        .iter()
        .filter(|p| active(p).next().is_some())
        .map(MarketEntry::new)
        .collect();
    let mut ranked: Vec<Ranked> = market
        .iter()
        .map(|entry| score_one(entry, &market, preferences, config, now))
        .collect();
    ranked.sort_by(|a, b| {
        let by_score = match (a.score, b.score) {
            (Some(x), Some(y)) => y.total_cmp(&x),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => std::cmp::Ordering::Equal,
        };
        by_score.then(a.property_id.cmp(&b.property_id))
    });
    ranked
}

fn active(p: &RankInput) -> impl Iterator<Item = &ListingInput> {
    p.listings
        .iter()
        .filter(|l| l.status == ListingStatus::Active)
}

/// Median of the established prices of the active listings (the average of the two middle ones
/// for an even count), so one outlier listing cannot pick the price.
fn comparable_price(p: &RankInput) -> Option<i64> {
    let mut prices: Vec<i64> = active(p)
        .filter_map(|l| l.established_price.map(|(price, _)| price))
        .collect();
    prices.sort_unstable();
    match prices.len() {
        0 => None,
        n if n % 2 == 1 => Some(prices[n / 2]),
        n => Some(prices[n / 2 - 1].midpoint(prices[n / 2])),
    }
}

/// One decimal: the score is a heuristic and must not suggest more precision than it has (this also
/// keeps stored documents stable across a JSON round trip).
fn round1(x: f64) -> f64 {
    (x * 10.0).round() / 10.0
}

fn folded(name: Option<&str>) -> Option<String> {
    name.map(fold_text).filter(|n| !n.trim().is_empty())
}

/// A property prepared for comparison: folded place names and price per m² for each area kind.
struct MarketEntry<'a> {
    input: &'a RankInput,
    municipality: Option<String>,
    neighborhood: Option<String>,
    comparable_price: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AreaKind {
    Built,
    Lot,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SegmentLevel {
    Neighborhood,
    Municipality,
}

impl SegmentLevel {
    const fn label(self) -> &'static str {
        match self {
            Self::Neighborhood => "neighborhood",
            Self::Municipality => "municipality",
        }
    }
}

impl<'a> MarketEntry<'a> {
    fn new(input: &'a RankInput) -> Self {
        Self {
            input,
            municipality: folded(input.municipality.as_deref()),
            neighborhood: folded(input.neighborhood.as_deref()),
            comparable_price: comparable_price(input),
        }
    }

    fn area(&self, kind: AreaKind) -> Option<u32> {
        match kind {
            AreaKind::Built => self.input.built_area_centi_m2,
            AreaKind::Lot => self.input.lot_area_centi_m2,
        }
    }

    /// Price per m² in centavos, when both the price and this area kind are known.
    fn price_per_m2(&self, kind: AreaKind) -> Option<f64> {
        let (price, area) = (self.comparable_price?, self.area(kind)?);
        Some(price as f64 * 100.0 / f64::from(area))
    }

    fn comparable_at(&self, other: &Self, level: SegmentLevel) -> bool {
        if self.input.property_type != other.input.property_type {
            return false;
        }
        match level {
            SegmentLevel::Neighborhood => {
                self.municipality.is_some()
                    && self.neighborhood.is_some()
                    && self.municipality == other.municipality
                    && self.neighborhood == other.neighborhood
            }
            SegmentLevel::Municipality => {
                self.municipality.is_some() && self.municipality == other.municipality
            }
        }
    }
}

/// How one area kind scored.
enum SubScore {
    Unavailable {
        reason: UnavailableReason,
        comparables: usize,
    },
    Scored {
        price_per_m2: f64,
        comparables: usize,
        level: SegmentLevel,
        score: f64,
    },
}

impl SubScore {
    fn evidence(&self) -> serde_json::Value {
        match self {
            Self::Unavailable {
                reason,
                comparables,
            } => json!({
                "unavailable": reason,
                "comparables": comparables,
            }),
            Self::Scored {
                price_per_m2,
                comparables,
                level,
                score,
            } => json!({
                "price_per_m2_centavos": price_per_m2.round(),
                "comparables": comparables,
                "segment": level.label(),
                "score": round1(*score),
            }),
        }
    }
}

/// Position of the subject's price per m² among its comparables: the cheaper, the higher.
fn price_sub_score(
    kind: AreaKind,
    subject: &MarketEntry<'_>,
    market: &[MarketEntry<'_>],
    config: &RankingConfig,
) -> SubScore {
    let Some(price_per_m2) = subject.price_per_m2(kind) else {
        let reason = if subject.comparable_price.is_none() {
            UnavailableReason::NoComparablePrice
        } else {
            UnavailableReason::AreaUnknown
        };
        return SubScore::Unavailable {
            reason,
            comparables: 0,
        };
    };
    let values_at = |level: SegmentLevel| -> Vec<f64> {
        market
            .iter()
            .filter(|other| subject.comparable_at(other, level))
            .filter_map(|other| other.price_per_m2(kind))
            .collect()
    };
    let neighborhood = values_at(SegmentLevel::Neighborhood);
    let municipality = values_at(SegmentLevel::Municipality);
    let (level, values) = if neighborhood.len() >= config.min_comparables {
        (SegmentLevel::Neighborhood, neighborhood)
    } else if municipality.len() >= config.min_comparables {
        (SegmentLevel::Municipality, municipality)
    } else {
        return SubScore::Unavailable {
            reason: UnavailableReason::TooFewComparables,
            comparables: neighborhood.len().max(municipality.len()),
        };
    };
    let ordering = |v: &f64| v.total_cmp(&price_per_m2);
    let below = values.iter().filter(|v| ordering(v).is_lt()).count();
    let equal = values.iter().filter(|v| ordering(v).is_eq()).count();
    let position = (below as f64 + 0.5 * equal as f64) / values.len() as f64;
    SubScore::Scored {
        price_per_m2,
        comparables: values.len(),
        level,
        score: 100.0 * (1.0 - position),
    }
}

fn unavailable(
    kind: ComponentKind,
    weight: f64,
    reason: UnavailableReason,
    evidence: serde_json::Value,
) -> Component {
    Component {
        kind,
        weight,
        score: None,
        unavailable_reason: Some(reason),
        evidence,
    }
}

fn available(
    kind: ComponentKind,
    weight: f64,
    score: f64,
    evidence: serde_json::Value,
) -> Component {
    Component {
        kind,
        weight,
        score: Some(round1(score)),
        unavailable_reason: None,
        evidence,
    }
}

fn price_component(
    subject: &MarketEntry<'_>,
    market: &[MarketEntry<'_>],
    config: &RankingConfig,
) -> (Component, Vec<SubScore>) {
    let subs =
        [AreaKind::Built, AreaKind::Lot].map(|kind| price_sub_score(kind, subject, market, config));
    let evidence = json!({
        "comparable_price_centavos": subject.comparable_price,
        "built": subs[0].evidence(),
        "lot": subs[1].evidence(),
    });
    let scores: Vec<f64> = subs
        .iter()
        .filter_map(|s| match s {
            SubScore::Scored { score, .. } => Some(*score),
            SubScore::Unavailable { .. } => None,
        })
        .collect();
    let weight = config.weight_price_per_m2;
    let component = if scores.is_empty() {
        let reason = if subject.comparable_price.is_none() {
            UnavailableReason::NoComparablePrice
        } else if subject.input.built_area_centi_m2.is_none()
            && subject.input.lot_area_centi_m2.is_none()
        {
            UnavailableReason::AreaUnknown
        } else {
            UnavailableReason::TooFewComparables
        };
        unavailable(ComponentKind::PricePerM2, weight, reason, evidence)
    } else {
        let mean = scores.iter().sum::<f64>() / scores.len() as f64;
        available(ComponentKind::PricePerM2, weight, mean, evidence)
    };
    (component, subs.into())
}

fn cross_listing_component(p: &RankInput, config: &RankingConfig) -> Component {
    let weight = config.weight_cross_listing;
    let prices: Vec<(&str, i64)> = active(p)
        .filter_map(|l| {
            l.established_price
                .map(|(price, _)| (l.adapter_id.as_str(), price))
        })
        .collect();
    let sources: std::collections::BTreeSet<&str> =
        prices.iter().map(|(adapter, _)| *adapter).collect();
    if !p.verified_merge {
        return unavailable(
            ComponentKind::CrossListing,
            weight,
            UnavailableReason::NotVerified,
            json!({}),
        );
    }
    if sources.len() < 2 {
        return unavailable(
            ComponentKind::CrossListing,
            weight,
            UnavailableReason::SingleSource,
            json!({}),
        );
    }
    let low = prices.iter().map(|(_, price)| *price).min().unwrap_or(0);
    let high = prices.iter().map(|(_, price)| *price).max().unwrap_or(0);
    let spread_percent = if high > 0 {
        (high - low) as f64 * 100.0 / high as f64
    } else {
        0.0
    };
    let score = (spread_percent / config.cross_listing_cap_percent).min(1.0) * 100.0;
    let mut listed: Vec<serde_json::Value> = prices
        .iter()
        .map(|(adapter, price)| json!({ "adapter_id": adapter, "price_centavos": price }))
        .collect();
    listed.sort_by_key(ToString::to_string);
    available(
        ComponentKind::CrossListing,
        weight,
        score,
        json!({ "prices": listed, "spread_percent": round1(spread_percent), "cap_percent": config.cross_listing_cap_percent }),
    )
}

/// One stated criterion: how well `value` meets `min`/`max`. More than wanted is not rewarded.
fn criterion_score(value: u8, min: Option<u8>, max: Option<u8>) -> f64 {
    let value = f64::from(value);
    if let Some(min) = min.filter(|m| f64::from(*m) > value) {
        let min = f64::from(min);
        return (1.0 - (min - value) / min.max(1.0)).max(0.0);
    }
    if let Some(max) = max.filter(|m| f64::from(*m) < value) {
        let max = f64::from(max);
        return (1.0 - (value - max) / max.max(1.0)).max(0.0);
    }
    1.0
}

/// Returns the component and whether a stated criterion had an unknown value.
/// A stated criterion: its name, the property's value, and the minimum and maximum the user gave.
type Criterion<'a> = (&'a str, Option<u8>, Option<u8>, Option<u8>);

fn fit_component(
    p: &RankInput,
    preferences: &SearchPreferences,
    config: &RankingConfig,
) -> (Component, bool) {
    let weight = config.weight_fit;
    if preferences.is_empty() {
        return (
            unavailable(
                ComponentKind::Fit,
                weight,
                UnavailableReason::NoPreferences,
                json!({}),
            ),
            false,
        );
    }
    let stated: [Criterion; 3] = [
        (
            "bedrooms",
            p.bedrooms,
            preferences.bedrooms_min,
            preferences.bedrooms_max,
        ),
        ("suites", p.suites, preferences.suites_min, None),
        (
            "parking_spaces",
            p.parking_spaces,
            preferences.parking_min,
            None,
        ),
    ];
    let mut criteria = Vec::new();
    let mut scores = Vec::new();
    let mut unknown = false;
    for (name, value, min, max) in stated {
        if min.is_none() && max.is_none() {
            continue;
        }
        if let Some(v) = value {
            let score = 100.0 * criterion_score(v, min, max);
            scores.push(score);
            criteria.push(json!({ "name": name, "value": v, "score": round1(score) }));
        } else {
            unknown = true;
            criteria.push(json!({ "name": name, "unavailable": UnavailableReason::ValueUnknown }));
        }
    }
    let evidence = json!({ "criteria": criteria });
    if scores.is_empty() {
        return (
            unavailable(
                ComponentKind::Fit,
                weight,
                UnavailableReason::ValueUnknown,
                evidence,
            ),
            unknown,
        );
    }
    let mean = scores.iter().sum::<f64>() / scores.len() as f64;
    (
        available(ComponentKind::Fit, weight, mean, evidence),
        unknown,
    )
}

fn freshness_component(p: &RankInput, config: &RankingConfig, now: DateTime<Utc>) -> Component {
    let weight = config.weight_freshness;
    let first_seen = active(p).map(|l| l.first_seen_at).min();
    let last_scraped = active(p).filter_map(|l| l.last_scraped_at).max();
    let Some(first_seen) = first_seen else {
        return unavailable(
            ComponentKind::Freshness,
            weight,
            UnavailableReason::ValueUnknown,
            json!({}),
        );
    };
    let age_days = ((now - first_seen).num_seconds() as f64 / 86_400.0).max(0.0);
    let score = 100.0 * (1.0 - age_days / f64::from(config.freshness_max_age_days)).max(0.0);
    available(
        ComponentKind::Freshness,
        weight,
        score,
        json!({
            "first_seen_at": first_seen.to_rfc3339(),
            "age_days": round1(age_days),
            "last_scraped_at": last_scraped.map(|t| t.to_rfc3339()),
        }),
    )
}

fn confidence_reasons(
    p: &RankInput,
    subs: &[SubScore],
    fit_unknown: bool,
    share: f64,
    config: &RankingConfig,
    now: DateTime<Utc>,
) -> Vec<ConfidenceReason> {
    let mut reasons = Vec::new();
    let comparable = comparable_price(p);
    if comparable.is_none() {
        reasons.push(ConfidenceReason::NoPrice);
    }
    if p.built_area_centi_m2.is_none() && p.lot_area_centi_m2.is_none() {
        reasons.push(ConfidenceReason::NoArea);
    }
    if share + 1e-9 < 0.5 {
        reasons.push(ConfidenceReason::LowComponentCoverage);
    } else if share + 1e-9 < 0.75 {
        reasons.push(ConfidenceReason::PartialComponentCoverage);
    }
    let scored = |s: &&SubScore| matches!(s, SubScore::Scored { .. });
    if subs.iter().filter(scored).any(|s| matches!(s, SubScore::Scored { comparables, .. } if *comparables < config.solid_comparables)) {
        reasons.push(ConfidenceReason::FewComparables);
    }
    if subs.iter().filter(scored).any(|s| {
        matches!(
            s,
            SubScore::Scored {
                level: SegmentLevel::Municipality,
                ..
            }
        )
    }) {
        reasons.push(ConfidenceReason::BroadSegment);
    }
    if p.property_type.is_none() {
        reasons.push(ConfidenceReason::PropertyTypeUnknown);
    }
    if !p.conflicting_fields.is_empty() {
        reasons.push(ConfidenceReason::ConflictingValues);
    }
    if active(p)
        .filter_map(|l| l.established_price)
        .any(|(_, confidence)| confidence < Confidence::High)
    {
        reasons.push(ConfidenceReason::PriceConfidenceMedium);
    }
    let scraped = active(p).filter_map(|l| l.last_scraped_at).max();
    let stale_after = chrono::Duration::days(i64::from(config.stale_scrape_days));
    if scraped.is_none_or(|t| now - t > stale_after) {
        reasons.push(ConfidenceReason::StaleData);
    }
    if fit_unknown {
        reasons.push(ConfidenceReason::FitValuesUnknown);
    }
    reasons
}

fn score_one(
    entry: &MarketEntry<'_>,
    market: &[MarketEntry<'_>],
    preferences: &SearchPreferences,
    config: &RankingConfig,
    now: DateTime<Utc>,
) -> Ranked {
    let p = entry.input;
    let (price, subs) = price_component(entry, market, config);
    let (fit, fit_unknown) = fit_component(p, preferences, config);
    let components = vec![
        price,
        cross_listing_component(p, config),
        fit,
        freshness_component(p, config, now),
    ];

    let available_weight: f64 = components
        .iter()
        .filter(|c| c.score.is_some())
        .map(|c| c.weight)
        .sum();
    let share = available_weight / config.total_weight();
    let score = (available_weight > 0.0).then(|| {
        let weighted: f64 = components
            .iter()
            .filter_map(|c| c.score.map(|s| s * c.weight))
            .sum();
        round1(weighted / available_weight)
    });

    let reasons = confidence_reasons(p, &subs, fit_unknown, share, config, now);
    let confidence = reasons
        .iter()
        .map(|r| r.cap())
        .min()
        .unwrap_or(Confidence::High);
    Ranked {
        property_id: p.property_id,
        score,
        confidence,
        details: ScoreDetails {
            ranking_version: RANKING_VERSION.to_owned(),
            comparable_price_centavos: entry.comparable_price,
            components,
            available_weight_share: (share * 1000.0).round() / 1000.0,
            confidence_reasons: reasons,
        },
    }
}

#[cfg(test)]
mod tests;
