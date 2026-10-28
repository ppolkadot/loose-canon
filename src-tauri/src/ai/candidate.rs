//! What is sent to the model about each candidate: a compact record, never contact data.
//!
//! A candidate carries the facts and the local score, the prices per source, and a short
//! excerpt of the advertisement text with phone numbers, addresses and links removed. It does not
//! carry listing URLs, titles, image links or anything that identifies the advertiser.

use std::collections::{BTreeSet, HashMap};

use rusqlite::Connection;
use serde::Serialize;
use sha2::{Digest, Sha256};

use super::redact::excerpt;
use crate::{
    db::properties::{PropertyQuery, PropertySort, PropertyView, list_properties},
    error::Result,
    models::{listing::ListingStatus, money::Money},
    ranking::load::conflicting_fields_of,
};

/// How much advertisement text one candidate may carry.
pub const TEXT_CHARS: usize = 700;

/// One active listing of the property: where the price comes from and what kind of price it is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CandidateListing {
    /// The source's identifier, not its address.
    pub source: String,
    pub price: Option<String>,
    /// `full_price`, `agio` or `unknown`: an ágio is not the price of the property.
    pub price_kind: Option<String>,
    pub price_confidence: Option<String>,
}

/// A local score component that could be computed, with its 0-100 value.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CandidateComponent {
    pub name: String,
    pub score: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Candidate {
    pub id: i64,
    pub municipality: Option<String>,
    pub neighborhood: Option<String>,
    pub property_type: Option<String>,
    /// The comparable price (median of the established full prices), as `R$ 450.000,00`.
    pub comparable_price: Option<String>,
    /// Gap between the highest and lowest established price across sources, in percent.
    pub price_spread_percent: Option<f64>,
    pub built_area: Option<String>,
    pub lot_area: Option<String>,
    pub bedrooms: Option<u8>,
    pub suites: Option<u8>,
    pub parking_spaces: Option<u8>,
    pub active_listings: usize,
    pub sources: usize,
    pub listings: Vec<CandidateListing>,
    /// Facts the data does not give.
    pub unknown_facts: Vec<String>,
    /// Facts on which the listings of the property contradict each other.
    pub conflicting_facts: Vec<String>,
    pub local_score: f64,
    pub data_confidence: String,
    pub confidence_reasons: Vec<String>,
    pub local_components: Vec<CandidateComponent>,
    /// An excerpt of the advertisement text, contact data removed.
    pub text: String,
}

impl Candidate {
    /// SHA-256 (hex) of everything that is sent about this property. When any of it changes, a
    /// stored AI note for the property is out of date.
    #[must_use]
    pub fn fingerprint(&self) -> String {
        let json =
            serde_json::to_vec(self).expect("a candidate is plain data and always serialises");
        hex(&Sha256::digest(json))
    }

    /// The candidate for a property, or `None` when it has no local score (an unscored property
    /// is never prioritised).
    fn from_view(view: &PropertyView, description: Option<&str>) -> Option<Self> {
        let local_score = view.local_score?;
        let active: Vec<_> = view
            .listings
            .iter()
            .filter(|l| l.status == ListingStatus::Active)
            .collect();
        let sources: BTreeSet<&str> = active.iter().map(|l| l.adapter_id.as_str()).collect();
        let details = view.score_details.as_ref();
        let price_spread_percent = match view.price_spread {
            Some((low, high)) if view.comparable_prices.len() >= 2 && high.centavos() > 0 => {
                let gap =
                    (high.centavos() - low.centavos()) as f64 * 100.0 / high.centavos() as f64;
                Some((gap * 10.0).round() / 10.0)
            }
            _ => None,
        };
        let unknown_facts = [
            ("property_type", view.property_type.is_none()),
            ("bedrooms", view.bedrooms.is_none()),
            ("suites", view.suites.is_none()),
            ("parking_spaces", view.parking_spaces.is_none()),
            ("built_area", view.built_area.is_none()),
            ("lot_area", view.lot_area.is_none()),
        ]
        .into_iter()
        .filter(|(_, unknown)| *unknown)
        .map(|(name, _)| name.to_owned())
        .collect();
        Some(Self {
            id: view.id,
            municipality: view.municipality.clone(),
            neighborhood: view.neighborhood.clone(),
            property_type: view.property_type.map(|t| t.as_str().to_owned()),
            comparable_price: details
                .and_then(|d| d.comparable_price_centavos)
                .and_then(|c| Money::from_centavos(c).ok())
                .map(|m| m.to_string()),
            price_spread_percent,
            built_area: view.built_area.map(|a| a.to_string()),
            lot_area: view.lot_area.map(|a| a.to_string()),
            bedrooms: view.bedrooms,
            suites: view.suites,
            parking_spaces: view.parking_spaces,
            active_listings: active.len(),
            sources: sources.len(),
            listings: active
                .iter()
                .map(|l| CandidateListing {
                    source: l.adapter_id.clone(),
                    price: l.price.as_ref().map(|p| p.amount.to_string()),
                    price_kind: l
                        .price
                        .as_ref()
                        .map(|p| p.classification.as_str().to_owned()),
                    price_confidence: l.price.as_ref().map(|p| p.confidence.as_str().to_owned()),
                })
                .collect(),
            unknown_facts,
            conflicting_facts: view
                .canonical_provenance
                .as_ref()
                .map(conflicting_fields_of)
                .unwrap_or_default(),
            local_score,
            data_confidence: view
                .data_confidence
                .map_or_else(String::new, |c| c.as_str().to_owned()),
            confidence_reasons: details
                .map(|d| d.confidence_reasons.iter().map(serde_name).collect())
                .unwrap_or_default(),
            local_components: details
                .map(|d| {
                    d.components
                        .iter()
                        .filter_map(|c| {
                            Some(CandidateComponent {
                                name: serde_name(&c.kind),
                                score: c.score?,
                            })
                        })
                        .collect()
                })
                .unwrap_or_default(),
            text: description
                .map(|d| excerpt(d, TEXT_CHARS))
                .unwrap_or_default(),
        })
    }
}

/// Lower-case hexadecimal text of a digest.
fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut out, b| {
            let _ = write!(out, "{b:02x}");
            out
        })
}

/// The snake_case name serde gives a plain enum value.
fn serde_name<T: Serialize>(value: &T) -> String {
    serde_json::to_value(value)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_default()
}

/// SHA-256 (hex) of the whole request: the model, the prompt version and every candidate in the
/// order sent. Equal fingerprints mean an equal request, so a stored answer can be reused.
#[must_use]
pub fn request_fingerprint(model: &str, prompt_version: &str, candidates: &[Candidate]) -> String {
    // A NUL between the parts keeps `("m1p", "1")` and `("m1", "p1")` apart.
    let mut hash = Sha256::new();
    for part in [model, prompt_version] {
        hash.update(part.as_bytes());
        hash.update([0]);
    }
    for candidate in candidates {
        hash.update(candidate.fingerprint().as_bytes());
        hash.update([0]);
    }
    hex(&hash.finalize())
}

/// The text of the most recently scraped active listing of each group, by group root.
fn descriptions(conn: &Connection, roots: &[i64]) -> Result<HashMap<i64, String>> {
    let mut statement = conn.prepare(
        "SELECT g.root_id, l.raw_description
           FROM property_groups g
           JOIN listing_links l ON l.property_id = g.member_id
          WHERE g.root_id IN (SELECT value FROM json_each(?1))
            AND l.listing_status = 'active'
            AND l.raw_description IS NOT NULL AND trim(l.raw_description) <> ''
          ORDER BY g.root_id, l.last_scraped_at DESC, l.id DESC",
    )?;
    let rows = statement
        .query_map([serde_json::to_string(roots)?], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let mut by_root = HashMap::new();
    for (root, text) in rows {
        by_root.entry(root).or_insert(text);
    }
    Ok(by_root)
}

/// The best-ranked visible properties, up to `count`, as candidates. Properties without a local
/// score, hidden ones and ones with no active listing are never candidates.
pub fn select_candidates(conn: &Connection, count: usize) -> Result<Vec<Candidate>> {
    // Sorted by score with the unscored last, so the first `count` are the ones wanted.
    let page = list_properties(
        conn,
        &PropertyQuery {
            sort: PropertySort::Score,
            limit: u32::try_from(count).unwrap_or(u32::MAX),
            ..PropertyQuery::default()
        },
    )?;
    let roots: Vec<i64> = page.items.iter().map(|v| v.id).collect();
    let texts = descriptions(conn, &roots)?;
    // An unscored property yields no candidate.
    Ok(page
        .items
        .iter()
        .filter_map(|v| Candidate::from_view(v, texts.get(&v.id).map(String::as_str)))
        .collect())
}

#[cfg(test)]
mod tests;
