//! Comparing two listings: the evidence for and against them being the same property, and the
//! resulting category. Pure and deterministic: integer arithmetic only, no I/O, no clock.

use std::{
    collections::{BTreeSet, HashSet},
    hash::BuildHasher,
};

use serde::{Deserialize, Serialize};

use super::RULE_VERSION;
use crate::{
    error::{Error, Result},
    images::phash::hamming_distance,
    models::{
        characteristics::PropertyType,
        matching::{
            Conflict, ConflictKind, ImagePair, MatchCategory, MatchEvidence, Severity, Signal,
            SignalKind,
        },
        money::Money,
    },
};

/// Experimental thresholds. None of these is a proven boundary; they are starting points to be
/// revised against labelled real data (see `matching::evaluation`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MatchConfig {
    /// Largest perceptual-hash Hamming distance at which two images are paired.
    pub image_distance: u32,
    /// Distinct image pairs needed for image evidence to count as strong.
    pub strong_image_pairs: usize,
    /// An image hash seen on this many different listings (a placeholder, a logo) is ignored.
    pub common_hash_listings: usize,
    /// Areas of the same kind within this many parts per thousand match.
    pub area_match_permille: u32,
    /// Areas of the same kind further apart than this are a hard conflict.
    pub area_conflict_permille: u32,
    pub price_close_permille: u32,
    pub price_far_permille: u32,
    /// Both texts need at least this many distinct terms to be compared.
    pub text_min_tokens: usize,
    pub text_min_jaccard_percent: u32,
    /// Metadata blocks larger than this are not expanded into pairs.
    pub max_block_size: usize,
}

impl Default for MatchConfig {
    fn default() -> Self {
        Self {
            image_distance: 5,
            strong_image_pairs: 2,
            common_hash_listings: 4,
            area_match_permille: 20,
            area_conflict_permille: 100,
            price_close_permille: 50,
            price_far_permille: 300,
            text_min_tokens: 8,
            text_min_jaccard_percent: 60,
            max_block_size: 200,
        }
    }
}

impl MatchConfig {
    /// Rejects values that would make the rule contradict itself.
    pub fn validate(&self) -> Result<()> {
        let problem = if self.image_distance > crate::images::index::MAX_SUPPORTED_DISTANCE {
            Some("image_distance is above what the image index supports")
        } else if self.strong_image_pairs == 0 {
            Some("strong_image_pairs must be at least 1")
        } else if self.common_hash_listings < 2 {
            Some("common_hash_listings must be at least 2")
        } else if self.area_match_permille >= self.area_conflict_permille {
            Some("area_match_permille must be below area_conflict_permille")
        } else if self.price_close_permille >= self.price_far_permille {
            Some("price_close_permille must be below price_far_permille")
        } else if self.text_min_jaccard_percent == 0 || self.text_min_jaccard_percent > 100 {
            Some("text_min_jaccard_percent must be within 1..=100")
        } else if self.text_min_tokens == 0 {
            Some("text_min_tokens must be at least 1")
        } else if self.max_block_size < 2 {
            Some("max_block_size must be at least 2")
        } else {
            None
        };
        problem.map_or(Ok(()), |detail| {
            Err(Error::InvalidConfig {
                what: "matching configuration",
                detail: detail.to_owned(),
            })
        })
    }
}

/// An image hash of one listing, with the image it belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageHash {
    pub image_id: i64,
    pub hash: u64,
}

/// What the comparison needs to know about one listing. Text is folded (lower case, no accents)
/// and `tokens` holds only distinctive words, never digits (so phone numbers and prices cannot
/// leak into evidence).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MatchInput {
    pub listing_id: i64,
    pub adapter_id: String,
    /// Root of the merge group the listing's property currently belongs to. Left out of stored
    /// evaluation sets, where every listing is its own group.
    #[serde(default)]
    pub group_root: i64,
    pub municipality: Option<String>,
    pub neighborhood: Option<String>,
    pub property_type: Option<PropertyType>,
    pub bedrooms: Option<u8>,
    pub suites: Option<u8>,
    pub parking_spaces: Option<u8>,
    pub lot_area_centi_m2: Option<u32>,
    pub built_area_centi_m2: Option<u32>,
    pub usable_area_centi_m2: Option<u32>,
    /// The listing's established full price (comparable classification and confidence), if any.
    pub full_price_centavos: Option<i64>,
    pub tokens: BTreeSet<String>,
    pub images: Vec<ImageHash>,
    /// Characteristics the listing states with conflicting values (so they are unknown here, but
    /// for a different reason than "not stated"), by label: `bedrooms`, `lot area`...
    pub contradictory: Vec<String>,
}

impl MatchInput {
    /// A listing about which nothing is known yet.
    #[must_use]
    pub fn new(listing_id: i64, adapter_id: &str) -> Self {
        Self {
            listing_id,
            adapter_id: adapter_id.to_owned(),
            group_root: listing_id,
            municipality: None,
            neighborhood: None,
            property_type: None,
            bedrooms: None,
            suites: None,
            parking_spaces: None,
            lot_area_centi_m2: None,
            built_area_centi_m2: None,
            usable_area_centi_m2: None,
            full_price_centavos: None,
            tokens: BTreeSet::new(),
            images: Vec::new(),
            contradictory: Vec::new(),
        }
    }

    /// What was compared, for storage next to a decision. No free text, no words, no ids.
    #[must_use]
    pub fn summary(&self) -> serde_json::Value {
        serde_json::json!({
            "adapter": self.adapter_id,
            "municipality": self.municipality,
            "neighborhood": self.neighborhood,
            "property_type": self.property_type.map(PropertyType::as_str),
            "bedrooms": self.bedrooms,
            "suites": self.suites,
            "parking_spaces": self.parking_spaces,
            "lot_area_centi_m2": self.lot_area_centi_m2,
            "built_area_centi_m2": self.built_area_centi_m2,
            "usable_area_centi_m2": self.usable_area_centi_m2,
            "full_price_centavos": self.full_price_centavos,
            "images": self.images.len(),
            "description_terms": self.tokens.len(),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Comparison {
    pub category: MatchCategory,
    pub evidence: MatchEvidence,
    /// Smallest distance among the paired images, when there are any.
    pub min_image_distance: Option<u32>,
}

/// Compares two listings. `a` and `b` are reported in the order given. `common_hashes` are image
/// hashes shared by so many listings that they carry no information.
#[must_use]
pub fn compare<S: BuildHasher>(
    a: &MatchInput,
    b: &MatchInput,
    config: &MatchConfig,
    common_hashes: &HashSet<u64, S>,
) -> Comparison {
    debug_assert_ne!(
        a.listing_id, b.listing_id,
        "a listing is never compared with itself"
    );
    let mut findings = Findings::default();
    compare_location(a, b, &mut findings);
    compare_type(a, b, &mut findings);
    compare_areas(a, b, config, &mut findings);
    compare_rooms(a, b, &mut findings);
    compare_price(a, b, config, &mut findings);
    compare_text(a, b, config, &mut findings);
    for (label, side) in [("listing A", a), ("listing B", b)] {
        for field in &side.contradictory {
            findings
                .notes
                .push(format!("{label} states conflicting values for {field}"));
        }
    }
    let image_pairs = pair_images(a, b, config, common_hashes, &mut findings);
    let min_image_distance = image_pairs.iter().map(|p| p.distance).min();
    if let Some(closest) = min_image_distance {
        findings.signal(
            SignalKind::NearDuplicateImages,
            format!(
                "{} image pair(s), closest distance {closest}",
                image_pairs.len()
            ),
        );
    }

    let evidence = MatchEvidence {
        rule_version: RULE_VERSION.to_owned(),
        signals: findings.signals,
        conflicts: findings.conflicts,
        unknown: findings.unknown,
        image_pairs,
        notes: findings.notes,
    };
    let category = categorize(a, b, &evidence, config);
    Comparison {
        category,
        evidence,
        min_image_distance,
    }
}

#[derive(Default)]
struct Findings {
    signals: Vec<Signal>,
    conflicts: Vec<Conflict>,
    unknown: Vec<String>,
    notes: Vec<String>,
}

impl Findings {
    fn signal(&mut self, kind: SignalKind, detail: String) {
        self.signals.push(Signal { kind, detail });
    }

    fn conflict(&mut self, kind: ConflictKind, severity: Severity, detail: String) {
        self.conflicts.push(Conflict {
            kind,
            severity,
            detail,
        });
    }

    fn has_signal(&self, kind: SignalKind) -> bool {
        self.signals.iter().any(|s| s.kind == kind)
    }

    fn has_conflict(&self, kind: ConflictKind) -> bool {
        self.conflicts.iter().any(|c| c.kind == kind)
    }

    /// Records that `what` could not be compared because a side does not state it.
    fn unknown_in(&mut self, what: &str, a_known: bool, b_known: bool) {
        let side = match (a_known, b_known) {
            (false, false) => "both listings",
            (false, true) => "listing A",
            (true, false) => "listing B",
            (true, true) => return,
        };
        self.unknown.push(format!("{what} unknown in {side}"));
    }
}

fn reais(centavos: i64) -> String {
    Money::from_centavos(centavos).map_or_else(|_| centavos.to_string(), |m| m.to_string())
}

fn compare_location(a: &MatchInput, b: &MatchInput, found: &mut Findings) {
    match (&a.municipality, &b.municipality) {
        (Some(x), Some(y)) if x == y => found.signal(SignalKind::SameMunicipality, x.clone()),
        (Some(x), Some(y)) => found.conflict(
            ConflictKind::Municipality,
            Severity::Hard,
            format!("{x} vs {y}"),
        ),
        (x, y) => found.unknown_in("municipality", x.is_some(), y.is_some()),
    }
    let same_municipality = found.has_signal(SignalKind::SameMunicipality);
    let different_municipality = found.has_conflict(ConflictKind::Municipality);
    match (&a.neighborhood, &b.neighborhood) {
        (Some(x), Some(y)) if x == y => {
            if same_municipality {
                found.signal(SignalKind::SameNeighborhood, x.clone());
            } else if !different_municipality {
                // "Centro" exists in every city: a name alone says nothing.
                found.notes.push(format!(
                    "neighbourhood names match ({x}) but the municipality is unknown, so it is not used"
                ));
            }
        }
        (Some(x), Some(y)) => {
            if !different_municipality {
                found.conflict(
                    ConflictKind::Neighborhood,
                    Severity::Soft,
                    format!("{x} vs {y}"),
                );
            }
        }
        (x, y) => found.unknown_in("neighbourhood", x.is_some(), y.is_some()),
    }
}

/// `Other` is a catch-all label, not a kind of property to compare.
fn comparable_type(t: Option<PropertyType>) -> Option<PropertyType> {
    t.filter(|t| *t != PropertyType::Other)
}

/// Casa, casa geminada and sobrado are all houses, and ads label the same one differently.
const fn is_house_family(t: PropertyType) -> bool {
    matches!(
        t,
        PropertyType::House | PropertyType::SemiDetached | PropertyType::Sobrado
    )
}

fn compare_type(a: &MatchInput, b: &MatchInput, found: &mut Findings) {
    match (
        comparable_type(a.property_type),
        comparable_type(b.property_type),
    ) {
        (Some(x), Some(y)) if x == y => found.signal(SignalKind::SamePropertyType, x.to_string()),
        (Some(x), Some(y)) if is_house_family(x) && is_house_family(y) => {
            found.notes.push(format!(
                "house type labels differ ({x} vs {y}); treated as compatible"
            ));
        }
        (Some(x), Some(y)) => found.conflict(
            ConflictKind::PropertyType,
            Severity::Hard,
            format!("{x} vs {y}"),
        ),
        (x, y) => found.unknown_in("property type", x.is_some(), y.is_some()),
    }
}

/// Two amounts differ by at most `permille` parts per thousand of the smaller one.
fn within_permille(x: u64, y: u64, permille: u32) -> bool {
    let (low, high) = (x.min(y), x.max(y));
    (high - low) * 1000 <= low * u64::from(permille)
}

fn beyond_permille(x: u64, y: u64, permille: u32) -> bool {
    let (low, high) = (x.min(y), x.max(y));
    (high - low) * 1000 > low * u64::from(permille)
}

fn square_meters(centi_m2: u32) -> String {
    format!("{}.{:02} m²", centi_m2 / 100, centi_m2 % 100)
}

/// Areas are compared only within their own kind: lot, built and usable area are different
/// things, and the same number under two labels is a coincidence, not evidence.
fn compare_areas(a: &MatchInput, b: &MatchInput, config: &MatchConfig, found: &mut Findings) {
    let kinds = [
        (
            "lot area",
            a.lot_area_centi_m2,
            b.lot_area_centi_m2,
            SignalKind::LotAreaMatch,
            ConflictKind::LotArea,
        ),
        (
            "built area",
            a.built_area_centi_m2,
            b.built_area_centi_m2,
            SignalKind::BuiltAreaMatch,
            ConflictKind::BuiltArea,
        ),
        (
            "usable area",
            a.usable_area_centi_m2,
            b.usable_area_centi_m2,
            SignalKind::UsableAreaMatch,
            ConflictKind::UsableArea,
        ),
    ];
    let mut compared = false;
    for (label, x, y, signal, conflict) in kinds {
        let (Some(x), Some(y)) = (x, y) else {
            found.unknown_in(label, x.is_some(), y.is_some());
            continue;
        };
        compared = true;
        let detail = format!("{label}: {} vs {}", square_meters(x), square_meters(y));
        if within_permille(u64::from(x), u64::from(y), config.area_match_permille) {
            found.signal(signal, detail);
        } else if beyond_permille(u64::from(x), u64::from(y), config.area_conflict_permille) {
            found.conflict(conflict, Severity::Hard, detail);
        }
    }
    let states_any = |m: &MatchInput| {
        m.lot_area_centi_m2.is_some()
            || m.built_area_centi_m2.is_some()
            || m.usable_area_centi_m2.is_some()
    };
    if !compared && states_any(a) && states_any(b) {
        found
            .notes
            .push("areas of different kinds were stated and were not compared".to_owned());
    }
}

fn compare_rooms(a: &MatchInput, b: &MatchInput, found: &mut Findings) {
    let fields = [
        ("bedrooms", a.bedrooms, b.bedrooms, ConflictKind::Bedrooms),
        ("suites", a.suites, b.suites, ConflictKind::Suites),
        (
            "parking spaces",
            a.parking_spaces,
            b.parking_spaces,
            ConflictKind::ParkingSpaces,
        ),
    ];
    let mut agreeing = Vec::new();
    let mut differing = false;
    for (label, x, y, kind) in fields {
        match (x, y) {
            (Some(x), Some(y)) if x == y => agreeing.push(format!("{label} {x}")),
            (Some(x), Some(y)) => {
                differing = true;
                found.conflict(kind, Severity::Hard, format!("{label}: {x} vs {y}"));
            }
            (x, y) => found.unknown_in(label, x.is_some(), y.is_some()),
        }
    }
    if agreeing.len() >= 2 && !differing {
        found.signal(SignalKind::RoomsMatch, agreeing.join(", "));
    }
}

fn compare_price(a: &MatchInput, b: &MatchInput, config: &MatchConfig, found: &mut Findings) {
    let (Some(x), Some(y)) = (a.full_price_centavos, b.full_price_centavos) else {
        found.unknown_in(
            "established full price",
            a.full_price_centavos.is_some(),
            b.full_price_centavos.is_some(),
        );
        return;
    };
    let (Ok(x_amount), Ok(y_amount)) = (u64::try_from(x), u64::try_from(y)) else {
        return;
    };
    if x_amount == 0 || y_amount == 0 {
        return;
    }
    let detail = format!("{} vs {}", reais(x), reais(y));
    if within_permille(x_amount, y_amount, config.price_close_permille) {
        found.signal(SignalKind::PriceClose, detail);
    } else if beyond_permille(x_amount, y_amount, config.price_far_permille) {
        found.conflict(ConflictKind::Price, Severity::Soft, detail);
    }
}

fn compare_text(a: &MatchInput, b: &MatchInput, config: &MatchConfig, found: &mut Findings) {
    let (a_long, b_long) = (
        a.tokens.len() >= config.text_min_tokens,
        b.tokens.len() >= config.text_min_tokens,
    );
    if !(a_long && b_long) {
        found.unknown_in("comparable description text", a_long, b_long);
        return;
    }
    let shared = a.tokens.intersection(&b.tokens).count();
    let distinct = a.tokens.len() + b.tokens.len() - shared;
    if shared * 100 >= distinct * usize::try_from(config.text_min_jaccard_percent).unwrap_or(100) {
        // Only counts: the words themselves never enter the evidence.
        found.signal(
            SignalKind::TextSimilar,
            format!("{shared} of {distinct} distinct terms in common"),
        );
    }
}

/// One-to-one pairs of close images, closest first, ignoring hashes that appear on many listings.
fn pair_images<S: BuildHasher>(
    a: &MatchInput,
    b: &MatchInput,
    config: &MatchConfig,
    common: &HashSet<u64, S>,
    found: &mut Findings,
) -> Vec<ImagePair> {
    let usable = |m: &MatchInput| -> Vec<ImageHash> {
        m.images
            .iter()
            .filter(|i| !common.contains(&i.hash))
            .copied()
            .collect()
    };
    let (usable_a, usable_b) = (usable(a), usable(b));
    let ignored = a.images.len() + b.images.len() - usable_a.len() - usable_b.len();
    if ignored > 0 {
        found.notes.push(format!(
            "{ignored} image(s) ignored: their hash is shared by many listings"
        ));
    }
    let mut close: Vec<(u32, i64, i64)> = Vec::new();
    for x in &usable_a {
        for y in &usable_b {
            let distance = hamming_distance(x.hash, y.hash);
            if distance <= config.image_distance {
                close.push((distance, x.image_id, y.image_id));
            }
        }
    }
    close.sort_unstable();
    let (mut used_a, mut used_b) = (HashSet::new(), HashSet::new());
    let mut pairs = Vec::new();
    for (distance, image_a, image_b) in close {
        if used_a.contains(&image_a) || used_b.contains(&image_b) {
            continue;
        }
        used_a.insert(image_a);
        used_b.insert(image_b);
        pairs.push(ImagePair {
            image_a,
            image_b,
            distance,
        });
    }
    pairs
}

fn categorize(
    a: &MatchInput,
    b: &MatchInput,
    e: &MatchEvidence,
    config: &MatchConfig,
) -> MatchCategory {
    use SignalKind as S;
    if e.has_conflict(ConflictKind::Municipality) {
        return MatchCategory::Low;
    }
    let pairs = e.image_pairs.len();
    let strong_images = pairs >= config.strong_image_pairs;
    let any_area = [S::LotAreaMatch, S::BuiltAreaMatch, S::UsableAreaMatch]
        .into_iter()
        .any(|k| e.has_signal(k));

    if e.has_hard_conflict() {
        // Same photos, different physical facts: typically units of one building. Worth a human
        // look only when the photos are strong; otherwise keep them apart.
        return if strong_images {
            MatchCategory::Possible
        } else {
            MatchCategory::Low
        };
    }
    if !e.conflicts.is_empty() {
        return if pairs >= 1 {
            MatchCategory::Possible
        } else {
            MatchCategory::Low
        };
    }

    let located = e.has_signal(S::SameMunicipality) && e.has_signal(S::SameNeighborhood);
    // Flats of one building share photos, address and floorplan: even all of that cannot tell two
    // units apart, so an apartment is never better than `Possible`.
    let apartment = [a, b]
        .iter()
        .any(|m| m.property_type == Some(PropertyType::Apartment));
    if strong_images && located && !apartment && (any_area || e.has_signal(S::RoomsMatch)) {
        return MatchCategory::High;
    }
    if pairs >= 1 {
        return MatchCategory::Possible;
    }
    // Without photos only a very specific combination is worth review, and never for apartments
    // (identical units of one building are indistinguishable by metadata) or an unknown type.
    let metadata_only = located
        && e.has_signal(S::SamePropertyType)
        && a.property_type != Some(PropertyType::Apartment)
        && any_area
        && e.has_signal(S::RoomsMatch)
        && e.has_signal(S::TextSimilar);
    if metadata_only {
        MatchCategory::Possible
    } else {
        MatchCategory::Low
    }
}

#[cfg(test)]
mod tests;
