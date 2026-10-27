//! Source collection: the adapter contract and the machinery around it.
//!
//! Adapters describe how one platform is accessed *by a permitted method*; everything else
//! (rate limits, bounded concurrency, retries, size limits, normalisation, persistence, outcome
//! reporting) is source-independent and lives here. No real adapter exists yet: see
//! `docs/sources/` for why none of the evaluated candidates qualifies.

pub mod adapter;
pub mod cancel;
pub mod config;
pub mod error;
pub mod fetch;
pub mod normalize;
pub mod pipeline;
#[cfg(test)]
pub(crate) mod testserver;
pub mod url_norm;

pub use adapter::{Discovery, ListingRef, RawListing, SearchRequest, SourceAdapter};
pub use cancel::CancelToken;
pub use config::SourceConfig;
pub use error::SourceError;
pub use fetch::{Fetched, Fetcher};

use std::sync::Arc;

/// The sources the application collects from.
///
/// **Empty on purpose.** None of the evaluated candidates (62imoveis, OLX, ZAP Imóveis, local
/// brokerages) currently permits automated collection by a method this application can use; the
/// dated evidence is in `docs/sources/`. An adapter is added here only after its evaluation
/// record says it may be used. Until then, starting a collection fails with
/// `no_sources_configured` instead of pretending to search.
#[must_use]
pub fn registered_adapters() -> Vec<Arc<dyn SourceAdapter>> {
    Vec::new()
}

#[cfg(test)]
mod registry_tests {
    #[test]
    fn no_adapter_is_registered_until_a_source_is_permitted() {
        // When a real adapter is added, update this test together with `docs/sources/`.
        assert_eq!(super::registered_adapters().len(), 0);
    }
}
