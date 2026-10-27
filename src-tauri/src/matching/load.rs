//! Turning stored listings into comparison inputs.

use std::collections::{BTreeSet, HashMap};

use rusqlite::Connection;

use super::compare::{ImageHash, MatchInput};
use crate::{
    db::images::hashes,
    error::{Error, Result},
    models::{
        confidence::Confidence,
        listing::ExtractedFacts,
        price::{MIN_COMPARABLE_CONFIDENCE, PriceClassification},
    },
    parser::fold_text,
};

struct Raw {
    id: i64,
    adapter_id: String,
    property_id: i64,
    title: Option<String>,
    description: Option<String>,
    facts_json: Option<String>,
    price_classification: String,
    price_centavos: Option<i64>,
    price_confidence: Option<String>,
    municipality: Option<String>,
    neighborhood: Option<String>,
}

/// Every listing that belongs to a property, in listing-id order, ready to compare. Listings of
/// every status are included: an expired ad and its relisting are the same property, and linking
/// them keeps price history and the user's choices together.
pub fn load_inputs(conn: &Connection) -> Result<Vec<MatchInput>> {
    let mut group_statement = conn.prepare("SELECT member_id, root_id FROM property_groups")?;
    let roots: HashMap<i64, i64> = group_statement
        .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)))?
        .collect::<std::result::Result<_, _>>()?;
    let mut images: HashMap<i64, Vec<ImageHash>> = HashMap::new();
    for (image_id, listing_id, hash) in hashes(conn)? {
        images
            .entry(listing_id)
            .or_default()
            .push(ImageHash { image_id, hash });
    }

    let mut statement = conn.prepare(
        "SELECT l.id, l.adapter_id, l.property_id, l.title, l.raw_description, l.extracted_facts_json,
                l.price_classification, l.price_centavos, l.price_confidence, p.municipality, p.neighborhood
           FROM listing_links l
           JOIN properties p ON p.id = l.property_id
          ORDER BY l.id",
    )?;
    let rows = statement
        .query_map([], |r| {
            Ok(Raw {
                id: r.get(0)?,
                adapter_id: r.get(1)?,
                property_id: r.get(2)?,
                title: r.get(3)?,
                description: r.get(4)?,
                facts_json: r.get(5)?,
                price_classification: r.get(6)?,
                price_centavos: r.get(7)?,
                price_confidence: r.get(8)?,
                municipality: r.get(9)?,
                neighborhood: r.get(10)?,
            })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    rows.into_iter()
        .map(|raw| {
            let listing_images = images.remove(&raw.id).unwrap_or_default();
            build(&raw, &roots, listing_images)
        })
        .collect()
}

fn folded(name: Option<&str>) -> Option<String> {
    name.map(fold_text).filter(|n| !n.trim().is_empty())
}

fn build(raw: &Raw, roots: &HashMap<i64, i64>, images: Vec<ImageHash>) -> Result<MatchInput> {
    let mut input = MatchInput::new(raw.id, &raw.adapter_id);
    input.group_root = roots
        .get(&raw.property_id)
        .copied()
        .unwrap_or(raw.property_id);
    input.municipality = folded(raw.municipality.as_deref());
    input.neighborhood = folded(raw.neighborhood.as_deref());
    input.images = images;
    input.tokens = distinctive_terms(raw.title.as_deref(), raw.description.as_deref());

    if let Some(json) = raw.facts_json.as_deref() {
        let facts: ExtractedFacts = serde_json::from_str(json)?;
        let c = &facts.characteristics;
        input.property_type = c.property_type.known().map(|e| e.value);
        input.bedrooms = c.bedrooms.known().map(|e| e.value);
        input.suites = c.suites.known().map(|e| e.value);
        input.parking_spaces = c.parking_spaces.known().map(|e| e.value);
        input.lot_area_centi_m2 = c
            .lot_area
            .known()
            .map(|e| e.value.area().centi_square_meters());
        input.built_area_centi_m2 = c
            .built_area
            .known()
            .map(|e| e.value.area().centi_square_meters());
        input.usable_area_centi_m2 = c
            .usable_area
            .known()
            .map(|e| e.value.area().centi_square_meters());
        let contradictions = [
            ("property type", c.property_type.is_conflicting()),
            ("bedrooms", c.bedrooms.is_conflicting()),
            ("suites", c.suites.is_conflicting()),
            ("parking spaces", c.parking_spaces.is_conflicting()),
            ("lot area", c.lot_area.is_conflicting()),
            ("built area", c.built_area.is_conflicting()),
            ("usable area", c.usable_area.is_conflicting()),
        ];
        input.contradictory = contradictions
            .into_iter()
            .filter(|(_, conflicting)| *conflicting)
            .map(|(label, _)| label.to_owned())
            .collect();
    }

    input.full_price_centavos = established_full_price(raw)?;
    Ok(input)
}

/// The listing's headline price, only when it is a full price with comparable confidence.
fn established_full_price(raw: &Raw) -> Result<Option<i64>> {
    let (Some(centavos), Some(confidence)) = (raw.price_centavos, raw.price_confidence.as_deref())
    else {
        return Ok(None);
    };
    if raw.price_classification != PriceClassification::FullPrice.as_str() {
        return Ok(None);
    }
    let confidence: Confidence = confidence.parse().map_err(|_| Error::InvalidStoredValue {
        column: "listing_links.price_confidence",
        value: confidence.to_owned(),
    })?;
    Ok((confidence >= MIN_COMPARABLE_CONFIDENCE).then_some(centavos))
}

/// Distinctive words of the advertisement: folded, at least four letters, letters only. Digits
/// are excluded on purpose so phone numbers, prices and lot numbers cannot influence (or leak
/// into) anything.
fn distinctive_terms(title: Option<&str>, description: Option<&str>) -> BTreeSet<String> {
    let text = fold_text(&format!(
        "{} {}",
        title.unwrap_or_default(),
        description.unwrap_or_default()
    ));
    text.split(|c: char| !c.is_alphabetic())
        .filter(|word| word.chars().count() >= 4)
        .map(str::to_owned)
        .collect()
}

#[cfg(test)]
mod tests;
