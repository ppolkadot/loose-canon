//! The image job: pending images -> bounded download -> decode and hash -> cache file -> record.
//!
//! Every image ends in an explicit outcome. Permanent problems (corrupt, gone, refused,
//! unsupported, oversized) are recorded as `failed`; an image host the source may not use is
//! `skipped`; transient problems (timeouts, server errors, rate limits, a cache that cannot be
//! written) leave the image `pending` so a later run retries it. Nothing here can fail a search:
//! problems are per-image outcomes.
//!
//! * downloads go through the source's [`Fetcher`], so host allow-lists, per-source and global
//!   concurrency, request spacing, retries and size limits are the same as for pages;
//! * a refusal (403) or a rate limit (429) ends that source's work for the call: the remaining
//!   images stay pending and are not probed again;
//! * decoding runs on the blocking pool under a small CPU bound, never on the async workers, and
//!   a decoder that panics on hostile bytes fails that one image;
//! * the database is never held across a download or a decode;
//! * cancellation is observed while waiting, downloading and between images.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use rusqlite::Connection;
use serde::Serialize;
use tokio::{sync::Semaphore, task::JoinSet};

use super::{
    cache::write_image,
    decode::{HashedImage, ImageError, ImageLimits, hash_image},
};
use crate::{
    db::images::{
        PendingImage, count_pending_outside, pending_images, record_final, record_hashed,
        record_transient,
    },
    error::Result,
    models::image::ImageProcessingStatus,
    sources::{
        CancelToken, Fetcher, SourceAdapter, SourceConfig, SourceError,
        pipeline::{CollectionDeps, with_db},
    },
};

/// The part of a source the image job needs: which host rules and limits to apply.
#[derive(Debug, Clone)]
pub struct ImageSource {
    pub adapter_id: String,
    pub config: SourceConfig,
}

impl ImageSource {
    #[must_use]
    pub fn from_adapter(adapter: &dyn SourceAdapter) -> Self {
        Self {
            adapter_id: adapter.id().to_owned(),
            config: adapter.config().clone(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageReport {
    /// Images a download was attempted for and that reached an outcome.
    pub attempted: u32,
    pub hashed: u32,
    /// Permanent failures recorded.
    pub failed: u32,
    /// Images the source may not use (host outside its allow-list), recorded as skipped.
    pub skipped: u32,
    /// Transient failures: still pending, retried by a later run.
    pub retry_later: u32,
    /// Pending images whose source is not registered; left untouched.
    pub unavailable_source: u32,
    /// Not completed because the call was cancelled or the source refused or rate-limited us.
    /// They stay pending.
    pub not_attempted: u32,
    /// Outcomes that could not be saved or tasks that failed unexpectedly; the image stays
    /// pending.
    pub internal_errors: u32,
    pub cancelled: bool,
}

impl ImageReport {
    fn add(&mut self, outcome: Outcome) {
        match outcome {
            Outcome::Hashed => {
                self.attempted += 1;
                self.hashed += 1;
            }
            Outcome::Failed => {
                self.attempted += 1;
                self.failed += 1;
            }
            Outcome::Skipped => self.skipped += 1,
            Outcome::RetryLater => {
                self.attempted += 1;
                self.retry_later += 1;
            }
            Outcome::Interrupted => self.not_attempted += 1,
            Outcome::InternalError => self.internal_errors += 1,
        }
    }

    fn merge(&mut self, other: &Self) {
        self.attempted += other.attempted;
        self.hashed += other.hashed;
        self.failed += other.failed;
        self.skipped += other.skipped;
        self.retry_later += other.retry_later;
        self.not_attempted += other.not_attempted;
        self.internal_errors += other.internal_errors;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Outcome {
    Hashed,
    Failed,
    Skipped,
    RetryLater,
    Interrupted,
    InternalError,
}

/// An outcome, and whether the source should get no further requests in this call.
#[derive(Debug, Clone, Copy)]
struct Done {
    outcome: Outcome,
    stop_source: bool,
}

impl Done {
    const fn of(outcome: Outcome) -> Self {
        Self {
            outcome,
            stop_source: false,
        }
    }
}

type Hasher = fn(&[u8], &ImageLimits) -> std::result::Result<HashedImage, ImageError>;

struct Shared {
    deps: CollectionDeps,
    cache_root: PathBuf,
    cancel: CancelToken,
    /// Bounds simultaneous decodes; decoding is the only CPU-heavy step.
    cpu: Semaphore,
    hasher: Hasher,
}

/// Processes up to `batch` pending images of the given sources.
pub async fn process_pending_images(
    deps: &CollectionDeps,
    sources: &[ImageSource],
    cache_root: &Path,
    batch: u32,
    cancel: &CancelToken,
) -> Result<ImageReport> {
    run(deps, sources, cache_root, batch, cancel, hash_image).await
}

/// [`process_pending_images`] with the decoder passed in, so a decoder failure can be tested.
pub(crate) async fn run(
    deps: &CollectionDeps,
    sources: &[ImageSource],
    cache_root: &Path,
    batch: u32,
    cancel: &CancelToken,
    hasher: Hasher,
) -> Result<ImageReport> {
    let mut report = ImageReport::default();
    if cancel.is_cancelled() {
        report.cancelled = true;
        return Ok(report);
    }

    let adapter_ids: Vec<String> = sources.iter().map(|s| s.adapter_id.clone()).collect();
    let (pending, outside) = with_db(&deps.db, move |conn| {
        let ids: Vec<&str> = adapter_ids.iter().map(String::as_str).collect();
        Ok((
            pending_images(conn, &ids, batch)?,
            count_pending_outside(conn, &ids)?,
        ))
    })
    .await?;
    report.unavailable_source = outside;

    let mut by_adapter: BTreeMap<String, Vec<PendingImage>> = BTreeMap::new();
    for image in pending {
        by_adapter
            .entry(image.adapter_id.clone())
            .or_default()
            .push(image);
    }

    // Build every fetcher first: an invalid configuration is a bug to report, not an image outcome.
    let mut jobs = Vec::new();
    for source in sources {
        if let Some(images) = by_adapter.remove(&source.adapter_id) {
            let fetcher = Fetcher::new(source.config.clone(), Arc::clone(&deps.global_limit))?;
            jobs.push((Arc::new(fetcher), images));
        }
    }

    let parallelism = std::thread::available_parallelism().map_or(2, std::num::NonZero::get);
    let shared = Arc::new(Shared {
        deps: deps.clone(),
        cache_root: cache_root.to_path_buf(),
        cancel: cancel.clone(),
        cpu: Semaphore::new(parallelism.clamp(1, 4)),
        hasher,
    });
    let mut tasks = JoinSet::new();
    for (fetcher, images) in jobs {
        tasks.spawn(process_source(Arc::clone(&shared), fetcher, images));
    }
    while let Some(joined) = tasks.join_next().await {
        match joined {
            Ok(source_report) => report.merge(&source_report),
            Err(_) => report.internal_errors += 1,
        }
    }
    report.cancelled = cancel.is_cancelled();
    Ok(report)
}

async fn process_source(
    shared: Arc<Shared>,
    fetcher: Arc<Fetcher>,
    images: Vec<PendingImage>,
) -> ImageReport {
    let config = fetcher.config();
    let limits = ImageLimits {
        max_bytes: config.max_image_bytes,
        ..ImageLimits::default()
    };
    let permits = Arc::new(Semaphore::new(config.max_concurrency));
    let stop = Arc::new(AtomicBool::new(false));
    let mut tasks = JoinSet::new();
    let mut report = ImageReport::default();

    let total = images.len();
    for (index, image) in images.into_iter().enumerate() {
        let permit = tokio::select! {
            biased;
            () = shared.cancel.cancelled() => None,
            permit = Arc::clone(&permits).acquire_owned() => permit.ok(),
        };
        let Some(permit) = permit else {
            report.not_attempted += count(total - index);
            break;
        };
        if stop.load(Ordering::SeqCst) {
            report.not_attempted += count(total - index);
            break;
        }
        let (shared, fetcher, stop) =
            (Arc::clone(&shared), Arc::clone(&fetcher), Arc::clone(&stop));
        tasks.spawn(async move {
            let _permit = permit;
            let done = process_image(&shared, &fetcher, &limits, &image).await;
            if done.stop_source {
                stop.store(true, Ordering::SeqCst);
            }
            done.outcome
        });
    }
    while let Some(joined) = tasks.join_next().await {
        report.add(joined.unwrap_or(Outcome::InternalError));
    }
    report
}

fn count(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

async fn process_image(
    shared: &Arc<Shared>,
    fetcher: &Fetcher,
    limits: &ImageLimits,
    image: &PendingImage,
) -> Done {
    match fetcher.get_image(&image.source_url, &shared.cancel).await {
        Ok(response) => store(shared, limits, image, response.body).await,
        Err(error) => record_fetch_error(shared, image, &error).await,
    }
}

enum Processed {
    Stored {
        hashed: HashedImage,
        relative: String,
    },
    Rejected(ImageError),
    CacheWrite,
}

async fn store(
    shared: &Arc<Shared>,
    limits: &ImageLimits,
    image: &PendingImage,
    body: Vec<u8>,
) -> Done {
    let _cpu = shared.cpu.acquire().await;
    let (root, decode, limits) = (shared.cache_root.clone(), shared.hasher, *limits);
    let (listing_id, position) = (image.listing_id, image.position);
    let work = tokio::task::spawn_blocking(move || match decode(&body, &limits) {
        Err(error) => Processed::Rejected(error),
        Ok(hashed) => match write_image(&root, listing_id, position, hashed.format, &body) {
            Ok(relative) => Processed::Stored { hashed, relative },
            Err(_) => Processed::CacheWrite,
        },
    })
    .await;

    let at = (shared.deps.clock)();
    let id = image.id;
    match work {
        Ok(Processed::Stored { hashed, relative }) => {
            let HashedImage {
                width,
                height,
                phash,
                ..
            } = hashed;
            finish(shared, Outcome::Hashed, move |conn| {
                record_hashed(conn, id, &relative, width, height, phash, at)
            })
            .await
        }
        Ok(Processed::Rejected(error)) => {
            record_permanent(shared, id, ImageProcessingStatus::Failed, error.kind()).await
        }
        Ok(Processed::CacheWrite) => record_retry(shared, id, "cache_write_failed").await,
        // A decoder that panics on these bytes will do so every time: permanent for this image.
        Err(join_error) if join_error.is_panic() => {
            record_permanent(shared, id, ImageProcessingStatus::Failed, "decoder_crashed").await
        }
        Err(_) => Done::of(Outcome::Interrupted),
    }
}

async fn record_fetch_error(
    shared: &Arc<Shared>,
    image: &PendingImage,
    error: &SourceError,
) -> Done {
    let id = image.id;
    match error {
        SourceError::Cancelled => Done::of(Outcome::Interrupted),
        SourceError::Disallowed(_) => {
            record_permanent(shared, id, ImageProcessingStatus::Skipped, error.kind()).await
        }
        SourceError::RateLimited { .. } => stop(record_retry(shared, id, error.kind()).await),
        SourceError::Blocked { .. } => {
            stop(record_permanent(shared, id, ImageProcessingStatus::Failed, error.kind()).await)
        }
        other if other.is_retryable() => record_retry(shared, id, other.kind()).await,
        other => record_permanent(shared, id, ImageProcessingStatus::Failed, other.kind()).await,
    }
}

const fn stop(done: Done) -> Done {
    Done {
        stop_source: true,
        ..done
    }
}

async fn record_permanent(
    shared: &Arc<Shared>,
    id: i64,
    status: ImageProcessingStatus,
    kind: &'static str,
) -> Done {
    let at = (shared.deps.clock)();
    let outcome = if status == ImageProcessingStatus::Skipped {
        Outcome::Skipped
    } else {
        Outcome::Failed
    };
    finish(shared, outcome, move |conn| {
        record_final(conn, id, status, kind, at)
    })
    .await
}

async fn record_retry(shared: &Arc<Shared>, id: i64, kind: &'static str) -> Done {
    finish(shared, Outcome::RetryLater, move |conn| {
        record_transient(conn, id, kind)
    })
    .await
}

/// Saves an outcome; when it cannot be saved the image stays as it was and the failure is counted.
async fn finish(
    shared: &Arc<Shared>,
    outcome: Outcome,
    write: impl FnOnce(&mut Connection) -> Result<bool> + Send + 'static,
) -> Done {
    match with_db(&shared.deps.db, write).await {
        Ok(_) => Done::of(outcome),
        Err(_) => Done::of(Outcome::InternalError),
    }
}

#[cfg(test)]
mod tests;
