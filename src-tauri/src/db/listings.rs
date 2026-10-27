//! Persistence of individual advertisements.
//!
//! A listing is identified by `(adapter_id, normalized_url)`. Seeing it again updates it in place;
//! the first time it is seen a property candidate is created for it (merging candidates is the
//! matcher's job). Content, price columns and price history are written in ONE transaction, so a
//! failure anywhere leaves the listing exactly as it was.
//!
//! Policies that protect what was already known:
//! * a **failed extraction** never overwrites content, price or `last_verified_at`;
//! * a page with **no price at all** keeps the last known price columns;
//! * a page whose full prices **contradict** each other clears the current price (an ambiguous
//!   price must not look current) but keeps the history;
//! * `first_seen_at` never moves and `last_seen_at` / `last_scraped_at` never go backwards;
//! * nothing is ever deleted: an ad that disappears becomes `unavailable` on explicit evidence.

use chrono::{DateTime, Utc};
use rusqlite::{Connection, OptionalExtension, Row, TransactionBehavior, params};

use super::{
    format_timestamp, parse_timestamp,
    price_history::{PriceObservation, RecordOutcome, record_observation_in},
    properties,
    regions::ResolvedLocation,
};
use crate::{
    error::{Error, Result},
    models::{
        confidence::Confidence,
        listing::{ExtractedFacts, ExtractionStatus, ListingStatus, NormalizedListing},
        money::Money,
        price::{FullPriceStatus, PriceClassification, PriceMention, PriceProvenance},
    },
};

/// One scrape of one advertisement, ready to be stored.
pub struct ListingWrite<'a> {
    pub listing: &'a NormalizedListing,
    /// Raw structured fields exactly as the adapter returned them (so evidence spans that name a
    /// field other than the description can still be resolved).
    pub source_fields: Option<&'a serde_json::Value>,
    /// Where the listing's location text resolved to, if anywhere.
    pub location: Option<ResolvedLocation>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpsertOutcome {
    pub listing_id: i64,
    pub property_id: i64,
    pub listing_created: bool,
    pub property_created: bool,
    /// The listing had been marked unavailable and is live again.
    pub reappeared: bool,
    /// What happened to the price history; `None` when no price was involved.
    pub price: Option<RecordOutcome>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredPrice {
    pub amount: Money,
    pub classification: PriceClassification,
    pub confidence: Confidence,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StoredListing {
    pub id: i64,
    pub property_id: Option<i64>,
    pub source_platform: String,
    pub adapter_id: String,
    pub source_listing_id: Option<String>,
    pub url: String,
    pub normalized_url: String,
    pub title: Option<String>,
    pub raw_description: Option<String>,
    pub location_text: Option<String>,
    pub price: Option<StoredPrice>,
    pub facts: Option<ExtractedFacts>,
    pub parser_version: Option<String>,
    pub status: ListingStatus,
    pub extraction_status: ExtractionStatus,
    pub extraction_error: Option<String>,
    pub first_seen_at: DateTime<Utc>,
    pub last_seen_at: DateTime<Utc>,
    pub last_verified_at: Option<DateTime<Utc>>,
    pub last_scraped_at: Option<DateTime<Utc>>,
    pub status_changed_at: Option<DateTime<Utc>>,
    pub status_reason: Option<String>,
    pub source_fields: Option<serde_json::Value>,
}

/// Inserts or updates a listing (identified by `adapter_id` + `normalized_url`), creates its
/// property candidate when it is new, and records a changed price, all in ONE transaction.
pub fn upsert_listing(conn: &mut Connection, write: &ListingWrite<'_>) -> Result<UpsertOutcome> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let outcome = upsert_in(&tx, write)?;
    tx.commit()?;
    Ok(outcome)
}

/// The price columns a write should set, if any.
enum PriceColumns<'a> {
    Keep,
    Clear,
    Set(&'a PriceMention),
}

/// Everything one write needs, computed once.
struct Prepared<'a> {
    listing: &'a NormalizedListing,
    failed: bool,
    price_columns: PriceColumns<'a>,
    scraped_at: DateTime<Utc>,
    facts_json: String,
    source_fields_json: Option<String>,
}

fn upsert_in(tx: &Connection, write: &ListingWrite<'_>) -> Result<UpsertOutcome> {
    let l = write.listing;
    let failed = l.extraction_status == ExtractionStatus::Failed;
    let headline = if failed { None } else { l.headline_price() };
    let contradictory = !failed
        && matches!(
            l.facts.full_price_status(),
            FullPriceStatus::Conflicting { .. }
        );
    let price_columns = match (headline, contradictory) {
        (Some(mention), _) => PriceColumns::Set(mention),
        (None, true) => PriceColumns::Clear,
        (None, false) => PriceColumns::Keep,
    };
    let prepared = Prepared {
        listing: l,
        failed,
        price_columns,
        scraped_at: l.last_scraped_at.unwrap_or(l.last_seen_at),
        facts_json: serde_json::to_string(&l.facts)?,
        source_fields_json: write.source_fields.map(serde_json::to_string).transpose()?,
    };

    let existing: Option<(i64, Option<i64>, String)> = tx
        .query_row(
            "SELECT id, property_id, listing_status FROM listing_links
              WHERE adapter_id = ?1 AND normalized_url = ?2",
            params![l.source.adapter_id, l.normalized_url],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;

    let (listing_id, property_id, created, reappeared, price) = match existing {
        None => {
            let (listing_id, property_id, price) = insert_new(tx, write, &prepared)?;
            (listing_id, property_id, true, false, price)
        }
        Some((id, property_id, previous_status)) => {
            let (reappeared, price) = update_existing(tx, &prepared, id, &previous_status)?;
            let property_id = property_id.ok_or_else(|| Error::InvalidStoredValue {
                column: "listing_links.property_id",
                value: "NULL on an existing listing".to_owned(),
            })?;
            (id, property_id, false, reappeared, price)
        }
    };

    properties::refresh_canonical_chain(tx, property_id)?;
    Ok(UpsertOutcome {
        listing_id,
        property_id,
        listing_created: created,
        // A new listing always starts a new property candidate; an existing one never does.
        property_created: created,
        reappeared,
        price,
    })
}

/// First sighting: a property candidate, the listing, and its first price observation.
fn insert_new(
    tx: &Connection,
    write: &ListingWrite<'_>,
    p: &Prepared<'_>,
) -> Result<(i64, i64, Option<RecordOutcome>)> {
    let l = p.listing;
    tx.execute(
        "INSERT INTO properties (region_id, municipality, neighborhood) VALUES (?1, ?2, ?3)",
        params![
            write.location.as_ref().map(|loc| loc.region_id),
            write
                .location
                .as_ref()
                .and_then(|loc| loc.municipality.clone()),
            write
                .location
                .as_ref()
                .and_then(|loc| loc.neighborhood.clone()),
        ],
    )?;
    let property_id = tx.last_insert_rowid();
    let (centavos, classification, confidence) = price_values(&p.price_columns);
    tx.execute(
        "INSERT INTO listing_links
            (property_id, source_platform, adapter_id, source_listing_id, url, normalized_url,
             title, raw_description, location_text,
             price_centavos, price_classification, price_confidence, currency,
             extracted_facts_json, parser_version, listing_status, status_changed_at,
             extraction_status, extraction_error,
             first_seen_at, last_seen_at, last_verified_at, last_scraped_at, source_fields_json)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, 'BRL', ?13, ?14, ?15, ?16, ?17, ?18,
                 ?19, ?20, ?21, ?22, ?23)",
        params![
            property_id,
            l.source.platform,
            l.source.adapter_id,
            l.source.source_listing_id,
            l.url,
            l.normalized_url,
            l.title,
            l.description,
            l.location_text,
            centavos,
            classification,
            confidence,
            p.facts_json,
            l.facts.parser_version,
            l.status.as_str(),
            format_timestamp(l.last_seen_at.max(l.first_seen_at)),
            l.extraction_status.as_str(),
            l.extraction_error,
            format_timestamp(l.first_seen_at),
            format_timestamp(l.last_seen_at.max(l.first_seen_at)),
            l.last_verified_at.map(format_timestamp),
            l.last_scraped_at.map(format_timestamp),
            p.source_fields_json,
        ],
    )?;
    let listing_id = tx.last_insert_rowid();
    let price = match &p.price_columns {
        PriceColumns::Set(mention) => Some(record_price(tx, listing_id, l, mention)?),
        _ => None,
    };
    Ok((listing_id, property_id, price))
}

/// A repeat sighting: updates the listing in place under the protective policies of the module
/// docs. Returns whether it reappeared and what happened to the price history.
fn update_existing(
    tx: &Connection,
    p: &Prepared<'_>,
    id: i64,
    previous_status: &str,
) -> Result<(bool, Option<RecordOutcome>)> {
    let l = p.listing;
    // Price first: if the recorder refuses (e.g. an out-of-order observation) nothing else has
    // been touched, and the transaction rolls back regardless.
    let reappeared =
        previous_status == ListingStatus::Unavailable.as_str() && l.status == ListingStatus::Active;
    let mut price = None;
    if p.failed {
        tx.execute(
            "UPDATE listing_links
                SET extraction_status = ?1, extraction_error = ?2,
                    last_seen_at = MAX(last_seen_at, ?3),
                    last_scraped_at = MAX(COALESCE(last_scraped_at, ''), ?4)
              WHERE id = ?5",
            params![
                l.extraction_status.as_str(),
                l.extraction_error,
                format_timestamp(l.last_seen_at),
                format_timestamp(p.scraped_at),
                id
            ],
        )?;
    } else {
        if let PriceColumns::Set(mention) = &p.price_columns {
            price = Some(record_price(tx, id, l, mention)?);
        }
        let (centavos, classification, confidence) = price_values(&p.price_columns);
        let touches_price = !matches!(p.price_columns, PriceColumns::Keep);
        tx.execute(
            "UPDATE listing_links
                SET title = ?1, raw_description = ?2, location_text = ?3,
                    price_centavos = CASE WHEN ?4 THEN ?5 ELSE price_centavos END,
                    price_classification = CASE WHEN ?4 THEN ?6 ELSE price_classification END,
                    price_confidence = CASE WHEN ?4 THEN ?7 ELSE price_confidence END,
                    extracted_facts_json = ?8, parser_version = ?9,
                    listing_status = ?10,
                    status_changed_at = CASE WHEN listing_status <> ?10 THEN ?11 ELSE status_changed_at END,
                    status_reason = CASE WHEN listing_status <> ?10 THEN NULL ELSE status_reason END,
                    extraction_status = ?12, extraction_error = ?13,
                    last_seen_at = MAX(last_seen_at, ?11),
                    last_verified_at = COALESCE(?14, last_verified_at),
                    last_scraped_at = MAX(COALESCE(last_scraped_at, ''), ?15),
                    source_fields_json = COALESCE(?16, source_fields_json)
              WHERE id = ?17",
            params![
                l.title,
                l.description,
                l.location_text,
                touches_price,
                centavos,
                classification,
                confidence,
                p.facts_json,
                l.facts.parser_version,
                l.status.as_str(),
                format_timestamp(l.last_seen_at),
                l.extraction_status.as_str(),
                l.extraction_error,
                l.last_verified_at.map(format_timestamp),
                format_timestamp(p.scraped_at),
                p.source_fields_json,
                id
            ],
        )?;
    }
    Ok((reappeared, price))
}

fn price_values(columns: &PriceColumns<'_>) -> (Option<i64>, &'static str, Option<&'static str>) {
    match columns {
        PriceColumns::Set(m) => (
            Some(m.price.money().centavos()),
            m.price.classification().as_str(),
            Some(m.confidence.as_str()),
        ),
        PriceColumns::Keep | PriceColumns::Clear => {
            (None, PriceClassification::Unknown.as_str(), None)
        }
    }
}

fn record_price(
    tx: &Connection,
    listing_id: i64,
    l: &NormalizedListing,
    mention: &PriceMention,
) -> Result<RecordOutcome> {
    record_observation_in(
        tx,
        listing_id,
        &PriceObservation {
            amount: mention.price.money(),
            classification: mention.price.classification(),
            confidence: Some(mention.confidence),
            observed_at: l.last_scraped_at.unwrap_or(l.last_seen_at),
            provenance: PriceProvenance {
                extractor: l.facts.parser_version.clone(),
                source_field: mention.evidence.field.clone(),
                evidence: Some(mention.evidence.clone()),
            },
        },
    )
}

/// Marks a listing unavailable on explicit evidence (e.g. the source answered 404/410). Never
/// deletes anything. Returns `false` when it was already unavailable (or does not exist).
pub fn mark_unavailable(
    conn: &mut Connection,
    listing_id: i64,
    at: DateTime<Utc>,
    reason: &str,
) -> Result<bool> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let changed = tx.execute(
        "UPDATE listing_links
            SET listing_status = 'unavailable', status_changed_at = ?1, status_reason = ?2
          WHERE id = ?3 AND listing_status <> 'unavailable'",
        params![format_timestamp(at), reason, listing_id],
    )?;
    tx.commit()?;
    Ok(changed == 1)
}

const COLUMNS: &str = "id, property_id, source_platform, adapter_id, source_listing_id, url,
    normalized_url, title, raw_description, location_text, price_centavos, price_classification,
    price_confidence, extracted_facts_json, parser_version, listing_status, extraction_status,
    extraction_error, first_seen_at, last_seen_at, last_verified_at, last_scraped_at,
    status_changed_at, status_reason, source_fields_json";

/// Raw column values, converted to [`StoredListing`] outside the row callback so conversion
/// errors can use the crate error type.
struct RawListing {
    id: i64,
    property_id: Option<i64>,
    source_platform: String,
    adapter_id: String,
    source_listing_id: Option<String>,
    url: String,
    normalized_url: String,
    title: Option<String>,
    raw_description: Option<String>,
    location_text: Option<String>,
    price_centavos: Option<i64>,
    price_classification: String,
    price_confidence: Option<String>,
    facts_json: Option<String>,
    parser_version: Option<String>,
    listing_status: String,
    extraction_status: String,
    extraction_error: Option<String>,
    first_seen_at: String,
    last_seen_at: String,
    last_verified_at: Option<String>,
    last_scraped_at: Option<String>,
    status_changed_at: Option<String>,
    status_reason: Option<String>,
    source_fields_json: Option<String>,
}

fn raw_from_row(r: &Row<'_>) -> rusqlite::Result<RawListing> {
    Ok(RawListing {
        id: r.get(0)?,
        property_id: r.get(1)?,
        source_platform: r.get(2)?,
        adapter_id: r.get(3)?,
        source_listing_id: r.get(4)?,
        url: r.get(5)?,
        normalized_url: r.get(6)?,
        title: r.get(7)?,
        raw_description: r.get(8)?,
        location_text: r.get(9)?,
        price_centavos: r.get(10)?,
        price_classification: r.get(11)?,
        price_confidence: r.get(12)?,
        facts_json: r.get(13)?,
        parser_version: r.get(14)?,
        listing_status: r.get(15)?,
        extraction_status: r.get(16)?,
        extraction_error: r.get(17)?,
        first_seen_at: r.get(18)?,
        last_seen_at: r.get(19)?,
        last_verified_at: r.get(20)?,
        last_scraped_at: r.get(21)?,
        status_changed_at: r.get(22)?,
        status_reason: r.get(23)?,
        source_fields_json: r.get(24)?,
    })
}

fn parse_enum<T: std::str::FromStr>(column: &'static str, text: &str) -> Result<T> {
    text.parse().map_err(|_| Error::InvalidStoredValue {
        column,
        value: text.to_owned(),
    })
}

fn stored_from_raw(raw: RawListing) -> Result<StoredListing> {
    let price = match (raw.price_centavos, raw.price_confidence.as_deref()) {
        (Some(centavos), Some(confidence)) => Some(StoredPrice {
            amount: Money::from_centavos(centavos)?,
            classification: parse_enum(
                "listing_links.price_classification",
                &raw.price_classification,
            )?,
            confidence: parse_enum("listing_links.price_confidence", confidence)?,
        }),
        _ => None,
    };
    let optional_ts = |value: Option<String>| value.as_deref().map(parse_timestamp).transpose();
    Ok(StoredListing {
        id: raw.id,
        property_id: raw.property_id,
        source_platform: raw.source_platform,
        adapter_id: raw.adapter_id,
        source_listing_id: raw.source_listing_id,
        url: raw.url,
        normalized_url: raw.normalized_url,
        title: raw.title,
        raw_description: raw.raw_description,
        location_text: raw.location_text,
        price,
        facts: raw
            .facts_json
            .as_deref()
            .map(serde_json::from_str)
            .transpose()?,
        parser_version: raw.parser_version,
        status: parse_enum("listing_links.listing_status", &raw.listing_status)?,
        extraction_status: parse_enum("listing_links.extraction_status", &raw.extraction_status)?,
        extraction_error: raw.extraction_error,
        first_seen_at: parse_timestamp(&raw.first_seen_at)?,
        last_seen_at: parse_timestamp(&raw.last_seen_at)?,
        last_verified_at: optional_ts(raw.last_verified_at)?,
        last_scraped_at: optional_ts(raw.last_scraped_at)?,
        status_changed_at: optional_ts(raw.status_changed_at)?,
        status_reason: raw.status_reason,
        source_fields: raw
            .source_fields_json
            .as_deref()
            .map(serde_json::from_str)
            .transpose()?,
    })
}

pub fn get_listing(conn: &Connection, id: i64) -> Result<Option<StoredListing>> {
    conn.query_row(
        &format!("SELECT {COLUMNS} FROM listing_links WHERE id = ?1"),
        [id],
        raw_from_row,
    )
    .optional()?
    .map(stored_from_raw)
    .transpose()
}

pub fn find_listing(
    conn: &Connection,
    adapter_id: &str,
    normalized_url: &str,
) -> Result<Option<StoredListing>> {
    conn.query_row(
        &format!(
            "SELECT {COLUMNS} FROM listing_links WHERE adapter_id = ?1 AND normalized_url = ?2"
        ),
        params![adapter_id, normalized_url],
        raw_from_row,
    )
    .optional()?
    .map(stored_from_raw)
    .transpose()
}

/// Listings whose own `property_id` is `property_id` (not those of properties merged into it).
pub fn listings_of_property(conn: &Connection, property_id: i64) -> Result<Vec<StoredListing>> {
    let mut statement = conn.prepare(&format!(
        "SELECT {COLUMNS} FROM listing_links WHERE property_id = ?1 ORDER BY id"
    ))?;
    let raws = statement
        .query_map([property_id], raw_from_row)?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    raws.into_iter().map(stored_from_raw).collect()
}

#[cfg(test)]
mod tests;
