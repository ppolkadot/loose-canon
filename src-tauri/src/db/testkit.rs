//! Helpers shared by the repository tests.

use chrono::{DateTime, TimeZone, Utc};

use crate::{
    db::Database,
    models::{
        listing::{ExtractionStatus, ListingStatus, NormalizedListing, SourceRef},
        money::Currency,
    },
    parser::parse_listing_text,
};

pub fn mem() -> Database {
    Database::open_in_memory().expect("in-memory database")
}

/// A fixed epoch plus `secs`, so tests control "when" exactly.
pub fn at(secs: i64) -> DateTime<Utc> {
    Utc.timestamp_opt(1_780_000_000 + secs, 0).unwrap()
}

/// A successfully parsed, active listing scraped at `at(secs)`.
pub fn normalized(adapter: &str, url: &str, text: &str, secs: i64) -> NormalizedListing {
    NormalizedListing {
        source: SourceRef {
            platform: adapter.to_uppercase(),
            adapter_id: adapter.to_owned(),
            source_listing_id: None,
        },
        url: url.to_owned(),
        normalized_url: url.to_owned(),
        title: Some("Casa".to_owned()),
        description: Some(text.to_owned()),
        location_text: None,
        currency: Currency::Brl,
        facts: parse_listing_text(text),
        status: ListingStatus::Active,
        extraction_status: ExtractionStatus::Ok,
        extraction_error: None,
        first_seen_at: at(secs),
        last_seen_at: at(secs),
        last_verified_at: Some(at(secs)),
        last_scraped_at: Some(at(secs)),
    }
}
