//! Collection: sources, starting and cancelling a run, and what past runs did.
//!
//! Progress and completion reach the frontend as the events [`PROGRESS_EVENT`] and
//! [`FINISHED_EVENT`]; both are also recoverable from the saved runs, so a missed event loses
//! nothing.

// Tauri hands `State` and `AppHandle` to commands by value.
#![allow(clippy::needless_pass_by_value)]

use std::sync::Arc;

use serde::Deserialize;
use tauri::{AppHandle, Emitter, State};

use super::validate_slug;
use crate::{
    db::{
        regions::{self, RegionRow},
        runs::{self, RunRecord, RunSummary, StoredOutcome},
    },
    error::{Error, Result},
    models::money::Money,
    service::{AppService, CollectionFinished, FinishedSink, SourceInfo},
    sources::{SearchRequest, pipeline::ProgressSink},
};

pub const PROGRESS_EVENT: &str = "collection-progress";
pub const FINISHED_EVENT: &str = "collection-finished";

/// Most slugs one request may name, and the largest page of history a command returns.
const MAX_REGION_SLUGS: usize = 50;
const MAX_HISTORY: u32 = 200;
const DEFAULT_HISTORY: u32 = 20;

/// What the user asks a collection for. Mirrored by `CollectionRequest` in `src/lib/api.ts`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
pub struct CollectionArgs {
    pub region_slugs: Vec<String>,
    pub min_price_centavos: Option<i64>,
    pub max_price_centavos: Option<i64>,
    pub max_listings: Option<u32>,
}

impl CollectionArgs {
    pub fn into_request(self) -> Result<SearchRequest> {
        let invalid = |detail: &str| Error::InvalidConfig {
            what: "collection request",
            detail: detail.to_owned(),
        };
        if self.region_slugs.len() > MAX_REGION_SLUGS {
            return Err(invalid("too many regions in one request"));
        }
        for slug in &self.region_slugs {
            validate_slug(slug)?;
        }
        let min_price = self
            .min_price_centavos
            .map(Money::from_centavos)
            .transpose()?;
        let max_price = self
            .max_price_centavos
            .map(Money::from_centavos)
            .transpose()?;
        if let (Some(min), Some(max)) = (min_price, max_price)
            && min > max
        {
            return Err(invalid("the minimum price is above the maximum price"));
        }
        if self.max_listings == Some(0) {
            return Err(invalid("max listings must be at least 1"));
        }
        Ok(SearchRequest {
            region_slugs: self.region_slugs,
            min_price,
            max_price,
            max_listings: self.max_listings,
        })
    }
}

fn history_limit(limit: Option<u32>) -> u32 {
    limit.unwrap_or(DEFAULT_HISTORY).clamp(1, MAX_HISTORY)
}

#[tauri::command]
pub async fn list_sources(service: State<'_, Arc<AppService>>) -> Result<Vec<SourceInfo>> {
    service.offload(AppService::sources).await
}

/// Starts a collection in the background. Fails with `no_sources_configured` when no source is
/// registered (nothing is recorded: that is not "searched and found nothing") and with
/// `collection_already_running` when one is running.
#[tauri::command]
pub async fn start_collection(
    app: AppHandle,
    service: State<'_, Arc<AppService>>,
    args: CollectionArgs,
) -> Result<()> {
    let request = args.into_request()?;
    let progress_emitter = app.clone();
    let progress: ProgressSink = Arc::new(move |event| {
        let _ = progress_emitter.emit(PROGRESS_EVENT, &event);
    });
    let finished_emitter = app;
    let finished: FinishedSink = Box::new(move |done: CollectionFinished| {
        let _ = finished_emitter.emit(FINISHED_EVENT, &done);
    });
    service.start_collection(request, Some(progress), Some(finished))
}

/// Returns whether a collection was running (and is now being cancelled).
#[must_use]
#[tauri::command]
pub fn cancel_collection(service: State<'_, Arc<AppService>>) -> bool {
    service.cancel_collection()
}

#[must_use]
#[tauri::command]
pub fn is_collection_running(service: State<'_, Arc<AppService>>) -> bool {
    service.collection_running()
}

/// Newest first.
#[tauri::command]
pub async fn list_collection_runs(
    service: State<'_, Arc<AppService>>,
    limit: Option<u32>,
) -> Result<Vec<RunRecord>> {
    let limit = history_limit(limit);
    service
        .blocking(move |conn| runs::list_runs(conn, limit))
        .await
}

#[tauri::command]
pub async fn get_collection_run(
    service: State<'_, Arc<AppService>>,
    run_id: i64,
) -> Result<Option<RunSummary>> {
    service
        .blocking(move |conn| runs::get_run(conn, run_id))
        .await
}

/// Failed, unreliable or incomplete source outcomes, newest first.
#[tauri::command]
pub async fn list_source_problems(
    service: State<'_, Arc<AppService>>,
    limit: Option<u32>,
) -> Result<Vec<StoredOutcome>> {
    let limit = history_limit(limit);
    service
        .blocking(move |conn| runs::recent_problems(conn, limit))
        .await
}

#[tauri::command]
pub async fn list_regions(
    service: State<'_, Arc<AppService>>,
    only_enabled: Option<bool>,
) -> Result<Vec<RegionRow>> {
    let only_enabled = only_enabled.unwrap_or(true);
    service
        .blocking(move |conn| regions::list_regions(conn, only_enabled))
        .await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(json: &str) -> CollectionArgs {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn an_empty_request_means_everything_the_source_offers() {
        let request = args("{}").into_request().unwrap();
        assert_eq!(request, SearchRequest::default());
    }

    #[test]
    fn the_request_is_read_in_camel_case_and_converted_to_exact_money() {
        let request = args(
            r#"{ "regionSlugs": ["goiania", "br-go-aparecida"], "minPriceCentavos": 30000000,
                 "maxPriceCentavos": 45000000, "maxListings": 40 }"#,
        )
        .into_request()
        .unwrap();
        assert_eq!(request.region_slugs, vec!["goiania", "br-go-aparecida"]);
        assert_eq!(request.min_price.map(Money::centavos), Some(30_000_000));
        assert_eq!(request.max_price.map(Money::centavos), Some(45_000_000));
        assert_eq!(request.max_listings, Some(40));
    }

    #[test]
    fn unknown_fields_are_refused_rather_than_silently_ignored() {
        assert!(serde_json::from_str::<CollectionArgs>(r#"{ "regionSlug": "goiania" }"#).is_err());
    }

    #[test]
    fn contradictory_or_malformed_requests_are_refused() {
        let refused = |json: &str| {
            let error = args(json).into_request().unwrap_err();
            assert!(
                matches!(error, Error::InvalidConfig { .. } | Error::Model(_)),
                "{json}: {error}"
            );
        };
        refused(r#"{ "minPriceCentavos": 50000000, "maxPriceCentavos": 40000000 }"#);
        refused(r#"{ "minPriceCentavos": -1 }"#);
        refused(r#"{ "maxListings": 0 }"#);
        refused(r#"{ "regionSlugs": ["Setor Bueno"] }"#);
        let many: Vec<String> = (0..51).map(|i| format!("r{i}")).collect();
        refused(&format!(
            r#"{{ "regionSlugs": {} }}"#,
            serde_json::to_string(&many).unwrap()
        ));
    }

    #[test]
    fn equal_minimum_and_maximum_prices_are_allowed() {
        assert!(
            args(r#"{ "minPriceCentavos": 100, "maxPriceCentavos": 100 }"#)
                .into_request()
                .is_ok()
        );
    }

    #[test]
    fn history_limits_are_bounded() {
        assert_eq!(history_limit(None), DEFAULT_HISTORY);
        assert_eq!(history_limit(Some(0)), 1);
        assert_eq!(history_limit(Some(5)), 5);
        assert_eq!(history_limit(Some(10_000)), MAX_HISTORY);
    }
}
