//! Properties: canonical values reconciled from listings, user status across merge groups, and
//! the paginated read model behind the search results.
//!
//! A *merge group* is a root property plus everything merged into it (see the `property_groups`
//! view). Listings keep pointing at their original property, so a group's listings are those of
//! all its members.

use chrono::{DateTime, Utc};
use rusqlite::{Connection, OptionalExtension, Row, TransactionBehavior, named_params, params};
use serde::Serialize;
use serde_json::json;

use super::{listings::StoredPrice, parse_timestamp};
use crate::{
    error::{Error, Result},
    models::{
        area::Area,
        characteristics::{Fact, PropertyType},
        confidence::Confidence,
        listing::{ExtractedFacts, ExtractionStatus, ListingStatus, UserStatus},
        money::Money,
        price::{MIN_COMPARABLE_CONFIDENCE, PriceClassification},
    },
};

/// Bound for walks up the merge chain. Triggers make cycles impossible; this is belt and braces.
const MAX_CHAIN_DEPTH: i64 = 32;

// ================================================================================================
// Canonical values
// ================================================================================================

/// Recomputes the canonical columns of `property_id` and of every property it has been merged
/// into, so each reflects the listings of its own merge group.
pub fn refresh_canonical_chain(conn: &Connection, property_id: i64) -> Result<()> {
    let mut statement = conn.prepare(
        "WITH RECURSIVE up(id, parent, depth) AS (
             SELECT id, merged_into_property_id, 0 FROM properties WHERE id = ?1
             UNION ALL
             SELECT p.id, p.merged_into_property_id, up.depth + 1
               FROM properties p JOIN up ON p.id = up.parent
              WHERE up.depth < ?2
         )
         SELECT id FROM up ORDER BY depth",
    )?;
    let chain = statement
        .query_map(params![property_id, MAX_CHAIN_DEPTH], |r| {
            r.get::<_, i64>(0)
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    for id in chain {
        refresh_canonical(conn, id)?;
    }
    Ok(())
}

/// What one listing says about one characteristic.
enum Reading {
    Known {
        value: i64,
        snippet: String,
        field: Option<String>,
    },
    /// The listing contradicts itself (e.g. two different bedroom counts).
    Conflicting,
}

fn readings<V>(
    facts: &[(i64, ExtractedFacts)],
    pick: impl Fn(&ExtractedFacts) -> &Fact<V>,
    to_i64: impl Fn(&V) -> i64,
) -> Vec<(i64, Reading)> {
    facts
        .iter()
        .filter_map(|(listing_id, f)| match pick(f) {
            Fact::Unknown => None,
            Fact::Known(e) => Some((
                *listing_id,
                Reading::Known {
                    value: to_i64(&e.value),
                    snippet: e.evidence.snippet.clone(),
                    field: e.evidence.field.clone(),
                },
            )),
            Fact::Conflicting(_) => Some((*listing_id, Reading::Conflicting)),
        })
        .collect()
}

/// The canonical value (only when every listing that speaks agrees) and its provenance.
fn reconcile(observations: &[(i64, Reading)]) -> (Option<i64>, serde_json::Value) {
    if observations.is_empty() {
        return (None, json!({ "status": "unknown" }));
    }
    let mut first: Option<i64> = None;
    let mut agreed = true;
    for (_, reading) in observations {
        match reading {
            Reading::Conflicting => agreed = false,
            Reading::Known { value, .. } => match first {
                None => first = Some(*value),
                Some(v) if v != *value => agreed = false,
                Some(_) => {}
            },
        }
    }
    if agreed {
        let sources: Vec<serde_json::Value> = observations
            .iter()
            .filter_map(|(id, reading)| match reading {
                Reading::Known { snippet, field, .. } => {
                    let mut source = json!({ "listing_id": id, "snippet": snippet });
                    if let Some(field) = field {
                        source["field"] = json!(field);
                    }
                    Some(source)
                }
                Reading::Conflicting => None,
            })
            .collect();
        return (
            first,
            json!({ "status": "known", "value": first, "sources": sources }),
        );
    }
    let values: Vec<serde_json::Value> = observations
        .iter()
        .map(|(id, reading)| match reading {
            Reading::Known { value, .. } => json!({ "listing_id": id, "value": value }),
            Reading::Conflicting => {
                json!({ "listing_id": id, "value": null, "internal_conflict": true })
            }
        })
        .collect();
    (None, json!({ "status": "conflicting", "values": values }))
}

/// The canonical columns of a `properties` row as stored (`bedrooms` .. `usable_area`, provenance).
type CanonicalColumns = (
    Option<i64>,
    Option<i64>,
    Option<i64>,
    Option<i64>,
    Option<i64>,
    Option<i64>,
    Option<String>,
);

/// `(listing id, extracted facts)` of every listing in the merge group rooted at `property_id`
/// (itself and everything merged into it, directly or through a chain), oldest listing first.
fn group_facts(conn: &Connection, property_id: i64) -> Result<Vec<(i64, ExtractedFacts)>> {
    let mut statement = conn.prepare(
        "WITH RECURSIVE sub(id) AS (
             SELECT ?1
             UNION ALL
             SELECT p.id FROM properties p JOIN sub ON p.merged_into_property_id = sub.id
         )
         SELECT l.id, l.extracted_facts_json
           FROM listing_links l
          WHERE l.property_id IN (SELECT id FROM sub) AND l.extracted_facts_json IS NOT NULL
          ORDER BY l.id",
    )?;
    let raw = statement
        .query_map([property_id], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    raw.into_iter()
        .map(|(id, json)| Ok((id, serde_json::from_str(&json)?)))
        .collect()
}

/// Recomputes the canonical columns of one property from the listings of its merge group
/// (itself and everything merged into it). Values on which listings disagree stay unknown and
/// the disagreement is recorded in `canonical_provenance_json`. Rows are only written when
/// something actually changed, so `updated_at` is not bumped by every scrape.
pub fn refresh_canonical(conn: &Connection, property_id: i64) -> Result<()> {
    let facts = group_facts(conn, property_id)?;

    let area = |a: Area| i64::from(a.centi_square_meters());
    let fields = [
        (
            "bedrooms",
            readings(&facts, |f| &f.characteristics.bedrooms, |v| i64::from(*v)),
        ),
        (
            "suites",
            readings(&facts, |f| &f.characteristics.suites, |v| i64::from(*v)),
        ),
        (
            "parking_spaces",
            readings(
                &facts,
                |f| &f.characteristics.parking_spaces,
                |v| i64::from(*v),
            ),
        ),
        (
            "lot_area",
            readings(&facts, |f| &f.characteristics.lot_area, |v| area(v.area())),
        ),
        (
            "built_area",
            readings(
                &facts,
                |f| &f.characteristics.built_area,
                |v| area(v.area()),
            ),
        ),
        (
            "usable_area",
            readings(
                &facts,
                |f| &f.characteristics.usable_area,
                |v| area(v.area()),
            ),
        ),
    ];
    let mut values = Vec::with_capacity(fields.len());
    let mut provenance = serde_json::Map::new();
    for (name, observations) in &fields {
        let (value, why) = reconcile(observations);
        values.push(value);
        provenance.insert((*name).to_owned(), why);
    }
    let provenance_json = serde_json::to_string(&provenance)?;

    let current: Option<CanonicalColumns> = conn
        .query_row(
            "SELECT bedrooms, suites, parking_spaces, lot_area_centi_m2, built_area_centi_m2,
                    usable_area_centi_m2, canonical_provenance_json
               FROM properties WHERE id = ?1",
            [property_id],
            |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                    r.get(6)?,
                ))
            },
        )
        .optional()?;
    let Some(current) = current else {
        return Ok(());
    };
    let unchanged = (
        current.0, current.1, current.2, current.3, current.4, current.5,
    ) == (
        values[0], values[1], values[2], values[3], values[4], values[5],
    ) && current.6.as_deref() == Some(provenance_json.as_str());
    if !unchanged {
        conn.execute(
            "UPDATE properties
                SET bedrooms = ?1, suites = ?2, parking_spaces = ?3, lot_area_centi_m2 = ?4,
                    built_area_centi_m2 = ?5, usable_area_centi_m2 = ?6, canonical_provenance_json = ?7
              WHERE id = ?8",
            params![values[0], values[1], values[2], values[3], values[4], values[5], provenance_json, property_id],
        )?;
    }
    Ok(())
}

// ================================================================================================
// Merge groups and user status
// ================================================================================================

/// The root of the merge group `property_id` belongs to (itself when it is not merged).
pub fn root_of(conn: &Connection, property_id: i64) -> Result<Option<i64>> {
    Ok(conn
        .query_row(
            "WITH RECURSIVE up(id, parent, depth) AS (
                 SELECT id, merged_into_property_id, 0 FROM properties WHERE id = ?1
                 UNION ALL
                 SELECT p.id, p.merged_into_property_id, up.depth + 1
                   FROM properties p JOIN up ON p.id = up.parent
                  WHERE up.depth < ?2
             )
             SELECT id FROM up WHERE parent IS NULL",
            params![property_id, MAX_CHAIN_DEPTH],
            |r| r.get(0),
        )
        .optional()?)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct EffectiveStatus {
    pub status: UserStatus,
    /// Members disagree in the favorite/hidden sense; show it to the user.
    pub conflict: bool,
}

pub fn effective_status(conn: &Connection, root_id: i64) -> Result<Option<EffectiveStatus>> {
    let row: Option<(String, i64)> = conn
        .query_row(
            "SELECT effective_status, status_conflict FROM property_group_status WHERE root_id = ?1",
            [root_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    row.map(|(status, conflict)| {
        Ok(EffectiveStatus {
            status: parse_enum("property_group_status.effective_status", &status)?,
            conflict: conflict == 1,
        })
    })
    .transpose()
}

/// Applies a user decision to the **whole merge group** of `property_id`, returning how many
/// property rows were touched (0 when the property does not exist). Hiding keeps the original
/// `hidden_at` of members that were already hidden; any other status clears it.
pub fn set_user_status(
    conn: &mut Connection,
    property_id: i64,
    status: UserStatus,
    at: DateTime<Utc>,
) -> Result<usize> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let Some(root) = root_of(&tx, property_id)? else {
        return Ok(0);
    };
    let at = super::format_timestamp(at);
    let changed = if status == UserStatus::Hidden {
        tx.execute(
            "UPDATE properties
                SET user_status = 'hidden', hidden_at = COALESCE(hidden_at, ?1)
              WHERE id IN (SELECT member_id FROM property_groups WHERE root_id = ?2)",
            params![at, root],
        )?
    } else {
        tx.execute(
            "UPDATE properties SET user_status = ?1, hidden_at = NULL
              WHERE id IN (SELECT member_id FROM property_groups WHERE root_id = ?2)",
            params![status.as_str(), root],
        )?
    };
    tx.commit()?;
    Ok(changed)
}

// ================================================================================================
// Read model
// ================================================================================================

#[derive(Debug, Clone, Default)]
pub struct PropertyQuery {
    pub include_hidden: bool,
    pub favorites_only: bool,
    /// A region slug; matches properties in that region or in any region below it.
    pub region_slug: Option<String>,
    pub min_price: Option<Money>,
    pub max_price: Option<Money>,
    /// "Ocultar Ágio (Apenas Valor Total)": only properties with an established full price.
    pub full_price_only: bool,
    /// `0` means the default.
    pub limit: u32,
    pub offset: u32,
}

impl PropertyQuery {
    pub const DEFAULT_LIMIT: u32 = 50;
    pub const MAX_LIMIT: u32 = 200;

    #[must_use]
    pub fn effective_limit(&self) -> u32 {
        match self.limit {
            0 => Self::DEFAULT_LIMIT,
            n => n.min(Self::MAX_LIMIT),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub total: u64,
    pub offset: u32,
    pub limit: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PriceView {
    pub amount: Money,
    pub classification: PriceClassification,
    pub confidence: Confidence,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ListingView {
    pub listing_id: i64,
    pub source_platform: String,
    pub adapter_id: String,
    pub url: String,
    pub title: Option<String>,
    pub price: Option<PriceView>,
    pub status: ListingStatus,
    pub extraction_status: ExtractionStatus,
    pub first_seen_at: DateTime<Utc>,
    pub last_seen_at: DateTime<Utc>,
    pub last_verified_at: Option<DateTime<Utc>>,
    pub last_scraped_at: Option<DateTime<Utc>>,
}

/// An established, current full price of one listing of the property.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ComparablePrice {
    pub listing_id: i64,
    pub adapter_id: String,
    pub amount: Money,
    pub confidence: Confidence,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PropertyView {
    pub id: i64,
    pub status: UserStatus,
    pub status_conflict: bool,
    pub municipality: Option<String>,
    pub neighborhood: Option<String>,
    pub property_type: Option<PropertyType>,
    pub bedrooms: Option<u8>,
    pub suites: Option<u8>,
    pub parking_spaces: Option<u8>,
    pub lot_area: Option<Area>,
    pub built_area: Option<Area>,
    pub usable_area: Option<Area>,
    pub match_review_status: String,
    /// Every listing of the merge group, newest activity first.
    pub listings: Vec<ListingView>,
    /// Established full prices of the active listings. Different sources often differ; the
    /// spread is information, not a conflict.
    pub comparable_prices: Vec<ComparablePrice>,
    pub price_spread: Option<(Money, Money)>,
    pub canonical_provenance: Option<serde_json::Value>,
}

pub(super) fn parse_enum<T: std::str::FromStr>(column: &'static str, text: &str) -> Result<T> {
    text.parse().map_err(|_| Error::InvalidStoredValue {
        column,
        value: text.to_owned(),
    })
}

/// SQL list of the confidences that qualify a full price as comparable, derived from the same
/// constant the Rust side uses so the two cannot drift apart.
fn comparable_confidences_sql() -> String {
    Confidence::ALL
        .iter()
        .filter(|c| **c >= MIN_COMPARABLE_CONFIDENCE)
        .map(|c| format!("'{}'", c.as_str()))
        .collect::<Vec<_>>()
        .join(", ")
}

/// SQL predicate (over `listing_links` aliased `l`) for "an established, current full price".
fn established_sql() -> String {
    format!(
        "(l.price_classification = 'full_price' AND l.price_centavos IS NOT NULL \
          AND l.price_confidence IN ({}) AND l.listing_status = 'active')",
        comparable_confidences_sql()
    )
}

fn listing_view_from_row(r: &Row<'_>) -> rusqlite::Result<RawListingView> {
    Ok(RawListingView {
        listing_id: r.get(0)?,
        source_platform: r.get(1)?,
        adapter_id: r.get(2)?,
        url: r.get(3)?,
        title: r.get(4)?,
        price_centavos: r.get(5)?,
        price_classification: r.get(6)?,
        price_confidence: r.get(7)?,
        status: r.get(8)?,
        extraction_status: r.get(9)?,
        first_seen_at: r.get(10)?,
        last_seen_at: r.get(11)?,
        last_verified_at: r.get(12)?,
        last_scraped_at: r.get(13)?,
    })
}

struct RawListingView {
    listing_id: i64,
    source_platform: String,
    adapter_id: String,
    url: String,
    title: Option<String>,
    price_centavos: Option<i64>,
    price_classification: String,
    price_confidence: Option<String>,
    status: String,
    extraction_status: String,
    first_seen_at: String,
    last_seen_at: String,
    last_verified_at: Option<String>,
    last_scraped_at: Option<String>,
}

pub(super) fn stored_price(
    centavos: Option<i64>,
    classification: &str,
    confidence: Option<&str>,
) -> Result<Option<StoredPrice>> {
    match (centavos, confidence) {
        (Some(centavos), Some(confidence)) => Ok(Some(StoredPrice {
            amount: Money::from_centavos(centavos)?,
            classification: parse_enum("listing_links.price_classification", classification)?,
            confidence: parse_enum("listing_links.price_confidence", confidence)?,
        })),
        _ => Ok(None),
    }
}

/// The listings of a merge group (newest first) and the prices among them that count as
/// established, current full prices.
fn load_listings(
    conn: &Connection,
    root_id: i64,
) -> Result<(Vec<ListingView>, Vec<ComparablePrice>)> {
    let mut statement = conn.prepare(
        "SELECT l.id, l.source_platform, l.adapter_id, l.url, l.title, l.price_centavos,
                l.price_classification, l.price_confidence, l.listing_status, l.extraction_status,
                l.first_seen_at, l.last_seen_at, l.last_verified_at, l.last_scraped_at
           FROM listing_links l
          WHERE l.property_id IN (SELECT member_id FROM property_groups WHERE root_id = ?1)
          ORDER BY l.last_seen_at DESC, l.id DESC",
    )?;
    let raw_listings = statement
        .query_map([root_id], listing_view_from_row)?
        .collect::<std::result::Result<Vec<_>, _>>()?;

    let optional_ts = |value: Option<String>| value.as_deref().map(parse_timestamp).transpose();
    let mut listings = Vec::with_capacity(raw_listings.len());
    let mut comparable_prices = Vec::new();
    for raw in raw_listings {
        let price = stored_price(
            raw.price_centavos,
            &raw.price_classification,
            raw.price_confidence.as_deref(),
        )?;
        let listing_status: ListingStatus =
            parse_enum("listing_links.listing_status", &raw.status)?;
        if let Some(p) = &price
            && p.classification == PriceClassification::FullPrice
            && p.confidence >= MIN_COMPARABLE_CONFIDENCE
            && listing_status == ListingStatus::Active
        {
            comparable_prices.push(ComparablePrice {
                listing_id: raw.listing_id,
                adapter_id: raw.adapter_id.clone(),
                amount: p.amount,
                confidence: p.confidence,
            });
        }
        listings.push(ListingView {
            listing_id: raw.listing_id,
            source_platform: raw.source_platform,
            adapter_id: raw.adapter_id,
            url: raw.url,
            title: raw.title,
            price: price.map(|p| PriceView {
                amount: p.amount,
                classification: p.classification,
                confidence: p.confidence,
            }),
            status: listing_status,
            extraction_status: parse_enum(
                "listing_links.extraction_status",
                &raw.extraction_status,
            )?,
            first_seen_at: parse_timestamp(&raw.first_seen_at)?,
            last_seen_at: parse_timestamp(&raw.last_seen_at)?,
            last_verified_at: optional_ts(raw.last_verified_at)?,
            last_scraped_at: optional_ts(raw.last_scraped_at)?,
        });
    }
    Ok((listings, comparable_prices))
}

pub fn get_property_view(conn: &Connection, root_id: i64) -> Result<Option<PropertyView>> {
    type Raw = (
        Option<String>,
        Option<String>,
        Option<String>,
        Option<i64>,
        Option<i64>,
        Option<i64>,
        Option<i64>,
        Option<i64>,
        Option<i64>,
        String,
        Option<String>,
    );
    let raw: Option<Raw> = conn
        .query_row(
            "SELECT municipality, neighborhood, property_type, bedrooms, suites, parking_spaces,
                    lot_area_centi_m2, built_area_centi_m2, usable_area_centi_m2,
                    match_review_status, canonical_provenance_json
               FROM properties WHERE id = ?1 AND merged_into_property_id IS NULL",
            [root_id],
            |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                    r.get(6)?,
                    r.get(7)?,
                    r.get(8)?,
                    r.get(9)?,
                    r.get(10)?,
                ))
            },
        )
        .optional()?;
    let Some((
        municipality,
        neighborhood,
        property_type,
        bedrooms,
        suites,
        parking,
        lot,
        built,
        usable,
        review,
        provenance,
    )) = raw
    else {
        return Ok(None);
    };
    let status = effective_status(conn, root_id)?.ok_or_else(|| Error::InvalidStoredValue {
        column: "property_group_status",
        value: format!("no status for root {root_id}"),
    })?;

    let (listings, comparable_prices) = load_listings(conn, root_id)?;
    let price_spread = comparable_prices
        .iter()
        .map(|p| p.amount)
        .min()
        .zip(comparable_prices.iter().map(|p| p.amount).max());

    let count = |value: Option<i64>| value.and_then(|v| u8::try_from(v).ok());
    let area = |value: Option<i64>| {
        value
            .and_then(|v| u64::try_from(v).ok())
            .and_then(|v| Area::from_centi_square_meters(v).ok())
    };
    Ok(Some(PropertyView {
        id: root_id,
        status: status.status,
        status_conflict: status.conflict,
        municipality,
        neighborhood,
        property_type: property_type
            .as_deref()
            .map(|t| parse_enum("properties.property_type", t))
            .transpose()?,
        bedrooms: count(bedrooms),
        suites: count(suites),
        parking_spaces: count(parking),
        lot_area: area(lot),
        built_area: area(built),
        usable_area: area(usable),
        match_review_status: review,
        listings,
        comparable_prices,
        price_spread,
        canonical_provenance: provenance
            .as_deref()
            .map(serde_json::from_str)
            .transpose()?,
    }))
}

pub fn list_properties(conn: &Connection, query: &PropertyQuery) -> Result<Page<PropertyView>> {
    let established = established_sql();
    let filter = format!(
        "FROM properties p
         JOIN property_group_status s ON s.root_id = p.id
        WHERE p.merged_into_property_id IS NULL
          AND (:include_hidden = 1 OR s.effective_status <> 'hidden')
          AND (:favorites_only = 0 OR s.effective_status = 'favorite')
          AND (:region_slug IS NULL OR p.region_id IN (SELECT id FROM region_tree))
          AND EXISTS (SELECT 1 FROM property_groups g
                        JOIN listing_links l ON l.property_id = g.member_id
                       WHERE g.root_id = p.id AND l.extraction_status <> 'failed')
          AND (:full_price_only = 0 OR EXISTS (
                SELECT 1 FROM property_groups g JOIN listing_links l ON l.property_id = g.member_id
                 WHERE g.root_id = p.id AND {established}))
          AND ((:min_price IS NULL AND :max_price IS NULL) OR EXISTS (
                SELECT 1 FROM property_groups g JOIN listing_links l ON l.property_id = g.member_id
                 WHERE g.root_id = p.id AND {established}
                   AND (:min_price IS NULL OR l.price_centavos >= :min_price)
                   AND (:max_price IS NULL OR l.price_centavos <= :max_price)))"
    );
    let region_cte = "WITH RECURSIVE region_tree(id) AS (
            SELECT id FROM regions WHERE slug = :region_slug
            UNION ALL
            SELECT r.id FROM regions r JOIN region_tree t ON r.parent_id = t.id
         )";
    let limit = query.effective_limit();

    let total: i64 = conn.query_row(
        &format!("{region_cte} SELECT COUNT(*) {filter}"),
        named_params! {
            ":include_hidden": i64::from(query.include_hidden),
            ":favorites_only": i64::from(query.favorites_only),
            ":region_slug": query.region_slug,
            ":full_price_only": i64::from(query.full_price_only),
            ":min_price": query.min_price.map(Money::centavos),
            ":max_price": query.max_price.map(Money::centavos),
        },
        |r| r.get(0),
    )?;

    let mut statement = conn.prepare(&format!(
        "{region_cte}
         SELECT p.id {filter}
         ORDER BY (SELECT MAX(l.last_seen_at) FROM property_groups g
                     JOIN listing_links l ON l.property_id = g.member_id
                    WHERE g.root_id = p.id) DESC, p.id DESC
         LIMIT :limit OFFSET :offset"
    ))?;
    let root_ids = statement
        .query_map(
            named_params! {
                ":include_hidden": i64::from(query.include_hidden),
                ":favorites_only": i64::from(query.favorites_only),
                ":region_slug": query.region_slug,
                ":full_price_only": i64::from(query.full_price_only),
                ":min_price": query.min_price.map(Money::centavos),
                ":max_price": query.max_price.map(Money::centavos),
                ":limit": i64::from(limit),
                ":offset": i64::from(query.offset),
            },
            |r| r.get::<_, i64>(0),
        )?
        .collect::<std::result::Result<Vec<_>, _>>()?;

    let mut items = Vec::with_capacity(root_ids.len());
    for id in root_ids {
        if let Some(view) = get_property_view(conn, id)? {
            items.push(view);
        }
    }
    Ok(Page {
        items,
        total: u64::try_from(total).unwrap_or(0),
        offset: query.offset,
        limit,
    })
}

#[cfg(test)]
mod tests;
