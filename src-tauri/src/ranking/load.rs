//! Reading stored properties into ranking inputs.

use std::collections::{BTreeMap, HashSet};

use rusqlite::Connection;

use super::score::{ListingInput, RankInput};
use crate::{
    db::parse_timestamp,
    error::{Error, Result},
    models::{
        characteristics::PropertyType,
        confidence::Confidence,
        listing::ListingStatus,
        price::{MIN_COMPARABLE_CONFIDENCE, PriceClassification},
    },
};

fn stored<T: std::str::FromStr>(column: &'static str, text: &str) -> Result<T> {
    text.parse().map_err(|_| Error::InvalidStoredValue {
        column,
        value: text.to_owned(),
    })
}

fn small(column: &'static str, value: Option<i64>) -> Result<Option<u8>> {
    value
        .map(|v| {
            u8::try_from(v).map_err(|_| Error::InvalidStoredValue {
                column,
                value: v.to_string(),
            })
        })
        .transpose()
}

fn area(column: &'static str, value: Option<i64>) -> Result<Option<u32>> {
    value
        .map(|v| {
            u32::try_from(v).map_err(|_| Error::InvalidStoredValue {
                column,
                value: v.to_string(),
            })
        })
        .transpose()
}

/// Canonical values on which the group's listings contradict each other, from the provenance
/// document (which marks them `conflicting`), sorted.
pub(crate) fn conflicting_fields_of(provenance: &serde_json::Value) -> Vec<String> {
    let mut fields: Vec<String> = provenance
        .as_object()
        .into_iter()
        .flatten()
        .filter(|(_, why)| why["status"] == "conflicting")
        .map(|(field, _)| field.clone())
        .collect();
    fields.sort();
    fields
}

fn conflicting_fields(provenance: Option<&str>) -> Result<Vec<String>> {
    provenance.map_or_else(
        || Ok(Vec::new()),
        |text| Ok(conflicting_fields_of(&serde_json::from_str(text)?)),
    )
}

/// Roots of the groups that contain at least one merge the user accepted (and has not undone).
fn verified_roots(conn: &Connection) -> Result<HashSet<i64>> {
    let mut statement = conn.prepare(
        "SELECT DISTINCT g.root_id
           FROM property_matches m
           JOIN property_groups g ON g.member_id = m.merge_surviving_property_id
          WHERE m.decision_status = 'accepted' AND m.reverted_at IS NULL",
    )?;
    let roots = statement
        .query_map([], |r| r.get::<_, i64>(0))?
        .collect::<std::result::Result<HashSet<_>, _>>()?;
    Ok(roots)
}

/// Every property that is not merged into another, without its listings yet.
fn load_properties(conn: &Connection) -> Result<BTreeMap<i64, RankInput>> {
    let verified = verified_roots(conn)?;
    let mut statement = conn.prepare(
        "SELECT id, municipality, neighborhood, property_type, bedrooms, suites, parking_spaces,
                built_area_centi_m2, lot_area_centi_m2, canonical_provenance_json
           FROM properties WHERE merged_into_property_id IS NULL ORDER BY id",
    )?;
    let rows = statement
        .query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, Option<String>>(1)?,
                r.get::<_, Option<String>>(2)?,
                r.get::<_, Option<String>>(3)?,
                r.get::<_, Option<i64>>(4)?,
                r.get::<_, Option<i64>>(5)?,
                r.get::<_, Option<i64>>(6)?,
                r.get::<_, Option<i64>>(7)?,
                r.get::<_, Option<i64>>(8)?,
                r.get::<_, Option<String>>(9)?,
            ))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let mut properties = BTreeMap::new();
    for (id, municipality, neighborhood, kind, bedrooms, suites, parking, built, lot, provenance) in
        rows
    {
        properties.insert(
            id,
            RankInput {
                property_id: id,
                municipality,
                neighborhood,
                property_type: kind
                    .as_deref()
                    .map(|k| stored::<PropertyType>("properties.property_type", k))
                    .transpose()?,
                bedrooms: small("properties.bedrooms", bedrooms)?,
                suites: small("properties.suites", suites)?,
                parking_spaces: small("properties.parking_spaces", parking)?,
                built_area_centi_m2: area("properties.built_area_centi_m2", built)?,
                lot_area_centi_m2: area("properties.lot_area_centi_m2", lot)?,
                listings: Vec::new(),
                conflicting_fields: conflicting_fields(provenance.as_deref())?,
                verified_merge: verified.contains(&id),
            },
        );
    }
    Ok(properties)
}

/// Adds every listing to the root of its merge group.
fn attach_listings(conn: &Connection, properties: &mut BTreeMap<i64, RankInput>) -> Result<()> {
    let mut statement = conn.prepare(
        "SELECT g.root_id, l.adapter_id, l.listing_status, l.price_classification, l.price_centavos,
                l.price_confidence, l.first_seen_at, l.last_scraped_at
           FROM property_groups g
           JOIN listing_links l ON l.property_id = g.member_id
          ORDER BY l.id",
    )?;
    let listings = statement
        .query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, Option<i64>>(4)?,
                r.get::<_, Option<String>>(5)?,
                r.get::<_, String>(6)?,
                r.get::<_, Option<String>>(7)?,
            ))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    for (
        root,
        adapter_id,
        status,
        classification,
        centavos,
        confidence,
        first_seen,
        last_scraped,
    ) in listings
    {
        let Some(property) = properties.get_mut(&root) else {
            continue;
        };
        let established_price = match (centavos, confidence.as_deref()) {
            (Some(centavos), Some(confidence))
                if classification == PriceClassification::FullPrice.as_str() =>
            {
                let confidence: Confidence = stored("listing_links.price_confidence", confidence)?;
                (confidence >= MIN_COMPARABLE_CONFIDENCE).then_some((centavos, confidence))
            }
            _ => None,
        };
        property.listings.push(ListingInput {
            adapter_id,
            status: stored::<ListingStatus>("listing_links.listing_status", &status)?,
            established_price,
            first_seen_at: parse_timestamp(&first_seen)?,
            last_scraped_at: last_scraped.as_deref().map(parse_timestamp).transpose()?,
        });
    }
    Ok(())
}

/// Every merge-group root with all the listings of its group, in property-id order.
pub fn load_inputs(conn: &Connection) -> Result<Vec<RankInput>> {
    let mut properties = load_properties(conn)?;
    attach_listings(conn, &mut properties)?;
    Ok(properties.into_values().collect())
}

#[cfg(test)]
mod tests;
