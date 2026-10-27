//! The application service: the logic the Tauri commands call, kept free of Tauri so it can be
//! tested without a window.
//!
//! * **startup** seeds the region catalogue (idempotent, never deletes) and marks runs that were
//!   still `running` when the previous process died as failed;
//! * **one collection at a time**: starting a second is refused, cancelling stops the running one;
//! * a collection is followed by image processing and then a matching pass, so a finished run
//!   leaves the library ready to review. Matching only proposes; nothing is ever merged here;
//! * with no registered source, starting a collection fails with `no_sources_configured` and
//!   creates no run: that must never look like "searched and found nothing".

use std::{
    path::PathBuf,
    sync::{Arc, Mutex, MutexGuard, PoisonError},
};

use serde::Serialize;
use tokio::sync::Semaphore;

use crate::{
    db::{
        Database,
        regions::{DEFAULT_SEED_JSON, SeedReport, parse_seed, seed_regions},
        runs::{fail_interrupted_runs, latest_outcomes_by_source},
    },
    error::{Error, Result},
    images::pipeline::{ImageReport, ImageSource, process_pending_images},
    matching::{
        MatchConfig,
        run::{MatchRunReport, run_matching},
    },
    models::collection::{RunStatus, SourceOutcome},
    sources::{
        CancelToken, SearchRequest, SourceAdapter,
        pipeline::{Clock, CollectionDeps, ProgressSink, run_collection, with_db},
    },
};

/// Images processed after each collection (the rest wait for the next one).
const IMAGE_BATCH: u32 = 200;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StartupReport {
    pub regions: SeedReport,
    /// Runs left `running` by a process that stopped, now marked failed.
    pub interrupted_runs: usize,
}

/// A registered source and how its last run went.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceInfo {
    pub adapter_id: String,
    pub platform: String,
    pub latest_outcome: Option<SourceOutcome>,
}

/// What a collection and its follow-up work produced. `error` is set when the collection itself
/// could not run or finish; saved results are unaffected either way.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CollectionFinished {
    pub run_id: Option<i64>,
    pub status: Option<RunStatus>,
    pub outcomes: Vec<SourceOutcome>,
    pub error: Option<String>,
    /// Problems in the work after a successful collection (images, matching).
    pub follow_up_errors: Vec<String>,
    pub images: Option<ImageReport>,
    pub matching: Option<MatchRunReport>,
}

pub type FinishedSink = Box<dyn FnOnce(CollectionFinished) + Send>;

pub struct AppService {
    db: Arc<Database>,
    adapters: Vec<Arc<dyn SourceAdapter>>,
    image_cache_dir: PathBuf,
    clock: Clock,
    /// The cancel token of the collection that is running, if any.
    active: Mutex<Option<CancelToken>>,
}

impl AppService {
    #[must_use]
    pub fn new(
        db: Arc<Database>,
        adapters: Vec<Arc<dyn SourceAdapter>>,
        image_cache_dir: PathBuf,
        clock: Clock,
    ) -> Arc<Self> {
        Arc::new(Self {
            db,
            adapters,
            image_cache_dir,
            clock,
            active: Mutex::new(None),
        })
    }

    #[must_use]
    pub const fn db(&self) -> &Arc<Database> {
        &self.db
    }

    /// Runs a short blocking database closure off the async workers.
    pub async fn blocking<T: Send + 'static>(
        &self,
        f: impl FnOnce(&mut rusqlite::Connection) -> Result<T> + Send + 'static,
    ) -> Result<T> {
        with_db(&self.db, f).await
    }

    /// Runs `f` on the blocking pool: for work that touches the database from an async command.
    pub async fn offload<T: Send + 'static>(
        self: &Arc<Self>,
        f: impl FnOnce(&Self) -> Result<T> + Send + 'static,
    ) -> Result<T> {
        let service = Arc::clone(self);
        tokio::task::spawn_blocking(move || f(&service))
            .await
            .map_err(|e| {
                Error::Io(std::io::Error::other(format!(
                    "background task failed: {e}"
                )))
            })?
    }

    /// One matching pass on demand (a collection already runs one when it finishes).
    pub async fn run_matching_pass(&self) -> Result<MatchRunReport> {
        let deps = CollectionDeps {
            db: Arc::clone(&self.db),
            global_limit: Arc::new(Semaphore::new(CollectionDeps::DEFAULT_GLOBAL_CONCURRENCY)),
            clock: Arc::clone(&self.clock),
        };
        run_matching(&deps, &MatchConfig::default(), &CancelToken::new()).await
    }

    pub fn startup(&self) -> Result<StartupReport> {
        let now = (self.clock)();
        self.db.with_conn(|conn| {
            let regions = seed_regions(conn, &parse_seed(DEFAULT_SEED_JSON)?)?;
            let interrupted_runs = fail_interrupted_runs(conn, now)?;
            Ok(StartupReport {
                regions,
                interrupted_runs,
            })
        })
    }

    /// The registered sources and how their last run went. A source that never ran has no
    /// outcome; that is not the same as one that failed.
    pub fn sources(&self) -> Result<Vec<SourceInfo>> {
        let latest = self.db.with_conn(|conn| latest_outcomes_by_source(conn))?;
        Ok(self
            .adapters
            .iter()
            .map(|adapter| SourceInfo {
                adapter_id: adapter.id().to_owned(),
                platform: adapter.platform().to_owned(),
                latest_outcome: latest
                    .iter()
                    .find(|stored| stored.outcome.adapter_id == adapter.id())
                    .map(|stored| stored.outcome.clone()),
            })
            .collect())
    }

    fn active_guard(&self) -> MutexGuard<'_, Option<CancelToken>> {
        // The flag is a plain token holder: a panic elsewhere cannot leave it half-updated.
        self.active.lock().unwrap_or_else(PoisonError::into_inner)
    }

    #[must_use]
    pub fn collection_running(&self) -> bool {
        self.active_guard().is_some()
    }

    /// Cancels the running collection. Returns whether there was one.
    #[must_use]
    pub fn cancel_collection(&self) -> bool {
        self.active_guard().as_ref().is_some_and(|token| {
            token.cancel();
            true
        })
    }

    /// Starts a collection in the background and returns at once; `on_finished` is called when it
    /// (and the image and matching work after it) is done, after the service is free again.
    ///
    /// Must be called from within a Tokio runtime.
    pub fn start_collection(
        self: &Arc<Self>,
        request: SearchRequest,
        progress: Option<ProgressSink>,
        on_finished: Option<FinishedSink>,
    ) -> Result<()> {
        let cancel = CancelToken::new();
        {
            let mut active = self.active_guard();
            if self.adapters.is_empty() {
                return Err(Error::NoSourcesConfigured);
            }
            if active.is_some() {
                return Err(Error::CollectionAlreadyRunning);
            }
            *active = Some(cancel.clone());
        }
        let busy = Busy(Arc::clone(self));
        tokio::spawn(async move {
            let finished = busy.0.run_job(request, cancel, progress).await;
            // Free the service first, so the callback can already start another collection.
            let service = Arc::clone(&busy.0);
            drop(busy);
            let _ = service;
            if let Some(sink) = on_finished {
                sink(finished);
            }
        });
        Ok(())
    }

    async fn run_job(
        &self,
        request: SearchRequest,
        cancel: CancelToken,
        progress: Option<ProgressSink>,
    ) -> CollectionFinished {
        let deps = CollectionDeps {
            db: Arc::clone(&self.db),
            global_limit: Arc::new(Semaphore::new(CollectionDeps::DEFAULT_GLOBAL_CONCURRENCY)),
            clock: Arc::clone(&self.clock),
        };
        let report = match run_collection(&deps, &self.adapters, &request, &cancel, progress).await
        {
            Ok(report) => report,
            Err(error) => {
                return CollectionFinished {
                    run_id: None,
                    status: None,
                    outcomes: Vec::new(),
                    error: Some(error.to_string()),
                    follow_up_errors: Vec::new(),
                    images: None,
                    matching: None,
                };
            }
        };

        let mut follow_up_errors = Vec::new();
        let sources: Vec<ImageSource> = self
            .adapters
            .iter()
            .map(|adapter| ImageSource::from_adapter(adapter.as_ref()))
            .collect();
        let images =
            process_pending_images(&deps, &sources, &self.image_cache_dir, IMAGE_BATCH, &cancel)
                .await
                .map_err(|e| follow_up_errors.push(format!("image processing failed: {e}")))
                .ok();
        let matching = run_matching(&deps, &MatchConfig::default(), &cancel)
            .await
            .map_err(|e| follow_up_errors.push(format!("matching failed: {e}")))
            .ok();
        CollectionFinished {
            run_id: Some(report.run_id),
            status: Some(report.status),
            outcomes: report.outcomes,
            error: None,
            follow_up_errors,
            images,
            matching,
        }
    }
}

/// Marks the service busy while it lives. Dropping it (also when the job panics) frees the
/// service, so a crash can never leave it refusing every later collection.
struct Busy(Arc<AppService>);

impl Drop for Busy {
    fn drop(&mut self) {
        *self.0.active_guard() = None;
    }
}

#[cfg(test)]
mod tests;
