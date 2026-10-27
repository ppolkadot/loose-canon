//! One collection run: discovery -> bounded fetch -> normalise -> store, per source, with every
//! source's result recorded as one of four distinct outcomes.
//!
//! Design rules this module upholds:
//! * a source failure is an *outcome*, never an empty result and never a failed run;
//! * network work never holds the database lock (DB calls are short and run via
//!   `spawn_blocking`), and every wait observes cancellation;
//! * a listing is only marked unavailable on explicit evidence (the adapter reporting
//!   `NotFound`), never because it was absent from a search;
//! * partial work is kept: cancelling or rate limiting ends the source early but what was stored
//!   stays, and the outcome says it is incomplete;
//! * an adapter that panics is contained and reported, and cannot take the run down.

use std::{
    collections::{BTreeMap, HashSet},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use chrono::{DateTime, Utc};
use rusqlite::Connection;
use serde::Serialize;
use tokio::{sync::Semaphore, task::JoinSet};

use super::{
    CancelToken, Fetcher, ListingRef, SearchRequest, SourceAdapter, SourceConfig, SourceError,
    normalize::normalize, url_norm::normalize_url,
};
use crate::{
    db::{
        Database,
        images::register_images,
        listings::{ListingWrite, find_listing, mark_unavailable, upsert_listing},
        regions::LocationIndex,
        runs::{finish_run, record_outcome, start_run},
    },
    error::{Error, Result},
    models::{
        collection::{RunStatus, SourceOutcome, SourceOutcomeKind},
        listing::{ExtractionStatus, ListingStatus, NormalizedListing, SourceRef},
        money::Currency,
    },
    parser::parse_listing_text,
};

pub type Clock = Arc<dyn Fn() -> DateTime<Utc> + Send + Sync>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Discovering,
    Fetching,
    Finished,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProgressEvent {
    pub run_id: i64,
    pub adapter_id: String,
    pub phase: Phase,
    pub discovered: u32,
    pub processed: u32,
    pub stored: u32,
}

pub type ProgressSink = Arc<dyn Fn(ProgressEvent) + Send + Sync>;

#[derive(Debug, Clone, PartialEq)]
pub struct RunReport {
    pub run_id: i64,
    pub status: RunStatus,
    pub outcomes: Vec<SourceOutcome>,
}

/// Everything a run needs besides the adapters.
#[derive(Clone)]
pub struct CollectionDeps {
    pub db: Arc<Database>,
    /// Bounds simultaneous requests across **all** sources.
    pub global_limit: Arc<Semaphore>,
    pub clock: Clock,
}

impl CollectionDeps {
    /// Default global bound on simultaneous requests.
    pub const DEFAULT_GLOBAL_CONCURRENCY: usize = 4;

    #[must_use]
    pub fn new(db: Arc<Database>) -> Self {
        Self {
            db,
            global_limit: Arc::new(Semaphore::new(Self::DEFAULT_GLOBAL_CONCURRENCY)),
            clock: Arc::new(Utc::now),
        }
    }
}

/// Runs a blocking database closure off the async workers.
pub(crate) async fn with_db<T: Send + 'static>(
    db: &Arc<Database>,
    f: impl FnOnce(&mut Connection) -> Result<T> + Send + 'static,
) -> Result<T> {
    let db = Arc::clone(db);
    tokio::task::spawn_blocking(move || db.with_conn(f))
        .await
        .map_err(|e| Error::Io(std::io::Error::other(format!("database task failed: {e}"))))?
}

/// Collects from every adapter and records the run. See the module docs for the guarantees.
///
/// Fails *before* creating a run when there are no adapters (that must never look like "searched
/// and found nothing") or when adapter ids collide.
pub async fn run_collection(
    deps: &CollectionDeps,
    adapters: &[Arc<dyn SourceAdapter>],
    request: &SearchRequest,
    cancel: &CancelToken,
    progress: Option<ProgressSink>,
) -> Result<RunReport> {
    if adapters.is_empty() {
        return Err(Error::NoSourcesConfigured);
    }
    let mut ids = HashSet::new();
    for adapter in adapters {
        if !ids.insert(adapter.id().to_owned()) {
            return Err(Error::InvalidConfig {
                what: "source registry",
                detail: format!("duplicate adapter id {:?}", adapter.id()),
            });
        }
    }

    let started = (deps.clock)();
    let request_json = serde_json::to_value(request)?;
    let run_id = with_db(&deps.db, move |c| start_run(c, &request_json, started)).await?;
    let locations = Arc::new(with_db(&deps.db, |c| LocationIndex::load(c)).await?);

    let mut tasks = JoinSet::new();
    for (position, adapter) in adapters.iter().enumerate() {
        let source = SourceRun {
            deps: deps.clone(),
            adapter: Arc::clone(adapter),
            request: request.clone(),
            cancel: cancel.clone(),
            run_id,
            progress: progress.clone(),
            locations: Arc::clone(&locations),
        };
        let id = adapter.id().to_owned();
        let clock = Arc::clone(&deps.clock);
        tasks.spawn(async move {
            let began = clock();
            // The adapter's own code runs in a nested task so that even a panic inside
            // `discover` is caught and reported against the right source.
            let outcome = match tokio::spawn(run_source(source)).await {
                Ok(outcome) => outcome,
                Err(e) => failed_outcome(
                    &id,
                    began,
                    clock(),
                    "internal_error",
                    &format!("the source's collection task did not finish: {e}"),
                    Vec::new(),
                ),
            };
            (position, outcome)
        });
    }
    let mut collected: Vec<(usize, SourceOutcome)> = Vec::with_capacity(adapters.len());
    while let Some(joined) = tasks.join_next().await {
        match joined {
            Ok(pair) => collected.push(pair),
            Err(e) => {
                // The wrapper itself failed; the run cannot be trusted.
                let at = (deps.clock)();
                let note = format!("collection aborted: {e}");
                let _ = with_db(&deps.db, move |c| {
                    finish_run(c, run_id, RunStatus::Failed, at, Some(&note))
                })
                .await;
                return Err(Error::Io(std::io::Error::other("collection task failed")));
            }
        }
    }
    collected.sort_by_key(|(position, _)| *position);
    let outcomes: Vec<SourceOutcome> = collected.into_iter().map(|(_, o)| o).collect();

    for outcome in &outcomes {
        let outcome = outcome.clone();
        with_db(&deps.db, move |c| record_outcome(c, run_id, &outcome)).await?;
    }
    let status = if cancel.is_cancelled() {
        RunStatus::Cancelled
    } else {
        RunStatus::Completed
    };
    let finished = (deps.clock)();
    with_db(&deps.db, move |c| {
        finish_run(c, run_id, status, finished, None)
    })
    .await?;
    Ok(RunReport {
        run_id,
        status,
        outcomes,
    })
}

struct SourceRun {
    deps: CollectionDeps,
    adapter: Arc<dyn SourceAdapter>,
    request: SearchRequest,
    cancel: CancelToken,
    run_id: i64,
    progress: Option<ProgressSink>,
    locations: Arc<LocationIndex>,
}

fn failed_outcome(
    adapter_id: &str,
    started_at: DateTime<Utc>,
    finished_at: DateTime<Utc>,
    error_kind: &str,
    detail: &str,
    warnings: Vec<String>,
) -> SourceOutcome {
    SourceOutcome {
        adapter_id: adapter_id.to_owned(),
        kind: SourceOutcomeKind::Failed,
        complete: false,
        discovered: 0,
        stored: 0,
        unparseable: 0,
        failed: 0,
        skipped: 0,
        warnings,
        error_kind: Some(error_kind.to_owned()),
        error_detail: Some(detail.to_owned()),
        started_at,
        finished_at: finished_at.max(started_at),
    }
}

/// What happened to one referenced listing.
enum Item {
    Stored,
    /// The adapter reported the listing gone; `marked` is whether a stored listing was updated.
    Gone {
        marked: bool,
    },
    Unreadable(String),
    Failed {
        kind: &'static str,
        detail: String,
    },
    Skipped(String),
    RateLimited,
    /// Not attempted because the run was cancelled or the source rate-limited us.
    NotAttempted,
    Cancelled,
}

struct Shared {
    deps: CollectionDeps,
    adapter: Arc<dyn SourceAdapter>,
    fetcher: Fetcher,
    cancel: CancelToken,
    locations: Arc<LocationIndex>,
    permits: Semaphore,
    rate_limited: AtomicBool,
}

/// The listings to fetch, after de-duplication and the per-run cap.
struct Plan {
    work: Vec<(ListingRef, String)>,
    skipped: u32,
    truncated: bool,
    warnings: Vec<String>,
}

fn plan_work(
    refs: Vec<ListingRef>,
    max_listings: Option<u32>,
    config: &SourceConfig,
    mut warnings: Vec<String>,
) -> Plan {
    let mut skipped = 0_u32;
    let cap = max_listings
        .map_or(config.max_listings_per_run, |n| {
            (n as usize).min(config.max_listings_per_run)
        })
        .max(1);
    let mut seen = HashSet::new();
    let mut work: Vec<(ListingRef, String)> = Vec::new();
    for reference in refs {
        match normalize_url(&reference.url, &config.ignored_url_params) {
            Ok(normalized) => {
                if seen.insert(normalized.clone()) {
                    work.push((reference, normalized));
                }
            }
            Err(e) => {
                skipped += 1;
                warnings.push(format!(
                    "skipped a listing reference ({}): {e}",
                    reference.url.chars().take(60).collect::<String>()
                ));
            }
        }
    }
    let truncated = work.len() > cap;
    if truncated {
        work.truncate(cap);
        warnings.push(format!(
            "the per-run listing limit of {cap} was reached; the remaining listings were not fetched"
        ));
    }
    Plan {
        work,
        skipped,
        truncated,
        warnings,
    }
}

/// The outcome when discovery left nothing to fetch.
fn empty_outcome(
    adapter_id: &str,
    started: DateTime<Utc>,
    finished: DateTime<Utc>,
    discovery_complete: bool,
    skipped: u32,
    warnings: Vec<String>,
) -> SourceOutcome {
    let (kind, complete, error_kind) = if skipped > 0 {
        (SourceOutcomeKind::Unreliable, false, None)
    } else if discovery_complete {
        (SourceOutcomeKind::SuccessfulEmpty, true, None)
    } else {
        // The search did not finish and found nothing: that is not "no results".
        (
            SourceOutcomeKind::Failed,
            false,
            Some("incomplete_discovery".to_owned()),
        )
    };
    let detail = error_kind
        .as_ref()
        .map(|_| "the search was incomplete and discovered nothing".to_owned());
    SourceOutcome {
        adapter_id: adapter_id.to_owned(),
        kind,
        complete,
        discovered: 0,
        stored: 0,
        unparseable: 0,
        failed: 0,
        skipped,
        warnings,
        error_kind,
        error_detail: detail,
        started_at: started,
        finished_at: finished.max(started),
    }
}

struct Verdict {
    kind: SourceOutcomeKind,
    complete: bool,
    error_kind: Option<String>,
    error_detail: Option<String>,
}

/// What the listing tasks of one source reported, and the messages gathered meanwhile.
#[derive(Default)]
struct Tally {
    stored: u32,
    failed: u32,
    unparseable: u32,
    gone: u32,
    skipped: u32,
    rate_limited: bool,
    cancelled: bool,
    errors: BTreeMap<String, (u32, String)>,
    first_unreadable: Option<String>,
    warnings: Vec<String>,
}

impl Tally {
    fn record_error(&mut self, kind: &str, detail: String) {
        self.errors.entry(kind.to_owned()).or_insert((0, detail)).0 += 1;
    }

    fn absorb(&mut self, joined: std::result::Result<Item, tokio::task::JoinError>) {
        match joined {
            Ok(Item::Stored) => self.stored += 1,
            Ok(Item::Gone { marked }) => {
                self.skipped += 1;
                if marked {
                    self.gone += 1;
                }
            }
            Ok(Item::Unreadable(message)) => {
                self.unparseable += 1;
                self.first_unreadable.get_or_insert(message);
            }
            Ok(Item::Failed { kind, detail }) => {
                self.failed += 1;
                self.record_error(kind, detail);
            }
            Ok(Item::Skipped(why)) => {
                self.skipped += 1;
                self.warnings.push(why);
            }
            Ok(Item::RateLimited) => self.rate_limited = true,
            Ok(Item::NotAttempted) => {}
            Ok(Item::Cancelled) => self.cancelled = true,
            Err(e) => {
                self.failed += 1;
                self.record_error(
                    "internal_error",
                    format!("a listing task did not finish: {e}"),
                );
            }
        }
    }

    /// Adds the end-of-run warnings (call once, after every task has finished).
    fn summarize(&mut self) {
        if self.unparseable > 0 {
            self.warnings.push(format!(
                "{} listing(s) unreadable (unparseable): {}",
                self.unparseable,
                self.first_unreadable.take().unwrap_or_default()
            ));
        }
        if self.failed > 0 {
            let summary: Vec<String> = self
                .errors
                .iter()
                .map(|(k, (n, _))| format!("{k} x{n}"))
                .collect();
            self.warnings.push(format!(
                "{} listing(s) could not be fetched ({})",
                self.failed,
                summary.join(", ")
            ));
        }
        if self.gone > 0 {
            self.warnings.push(format!(
                "{} listing(s) confirmed gone at the source and marked unavailable",
                self.gone
            ));
        }
        if self.rate_limited {
            self.warnings
                .push("rate limited by the source; collection from it stopped early".to_owned());
        }
        if self.cancelled {
            self.warnings
                .push("cancelled before every listing was processed".to_owned());
        }
    }

    /// The outcome when listings were attempted but none was stored.
    fn nothing_stored(&self) -> (SourceOutcomeKind, Option<String>, Option<String>) {
        if self.unparseable > 0 && self.failed == 0 && !self.rate_limited && !self.cancelled {
            return (SourceOutcomeKind::Unreliable, None, None);
        }
        let dominant = self
            .errors
            .iter()
            .max_by_key(|(k, (n, _))| (*n, std::cmp::Reverse((*k).clone())));
        match dominant {
            Some((k, (_, detail))) => (
                SourceOutcomeKind::Failed,
                Some(k.clone()),
                Some(detail.clone()),
            ),
            None if self.rate_limited => (
                SourceOutcomeKind::Failed,
                Some("rate_limited".to_owned()),
                Some("the source rate-limited the request".to_owned()),
            ),
            None => (
                SourceOutcomeKind::Failed,
                Some("cancelled".to_owned()),
                Some("the run was cancelled before anything was stored".to_owned()),
            ),
        }
    }

    fn into_outcome(
        self,
        adapter_id: &str,
        verdict: Verdict,
        discovered: u32,
        started_at: DateTime<Utc>,
        finished_at: DateTime<Utc>,
    ) -> SourceOutcome {
        SourceOutcome {
            adapter_id: adapter_id.to_owned(),
            kind: verdict.kind,
            complete: verdict.complete,
            discovered,
            stored: self.stored,
            unparseable: self.unparseable,
            failed: self.failed,
            skipped: self.skipped,
            warnings: self.warnings,
            error_kind: verdict.error_kind,
            error_detail: verdict.error_detail,
            started_at,
            finished_at,
        }
    }

    fn verdict(&self, discovery_complete: bool, truncated: bool, discovered: u32) -> Verdict {
        let (stored, failed, unparseable) = (self.stored, self.failed, self.unparseable);
        let complete = discovery_complete
            && !truncated
            && !self.rate_limited
            && !self.cancelled
            && failed == 0
            && unparseable == 0;
        let (kind, error_kind, error_detail) = if stored == 0
            && (failed > 0 || unparseable > 0 || self.rate_limited || self.cancelled)
        {
            self.nothing_stored()
        } else if stored > 0 && (failed + unparseable) * 2 > discovered {
            (SourceOutcomeKind::Unreliable, None, None)
        } else {
            (SourceOutcomeKind::Results, None, None)
        };
        Verdict {
            kind,
            complete,
            error_kind,
            error_detail,
        }
    }
}

async fn run_source(source: SourceRun) -> SourceOutcome {
    let SourceRun {
        deps,
        adapter,
        request,
        cancel,
        run_id,
        progress,
        locations,
    } = source;
    let adapter_id = adapter.id().to_owned();
    let started = (deps.clock)();
    let config = adapter.config().clone();

    let emit = |phase: Phase, discovered: u32, processed: u32, stored: u32| {
        if let Some(sink) = &progress {
            sink(ProgressEvent {
                run_id,
                adapter_id: adapter_id.clone(),
                phase,
                discovered,
                processed,
                stored,
            });
        }
    };
    let fail = |kind: &str, detail: &str, warnings: Vec<String>| {
        failed_outcome(&adapter_id, started, (deps.clock)(), kind, detail, warnings)
    };

    let fetcher = match Fetcher::new(config.clone(), Arc::clone(&deps.global_limit)) {
        Ok(f) => f,
        Err(e) => return fail("invalid_config", &e.to_string(), Vec::new()),
    };
    emit(Phase::Discovering, 0, 0, 0);
    let discovery = match adapter.discover(&fetcher, &request, &cancel).await {
        Ok(d) => d,
        Err(e) => return fail(e.kind(), &e.to_string(), Vec::new()),
    };

    let Plan {
        work,
        skipped,
        truncated,
        warnings,
    } = plan_work(
        discovery.refs,
        request.max_listings,
        &config,
        discovery.warnings,
    );
    let discovered = u32::try_from(work.len()).unwrap_or(u32::MAX);
    emit(Phase::Fetching, discovered, 0, 0);

    if work.is_empty() {
        emit(Phase::Finished, 0, 0, 0);
        return empty_outcome(
            &adapter_id,
            started,
            (deps.clock)(),
            discovery.complete,
            skipped,
            warnings,
        );
    }

    let shared = Arc::new(Shared {
        deps: deps.clone(),
        adapter: Arc::clone(&adapter),
        fetcher,
        cancel: cancel.clone(),
        locations,
        permits: Semaphore::new(config.max_concurrency),
        rate_limited: AtomicBool::new(false),
    });
    let mut tasks = JoinSet::new();
    for (reference, normalized_url) in work {
        let shared = Arc::clone(&shared);
        tasks.spawn(process_one(shared, reference, normalized_url));
    }

    let mut tally = Tally {
        skipped,
        warnings,
        ..Tally::default()
    };
    let mut processed = 0_u32;
    while let Some(joined) = tasks.join_next().await {
        tally.absorb(joined);
        processed += 1;
        emit(Phase::Fetching, discovered, processed, tally.stored);
    }
    if shared.rate_limited.load(Ordering::SeqCst) {
        tally.rate_limited = true;
    }
    tally.summarize();
    let verdict = tally.verdict(discovery.complete, truncated, discovered);

    emit(Phase::Finished, discovered, processed, tally.stored);
    tally.into_outcome(
        &adapter_id,
        verdict,
        discovered,
        started,
        (deps.clock)().max(started),
    )
}

async fn process_one(shared: Arc<Shared>, reference: ListingRef, normalized_url: String) -> Item {
    let acquired = tokio::select! {
        biased;
        () = shared.cancel.cancelled() => return Item::Cancelled,
        permit = shared.permits.acquire() => permit,
    };
    let Ok(_permit) = acquired else {
        return Item::Failed {
            kind: "internal_error",
            detail: "the concurrency limiter closed".to_owned(),
        };
    };
    if shared.cancel.is_cancelled() {
        return Item::Cancelled;
    }
    if shared.rate_limited.load(Ordering::SeqCst) {
        return Item::NotAttempted;
    }

    let adapter = &shared.adapter;
    let raw = match adapter
        .fetch_listing(&shared.fetcher, &reference, &shared.cancel)
        .await
    {
        Ok(raw) => raw,
        Err(SourceError::NotFound { status }) => {
            return mark_gone(&shared, &normalized_url, status).await;
        }
        Err(SourceError::Unparseable(message)) => {
            record_unreadable(&shared, &reference, &normalized_url, &message).await;
            return Item::Unreadable(message);
        }
        Err(SourceError::RateLimited { .. }) => {
            shared.rate_limited.store(true, Ordering::SeqCst);
            return Item::RateLimited;
        }
        Err(SourceError::Cancelled) => return Item::Cancelled,
        Err(other) => {
            return Item::Failed {
                kind: other.kind(),
                detail: other.to_string(),
            };
        }
    };

    let now = (shared.deps.clock)();
    let normalized = match normalize(
        adapter.id(),
        adapter.platform(),
        adapter.config(),
        &raw,
        now,
    ) {
        Ok(n) => n,
        Err(SourceError::Unparseable(message)) => return Item::Unreadable(message),
        Err(SourceError::Disallowed(why)) => {
            return Item::Skipped(format!("a listing was skipped: {why}"));
        }
        Err(other) => {
            return Item::Failed {
                kind: other.kind(),
                detail: other.to_string(),
            };
        }
    };
    let location = normalized
        .listing
        .location_text
        .as_deref()
        .and_then(|text| shared.locations.resolve(text));
    let max_images = adapter.config().max_images_per_listing;
    let stored = with_db(&shared.deps.db, move |conn| {
        let outcome = upsert_listing(
            conn,
            &ListingWrite {
                listing: &normalized.listing,
                source_fields: Some(&normalized.source_fields),
                location,
            },
        )?;
        // Queue the images for the image job; nothing is downloaded here.
        register_images(
            conn,
            outcome.listing_id,
            &normalized.image_urls,
            max_images,
            now,
        )?;
        Ok(())
    })
    .await;
    match stored {
        Ok(()) => Item::Stored,
        Err(e) => Item::Failed {
            kind: "database",
            detail: e.to_string(),
        },
    }
}

/// `NotFound` is explicit evidence the listing is gone. Only a listing we already know about is
/// marked; nothing is created for one we never stored.
async fn mark_gone(shared: &Arc<Shared>, normalized_url: &str, status: u16) -> Item {
    let adapter_id = shared.adapter.id().to_owned();
    let url = normalized_url.to_owned();
    let at = (shared.deps.clock)();
    let marked = with_db(&shared.deps.db, move |conn| {
        match find_listing(conn, &adapter_id, &url)? {
            Some(existing) => mark_unavailable(conn, existing.id, at, &format!("HTTP {status}")),
            None => Ok(false),
        }
    })
    .await;
    Item::Gone {
        marked: marked.unwrap_or(false),
    }
}

/// A page we fetched but could not read: if the listing is already stored, record that its last
/// read failed (content and price are kept); never create a listing from nothing.
async fn record_unreadable(
    shared: &Arc<Shared>,
    reference: &ListingRef,
    normalized_url: &str,
    message: &str,
) {
    let adapter = &shared.adapter;
    let now = (shared.deps.clock)();
    let listing = NormalizedListing {
        source: SourceRef {
            platform: adapter.platform().to_owned(),
            adapter_id: adapter.id().to_owned(),
            source_listing_id: reference.source_listing_id.clone(),
        },
        url: reference.url.clone(),
        normalized_url: normalized_url.to_owned(),
        title: None,
        description: None,
        location_text: None,
        currency: Currency::Brl,
        facts: parse_listing_text(""),
        status: ListingStatus::Active,
        extraction_status: ExtractionStatus::Failed,
        extraction_error: Some(message.to_owned()),
        first_seen_at: now,
        last_seen_at: now,
        last_verified_at: None,
        last_scraped_at: Some(now),
    };
    let adapter_id = adapter.id().to_owned();
    let url = normalized_url.to_owned();
    let _ = with_db(&shared.deps.db, move |conn| {
        if find_listing(conn, &adapter_id, &url)?.is_some() {
            upsert_listing(
                conn,
                &ListingWrite {
                    listing: &listing,
                    source_fields: None,
                    location: None,
                },
            )?;
        }
        Ok(())
    })
    .await;
}

#[cfg(test)]
mod tests;
