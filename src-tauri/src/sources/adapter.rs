//! The contract between the shared pipeline and one platform's adapter.
//!
//! An adapter knows *how one source may be accessed by a permitted method* and nothing else.
//! It returns what the source said, field by field and verbatim; classification of prices,
//! extraction of rooms and areas, persistence, rate limiting and outcome reporting are all done
//! by shared code so every source is judged by the same rules.

use std::{collections::BTreeMap, future::Future, pin::Pin};

use serde::{Deserialize, Serialize};

use super::{CancelToken, Fetcher, SourceConfig, SourceError};
use crate::models::money::Money;

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// What the user asked for. Stored with the run, so it must stay serialisable.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchRequest {
    /// Region slugs (see `regions`); empty means "everything the source offers".
    pub region_slugs: Vec<String>,
    pub min_price: Option<Money>,
    pub max_price: Option<Money>,
    /// Upper bound on listings per source for this run (never above the source's own cap).
    pub max_listings: Option<u32>,
}

/// A pointer to one advertisement found during discovery.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListingRef {
    pub url: String,
    /// The platform's own identifier, when it exposes one. Not assumed globally unique.
    pub source_listing_id: Option<String>,
}

/// Result of discovery. `complete` is `false` whenever the adapter knows it did not see
/// everything (truncated pages, rate limit, partial failure); the reason goes in `warnings`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Discovery {
    pub refs: Vec<ListingRef>,
    pub complete: bool,
    pub warnings: Vec<String>,
}

/// How well the adapter could read one page.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExtractionReport {
    pub warnings: Vec<String>,
    /// Fields the page normally has but this one did not (e.g. `price`, `description`).
    pub missing_fields: Vec<String>,
}

/// One advertisement exactly as the source presented it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RawListing {
    /// Original URL of the advertisement.
    pub url: String,
    pub source_listing_id: Option<String>,
    pub title: Option<String>,
    pub description: Option<String>,
    pub location_text: Option<String>,
    /// The platform's structured asking-price field, verbatim (`R$ 350.000`).
    pub price_text: Option<String>,
    /// Every other structured field, name -> raw text.
    pub extra_fields: BTreeMap<String, String>,
    pub image_urls: Vec<String>,
    pub extraction: ExtractionReport,
}

pub trait SourceAdapter: Send + Sync {
    /// Stable identifier stored with every listing (`listing_links.adapter_id`).
    fn id(&self) -> &str;

    /// Human-facing platform name.
    fn platform(&self) -> &str;

    fn config(&self) -> &SourceConfig;

    /// Finds the advertisements matching `request`, using only `fetcher` for network access.
    fn discover<'a>(
        &'a self,
        fetcher: &'a Fetcher,
        request: &'a SearchRequest,
        cancel: &'a CancelToken,
    ) -> BoxFuture<'a, Result<Discovery, SourceError>>;

    /// Retrieves one advertisement. Return [`SourceError::NotFound`] only on explicit evidence
    /// that the advertisement is gone (e.g. HTTP 404/410), and [`SourceError::Unparseable`] when
    /// the page was fetched but cannot be read reliably.
    fn fetch_listing<'a>(
        &'a self,
        fetcher: &'a Fetcher,
        reference: &'a ListingRef,
        cancel: &'a CancelToken,
    ) -> BoxFuture<'a, Result<RawListing, SourceError>>;
}
