//! Turns what an adapter returned into a [`NormalizedListing`] using the shared parser.
//!
//! Title and description are joined with a newline into one text that the parser reads; that
//! exact text is what gets stored as `raw_description`, so every evidence span (field `text`)
//! can be resolved against it later. The structured price field (field `price`) is passed to the
//! parser as a hint carrying the **source's calibrated confidence**, which stays `Low` until the
//! source has been measured.

use chrono::{DateTime, Utc};
use serde_json::{Map, Value, json};

use super::{
    SourceConfig, SourceError,
    adapter::RawListing,
    url_norm::{normalize_url, parse_http_url},
};
use crate::{
    models::{
        listing::{ExtractionStatus, ListingStatus, NormalizedListing, SourceRef},
        money::Currency,
    },
    parser::{
        PriceFieldHint, StructuredText, parse_listing_with_price_hint, parse_structured_text,
    },
};

/// Evidence field names used by the shared normalisation.
pub const TEXT_FIELD: &str = "text";
pub const PRICE_FIELD: &str = "price";

/// A listing ready to store, with the raw structured fields to store alongside it.
#[derive(Debug, Clone, PartialEq)]
pub struct Normalized {
    pub listing: NormalizedListing,
    /// Raw structured fields as the adapter gave them (resolves non-`text` evidence spans).
    pub source_fields: Value,
    pub image_urls: Vec<String>,
}

fn clean(value: Option<&String>) -> Option<&str> {
    value.map(|s| s.trim()).filter(|s| !s.is_empty())
}

pub fn normalize(
    adapter_id: &str,
    platform: &str,
    config: &SourceConfig,
    raw: &RawListing,
    now: DateTime<Utc>,
) -> Result<Normalized, SourceError> {
    let normalized_url = normalize_url(&raw.url, &config.ignored_url_params)
        .map_err(|e| SourceError::Disallowed(e.to_string()))?;
    let host = parse_http_url(&normalized_url)
        .map_err(|e| SourceError::Disallowed(e.to_string()))?
        .host_str()
        .unwrap_or_default()
        .to_owned();
    if !config.host_allowed(&host) {
        return Err(SourceError::Disallowed(format!(
            "listing host {host:?} is not allowed for this source"
        )));
    }

    let text = [clean(raw.title.as_ref()), clean(raw.description.as_ref())]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join("\n");
    let price_text = clean(raw.price_text.as_ref());
    if text.is_empty() && price_text.is_none() {
        return Err(SourceError::Unparseable(
            "the page has no readable text or price".to_owned(),
        ));
    }

    let body = StructuredText {
        field: TEXT_FIELD,
        text: &text,
    };
    let facts = match price_text {
        Some(price) => parse_listing_with_price_hint(
            body,
            &PriceFieldHint {
                field: StructuredText {
                    field: PRICE_FIELD,
                    text: price,
                },
                confidence: config.price_field_confidence,
            },
        ),
        None => parse_structured_text(body),
    };

    let mut problems = Vec::new();
    if !raw.extraction.missing_fields.is_empty() {
        problems.push(format!(
            "missing fields: {}",
            raw.extraction.missing_fields.join(", ")
        ));
    }
    problems.extend(raw.extraction.warnings.iter().cloned());
    if facts.prices.is_empty() {
        problems.push("no price found".to_owned());
    }
    let extraction_status = if problems.is_empty() {
        ExtractionStatus::Ok
    } else {
        ExtractionStatus::Partial
    };

    let mut fields = Map::new();
    let mut put = |key: &str, value: Option<&String>| {
        if let Some(v) = value {
            fields.insert(key.to_owned(), json!(v));
        }
    };
    put("title", raw.title.as_ref());
    put("description", raw.description.as_ref());
    put("location", raw.location_text.as_ref());
    put("price", raw.price_text.as_ref());
    if !raw.extra_fields.is_empty() {
        fields.insert("extra".to_owned(), json!(raw.extra_fields));
    }
    if !raw.image_urls.is_empty() {
        fields.insert("image_urls".to_owned(), json!(raw.image_urls));
    }

    let listing = NormalizedListing {
        source: SourceRef {
            platform: platform.to_owned(),
            adapter_id: adapter_id.to_owned(),
            source_listing_id: raw.source_listing_id.clone(),
        },
        url: raw.url.clone(),
        normalized_url,
        title: raw.title.clone(),
        description: (!text.is_empty()).then_some(text),
        location_text: raw.location_text.clone(),
        currency: Currency::Brl,
        facts,
        status: ListingStatus::Active,
        extraction_status,
        extraction_error: (!problems.is_empty()).then(|| problems.join("; ")),
        first_seen_at: now,
        last_seen_at: now,
        // Only a complete, clean read counts as a verification of the listing.
        last_verified_at: (extraction_status == ExtractionStatus::Ok).then_some(now),
        last_scraped_at: Some(now),
    };
    Ok(Normalized {
        listing,
        source_fields: Value::Object(fields),
        image_urls: raw.image_urls.clone(),
    })
}

#[cfg(test)]
mod tests;
