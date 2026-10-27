use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicI64, AtomicUsize, Ordering},
    },
    time::Duration,
};

use chrono::TimeZone;
use tokio::sync::Notify;

use super::*;
use crate::{
    db::{
        listings::{find_listing, get_listing},
        price_history::list_observations,
        properties::{PropertyQuery, list_properties, set_user_status},
        regions::{DEFAULT_SEED_JSON, parse_seed, seed_regions},
        runs::{get_run, latest_outcomes_by_source, list_runs},
    },
    error::Error,
    models::{
        collection::SourceOutcomeKind,
        confidence::Confidence,
        listing::{ListingStatus, UserStatus},
        price::PriceClassification,
    },
    sources::{
        Discovery, Fetcher, ListingRef, RawListing, SourceConfig, SourceError,
        adapter::{BoxFuture, ExtractionReport},
    },
};

// ---- a scripted adapter (test double only; never part of the shipped registry) ---------------

#[derive(Clone)]
enum Script {
    Page(Box<RawListing>),
    Fail(SourceError),
    Panic,
}

struct FakeAdapter {
    id: String,
    config: SourceConfig,
    discovery: Mutex<std::result::Result<Discovery, SourceError>>,
    pages: Mutex<HashMap<String, Script>>,
    delay: Duration,
    gate: Option<Arc<Notify>>,
    in_flight: Arc<AtomicUsize>,
    max_in_flight: Arc<AtomicUsize>,
    started: Arc<AtomicUsize>,
    fetched: Mutex<Vec<String>>,
}

impl FakeAdapter {
    fn new(id: &str) -> Self {
        let mut config = SourceConfig::new(vec![format!("{id}.test")]);
        config.min_request_interval = Duration::ZERO;
        config.max_concurrency = 2;
        Self {
            id: id.to_owned(),
            config,
            discovery: Mutex::new(Ok(Discovery {
                refs: vec![],
                complete: true,
                warnings: vec![],
            })),
            pages: Mutex::new(HashMap::new()),
            delay: Duration::ZERO,
            gate: None,
            in_flight: Arc::default(),
            max_in_flight: Arc::default(),
            started: Arc::default(),
            fetched: Mutex::new(Vec::new()),
        }
    }

    fn url(&self, n: usize) -> String {
        format!("https://{}.test/imovel/{n}", self.id)
    }

    fn page(&self, n: usize, text: &str) -> RawListing {
        RawListing {
            url: self.url(n),
            source_listing_id: Some(n.to_string()),
            title: Some(format!("Casa {n}")),
            description: Some(text.to_owned()),
            location_text: None,
            price_text: None,
            extra_fields: BTreeMap::default(),
            image_urls: vec![],
            extraction: ExtractionReport::default(),
        }
    }

    /// Registers `count` listings, discoverable and readable, priced R$ 300 mil + 1 mil * n.
    fn with_listings(self, count: usize) -> Self {
        let refs: Vec<ListingRef> = (1..=count)
            .map(|n| ListingRef {
                url: self.url(n),
                source_listing_id: Some(n.to_string()),
            })
            .collect();
        for n in 1..=count {
            let text = format!("Casa 3 quartos. Valor total R$ {} mil", 300 + n);
            let page = self.page(n, &text);
            self.pages
                .lock()
                .unwrap()
                .insert(self.url(n), Script::Page(Box::new(page)));
        }
        *self.discovery.lock().unwrap() = Ok(Discovery {
            refs,
            complete: true,
            warnings: vec![],
        });
        self
    }

    fn script(self, n: usize, script: Script) -> Self {
        self.pages.lock().unwrap().insert(self.url(n), script);
        self
    }

    fn discovery(self, discovery: std::result::Result<Discovery, SourceError>) -> Self {
        *self.discovery.lock().unwrap() = discovery;
        self
    }

    fn arc(self) -> Arc<dyn SourceAdapter> {
        Arc::new(self)
    }
}

impl SourceAdapter for FakeAdapter {
    fn id(&self) -> &str {
        &self.id
    }

    fn platform(&self) -> &'static str {
        "Fake"
    }

    fn config(&self) -> &SourceConfig {
        &self.config
    }

    fn discover<'a>(
        &'a self,
        _fetcher: &'a Fetcher,
        _request: &'a SearchRequest,
        _cancel: &'a CancelToken,
    ) -> BoxFuture<'a, std::result::Result<Discovery, SourceError>> {
        Box::pin(async move { self.discovery.lock().unwrap().clone() })
    }

    fn fetch_listing<'a>(
        &'a self,
        _fetcher: &'a Fetcher,
        reference: &'a ListingRef,
        cancel: &'a CancelToken,
    ) -> BoxFuture<'a, std::result::Result<RawListing, SourceError>> {
        Box::pin(async move {
            self.started.fetch_add(1, Ordering::SeqCst);
            let now = self.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
            self.max_in_flight.fetch_max(now, Ordering::SeqCst);
            self.fetched.lock().unwrap().push(reference.url.clone());
            let result = async {
                if let Some(gate) = &self.gate {
                    tokio::select! {
                        () = gate.notified() => {}
                        () = cancel.cancelled() => return Err(SourceError::Cancelled),
                    }
                }
                if !self.delay.is_zero() {
                    tokio::select! {
                        () = tokio::time::sleep(self.delay) => {}
                        () = cancel.cancelled() => return Err(SourceError::Cancelled),
                    }
                }
                let script = self.pages.lock().unwrap().get(&reference.url).cloned();
                match script {
                    Some(Script::Page(page)) => Ok(*page),
                    Some(Script::Fail(e)) => Err(e),
                    Some(Script::Panic) => panic!("adapter bug"),
                    None => Err(SourceError::NotFound { status: 404 }),
                }
            }
            .await;
            self.in_flight.fetch_sub(1, Ordering::SeqCst);
            result
        })
    }
}

// ---- helpers -----------------------------------------------------------------------------------

fn deps() -> CollectionDeps {
    deps_with(Arc::new(Database::open_in_memory().unwrap()))
}

fn deps_with(db: Arc<Database>) -> CollectionDeps {
    // A deterministic clock: every reading is one second later than the last.
    let ticks = Arc::new(AtomicI64::new(0));
    CollectionDeps {
        db,
        global_limit: Arc::new(Semaphore::new(8)),
        clock: Arc::new(move || {
            chrono::Utc
                .timestamp_opt(1_780_000_000 + ticks.fetch_add(1, Ordering::SeqCst), 0)
                .unwrap()
        }),
    }
}

async fn run(deps: &CollectionDeps, adapters: Vec<Arc<dyn SourceAdapter>>) -> RunReport {
    run_collection(
        deps,
        &adapters,
        &SearchRequest::default(),
        &CancelToken::new(),
        None,
    )
    .await
    .expect("run")
}

fn outcome<'a>(report: &'a RunReport, adapter: &str) -> &'a SourceOutcome {
    report
        .outcomes
        .iter()
        .find(|o| o.adapter_id == adapter)
        .expect("outcome")
}

fn count(deps: &CollectionDeps, sql: &str) -> i64 {
    deps.db
        .with_conn(|c| Ok(c.query_row(sql, [], |r| r.get(0))?))
        .unwrap()
}

// ---- the happy path and the four outcomes ------------------------------------------------------

#[tokio::test]
async fn a_run_stores_listings_from_every_source_and_reports_each_outcome() {
    let d = deps();
    let report = run(
        &d,
        vec![
            FakeAdapter::new("alpha").with_listings(2).arc(),
            FakeAdapter::new("beta").with_listings(2).arc(),
        ],
    )
    .await;

    assert_eq!(report.status, RunStatus::Completed);
    for adapter in ["alpha", "beta"] {
        let o = outcome(&report, adapter);
        assert_eq!(o.kind, SourceOutcomeKind::Results);
        assert!(o.complete);
        assert_eq!(
            (o.discovered, o.stored, o.unparseable, o.failed),
            (2, 2, 0, 0)
        );
        assert_eq!(o.warnings.len(), 0, "{:?}", o.warnings);
    }
    assert_eq!(count(&d, "SELECT COUNT(*) FROM listing_links"), 4);
    assert_eq!(count(&d, "SELECT COUNT(*) FROM listing_price_history"), 4);
    let stored =
        d.db.with_conn(|c| get_run(c, report.run_id))
            .unwrap()
            .unwrap();
    assert_eq!(stored.run.status, RunStatus::Completed);
    assert_eq!(stored.outcomes.len(), 2);
    assert!(stored.run.finished_at.is_some());
}

#[tokio::test]
async fn nothing_found_is_reported_as_empty_only_when_the_search_was_complete() {
    let d = deps();
    let incomplete = Discovery {
        refs: vec![],
        complete: false,
        warnings: vec!["page 2 missing".into()],
    };
    let report = run(
        &d,
        vec![
            FakeAdapter::new("complete").arc(),
            FakeAdapter::new("partial").discovery(Ok(incomplete)).arc(),
        ],
    )
    .await;
    let empty = outcome(&report, "complete");
    assert_eq!(
        (empty.kind, empty.complete, empty.discovered),
        (SourceOutcomeKind::SuccessfulEmpty, true, 0)
    );
    let partial = outcome(&report, "partial");
    assert_eq!(
        partial.kind,
        SourceOutcomeKind::Failed,
        "an incomplete search that found nothing is not 'no results'"
    );
    assert_eq!(partial.error_kind.as_deref(), Some("incomplete_discovery"));
    assert!(
        partial
            .warnings
            .iter()
            .any(|w| w.contains("page 2 missing"))
    );
    assert!(!partial.complete);
}

#[tokio::test]
async fn a_failing_source_never_looks_empty_and_does_not_affect_the_others_or_saved_results() {
    let d = deps();
    // Results saved by an earlier run.
    run(&d, vec![FakeAdapter::new("beta").with_listings(1).arc()]).await;
    assert_eq!(count(&d, "SELECT COUNT(*) FROM listing_links"), 1);

    let report = run(
        &d,
        vec![
            FakeAdapter::new("alpha")
                .discovery(Err(SourceError::Blocked { status: 403 }))
                .arc(),
            FakeAdapter::new("beta").with_listings(1).arc(),
        ],
    )
    .await;
    let alpha = outcome(&report, "alpha");
    assert_eq!(
        (alpha.kind, alpha.complete),
        (SourceOutcomeKind::Failed, false)
    );
    assert_eq!(alpha.error_kind.as_deref(), Some("blocked"));
    assert!(alpha.error_detail.as_deref().unwrap().contains("403"));
    assert_eq!(outcome(&report, "beta").kind, SourceOutcomeKind::Results);
    assert_eq!(
        report.status,
        RunStatus::Completed,
        "a failing source is an outcome, not a failed run"
    );
    assert_eq!(
        count(&d, "SELECT COUNT(*) FROM listing_links"),
        1,
        "saved results survive"
    );

    let latest = d.db.with_conn(|c| latest_outcomes_by_source(c)).unwrap();
    let kinds: Vec<(&str, SourceOutcomeKind)> = latest
        .iter()
        .map(|o| (o.outcome.adapter_id.as_str(), o.outcome.kind))
        .collect();
    assert_eq!(
        kinds,
        vec![
            ("alpha", SourceOutcomeKind::Failed),
            ("beta", SourceOutcomeKind::Results)
        ]
    );
}

#[tokio::test]
async fn pages_that_cannot_be_read_make_a_source_unreliable_not_empty() {
    let d = deps();
    let source = FakeAdapter::new("alpha")
        .with_listings(3)
        .script(
            1,
            Script::Fail(SourceError::Unparseable("layout changed".into())),
        )
        .script(
            2,
            Script::Fail(SourceError::Unparseable("layout changed".into())),
        )
        .script(
            3,
            Script::Fail(SourceError::Unparseable("layout changed".into())),
        );
    let report = run(&d, vec![source.arc()]).await;
    let o = outcome(&report, "alpha");
    assert_eq!(o.kind, SourceOutcomeKind::Unreliable);
    assert_eq!((o.discovered, o.stored, o.unparseable), (3, 0, 3));
    assert!(!o.complete);
    assert_eq!(count(&d, "SELECT COUNT(*) FROM listing_links"), 0);
}

#[tokio::test]
async fn a_mostly_readable_source_has_results_but_is_not_complete_and_says_why() {
    let d = deps();
    let source = FakeAdapter::new("alpha").with_listings(4).script(
        4,
        Script::Fail(SourceError::Unparseable("price block missing".into())),
    );
    let report = run(&d, vec![source.arc()]).await;
    let o = outcome(&report, "alpha");
    assert_eq!(o.kind, SourceOutcomeKind::Results);
    assert_eq!((o.stored, o.unparseable), (3, 1));
    assert!(
        !o.complete,
        "one unreadable listing means the search is not complete"
    );
    assert!(
        o.warnings
            .iter()
            .any(|w| w.contains("unreadable") || w.contains("unparseable")),
        "{:?}",
        o.warnings
    );
}

#[tokio::test]
async fn a_mostly_unreadable_source_is_unreliable_even_if_a_page_was_stored() {
    let d = deps();
    let source = FakeAdapter::new("alpha")
        .with_listings(4)
        .script(2, Script::Fail(SourceError::Unparseable("x".into())))
        .script(3, Script::Fail(SourceError::Unparseable("x".into())))
        .script(4, Script::Fail(SourceError::Unparseable("x".into())));
    let report = run(&d, vec![source.arc()]).await;
    let o = outcome(&report, "alpha");
    assert_eq!(
        (o.kind, o.stored, o.unparseable),
        (SourceOutcomeKind::Unreliable, 1, 3)
    );
}

#[tokio::test]
async fn fetch_failures_are_counted_and_the_dominant_error_is_reported() {
    let d = deps();
    let all_down = FakeAdapter::new("alpha")
        .with_listings(3)
        .script(1, Script::Fail(SourceError::Timeout))
        .script(2, Script::Fail(SourceError::Timeout))
        .script(3, Script::Fail(SourceError::Blocked { status: 403 }));
    let report = run(&d, vec![all_down.arc()]).await;
    let o = outcome(&report, "alpha");
    assert_eq!(
        (o.kind, o.failed, o.stored),
        (SourceOutcomeKind::Failed, 3, 0)
    );
    assert_eq!(
        o.error_kind.as_deref(),
        Some("timeout"),
        "the most common failure"
    );

    let some_down = FakeAdapter::new("beta")
        .with_listings(3)
        .script(3, Script::Fail(SourceError::Timeout));
    let report = run(&d, vec![some_down.arc()]).await;
    let o = outcome(&report, "beta");
    assert_eq!(
        (o.kind, o.stored, o.failed, o.complete),
        (SourceOutcomeKind::Results, 2, 1, false)
    );
}

#[tokio::test]
async fn rate_limiting_stops_that_source_gracefully_and_is_reported() {
    let d = deps();
    let mut source = FakeAdapter::new("alpha").with_listings(6).script(
        2,
        Script::Fail(SourceError::RateLimited {
            retry_after_secs: Some(60),
        }),
    );
    source.config.max_concurrency = 1;
    let calls = Arc::new(source);
    let report = run(&d, vec![Arc::clone(&calls) as Arc<dyn SourceAdapter>]).await;
    let o = outcome(&report, "alpha");
    assert_eq!(
        (o.kind, o.stored, o.complete),
        (SourceOutcomeKind::Results, 1, false)
    );
    assert!(
        o.warnings.iter().any(|w| w.contains("rate limited")),
        "{:?}",
        o.warnings
    );
    assert!(
        calls.started.load(Ordering::SeqCst) <= 2,
        "no further requests after the rate limit"
    );
}

#[tokio::test]
async fn cancelling_mid_run_ends_the_run_as_cancelled_and_keeps_what_was_stored() {
    let d = deps();
    let mut source = FakeAdapter::new("alpha").with_listings(8);
    source.delay = Duration::from_millis(60);
    source.config.max_concurrency = 1;
    let source = Arc::new(source);
    let cancel = CancelToken::new();
    let task = {
        let (d, source, cancel) = (d.clone(), Arc::clone(&source), cancel.clone());
        tokio::spawn(async move {
            run_collection(
                &d,
                &[source as Arc<dyn SourceAdapter>],
                &SearchRequest::default(),
                &cancel,
                None,
            )
            .await
        })
    };
    tokio::time::sleep(Duration::from_millis(200)).await;
    cancel.cancel();
    let report = task.await.unwrap().unwrap();

    assert_eq!(report.status, RunStatus::Cancelled);
    let o = outcome(&report, "alpha");
    assert!(!o.complete);
    assert!(o.stored >= 1 && o.stored < 8, "stored {}", o.stored);
    assert_eq!(
        count(&d, "SELECT COUNT(*) FROM listing_links"),
        i64::from(o.stored),
        "partial results are saved"
    );
    assert!(
        source.started.load(Ordering::SeqCst) < 8,
        "no new fetches after cancelling"
    );
    let run =
        d.db.with_conn(|c| get_run(c, report.run_id))
            .unwrap()
            .unwrap()
            .run;
    assert_eq!(run.status, RunStatus::Cancelled);
}

// ---- keeping history and user choices ----------------------------------------------------------

#[tokio::test]
async fn a_listing_confirmed_gone_is_marked_unavailable_but_kept_with_its_history() {
    let d = deps();
    run(&d, vec![FakeAdapter::new("alpha").with_listings(2).arc()]).await;

    // Second run: listing 1 answers "not found"; listing 2 is still there.
    let second = FakeAdapter::new("alpha")
        .with_listings(2)
        .script(1, Script::Fail(SourceError::NotFound { status: 410 }));
    let report = run(&d, vec![second.arc()]).await;
    let o = outcome(&report, "alpha");
    assert_eq!((o.stored, o.skipped), (1, 1));
    assert!(
        o.warnings.iter().any(|w| w.contains("gone")),
        "{:?}",
        o.warnings
    );

    let gone =
        d.db.with_conn(|c| find_listing(c, "alpha", "https://alpha.test/imovel/1"))
            .unwrap()
            .unwrap();
    assert_eq!(gone.status, ListingStatus::Unavailable);
    assert!(gone.status_reason.as_deref().unwrap().contains("410"));
    assert!(gone.price.is_some(), "its last price is kept");
    let history = d.db.with_conn(|c| list_observations(c, gone.id)).unwrap();
    assert_eq!(history.len(), 1, "history is never rewritten");
}

#[tokio::test]
async fn a_not_found_answer_for_a_listing_we_never_stored_creates_nothing() {
    let d = deps();
    let source = FakeAdapter::new("alpha")
        .with_listings(1)
        .script(1, Script::Fail(SourceError::NotFound { status: 404 }));
    let report = run(&d, vec![source.arc()]).await;
    assert_eq!(outcome(&report, "alpha").skipped, 1);
    assert_eq!(count(&d, "SELECT COUNT(*) FROM listing_links"), 0);
}

#[tokio::test]
async fn running_again_changes_nothing_and_a_hidden_property_stays_hidden() {
    let d = deps();
    run(&d, vec![FakeAdapter::new("alpha").with_listings(2).arc()]).await;
    let first =
        d.db.with_conn(|c| find_listing(c, "alpha", "https://alpha.test/imovel/1"))
            .unwrap()
            .unwrap();
    d.db.with_conn(|c| {
        set_user_status(
            c,
            first.property_id.unwrap(),
            UserStatus::Hidden,
            chrono::Utc::now(),
        )
    })
    .unwrap();

    let report = run(&d, vec![FakeAdapter::new("alpha").with_listings(2).arc()]).await;
    assert_eq!(outcome(&report, "alpha").stored, 2);
    assert_eq!(
        count(&d, "SELECT COUNT(*) FROM listing_links"),
        2,
        "no duplicates"
    );
    assert_eq!(count(&d, "SELECT COUNT(*) FROM properties"), 2);
    assert_eq!(
        count(&d, "SELECT COUNT(*) FROM listing_price_history"),
        2,
        "an unchanged price is not a new observation"
    );
    let status: String =
        d.db.with_conn(|c| {
            Ok(c.query_row(
                "SELECT user_status FROM properties WHERE id = ?1",
                [first.property_id.unwrap()],
                |r| r.get(0),
            )?)
        })
        .unwrap();
    assert_eq!(status, "hidden");
    let visible =
        d.db.with_conn(|c| list_properties(c, &PropertyQuery::default()))
            .unwrap();
    assert_eq!(visible.total, 1);
}

// ---- bounds, isolation and safety --------------------------------------------------------------

#[tokio::test]
async fn no_sources_is_an_error_and_creates_no_run() {
    let d = deps();
    let err = run_collection(
        &d,
        &[],
        &SearchRequest::default(),
        &CancelToken::new(),
        None,
    )
    .await
    .unwrap_err();
    assert!(matches!(err, Error::NoSourcesConfigured), "{err}");
    let stored_runs = d.db.with_conn(|c| list_runs(c, 10)).unwrap();
    assert_eq!(stored_runs.len(), 0, "{stored_runs:?}");
}

#[tokio::test]
async fn adapter_ids_must_be_unique() {
    let d = deps();
    let err = run_collection(
        &d,
        &[FakeAdapter::new("dup").arc(), FakeAdapter::new("dup").arc()],
        &SearchRequest::default(),
        &CancelToken::new(),
        None,
    )
    .await
    .unwrap_err();
    assert!(matches!(err, Error::InvalidConfig { .. }), "{err}");
    let stored_runs = d.db.with_conn(|c| list_runs(c, 10)).unwrap();
    assert_eq!(stored_runs.len(), 0, "{stored_runs:?}");
}

#[tokio::test]
async fn an_invalid_source_configuration_fails_that_source_only() {
    let d = deps();
    let mut broken = FakeAdapter::new("broken").with_listings(1);
    broken.config.max_retries = 99;
    let report = run(
        &d,
        vec![
            broken.arc(),
            FakeAdapter::new("fine").with_listings(1).arc(),
        ],
    )
    .await;
    let o = outcome(&report, "broken");
    assert_eq!(
        (o.kind, o.error_kind.as_deref()),
        (SourceOutcomeKind::Failed, Some("invalid_config"))
    );
    assert_eq!(outcome(&report, "fine").kind, SourceOutcomeKind::Results);
}

#[tokio::test]
async fn per_source_concurrency_is_bounded() {
    let d = deps();
    let mut source = FakeAdapter::new("alpha").with_listings(8);
    source.delay = Duration::from_millis(40);
    source.config.max_concurrency = 2;
    let source = Arc::new(source);
    run(&d, vec![Arc::clone(&source) as Arc<dyn SourceAdapter>]).await;
    assert_eq!(source.max_in_flight.load(Ordering::SeqCst), 2);
    assert_eq!(count(&d, "SELECT COUNT(*) FROM listing_links"), 8);
}

#[tokio::test]
async fn the_database_stays_usable_while_a_fetch_is_in_flight() {
    let d = deps();
    let gate = Arc::new(Notify::new());
    let mut source = FakeAdapter::new("alpha").with_listings(1);
    source.gate = Some(Arc::clone(&gate));
    let source = Arc::new(source);
    let task = {
        let (d, source) = (d.clone(), Arc::clone(&source));
        tokio::spawn(async move {
            run_collection(
                &d,
                &[source as Arc<dyn SourceAdapter>],
                &SearchRequest::default(),
                &CancelToken::new(),
                None,
            )
            .await
        })
    };
    // Wait until the fetch is actually in flight.
    for _ in 0..100 {
        if source.in_flight.load(Ordering::SeqCst) == 1 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(source.in_flight.load(Ordering::SeqCst), 1);
    // The run row exists and the database answers promptly while the "network" is blocked.
    let runs = tokio::time::timeout(Duration::from_millis(500), async {
        d.db.with_conn(|c| list_runs(c, 5))
    })
    .await
    .expect("the database lock must not be held across a fetch")
    .unwrap();
    assert_eq!(runs.len(), 1);
    gate.notify_waiters();
    let report = task.await.unwrap().unwrap();
    assert_eq!(report.status, RunStatus::Completed);
}

#[tokio::test]
async fn locations_are_resolved_when_listings_are_stored() {
    let d = deps();
    d.db.with_conn(|c| seed_regions(c, &parse_seed(DEFAULT_SEED_JSON)?))
        .unwrap();
    let source = FakeAdapter::new("alpha").with_listings(1);
    let mut page = source.page(1, "Casa. Valor total R$ 350 mil");
    page.location_text = Some("Setor Bueno, Goiânia".into());
    let source = source.script(1, Script::Page(Box::new(page)));
    run(&d, vec![source.arc()]).await;
    let (municipality, neighborhood): (String, String) =
        d.db.with_conn(|c| {
            Ok(c.query_row(
                "SELECT municipality, neighborhood FROM properties",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?)
        })
        .unwrap();
    assert_eq!(
        (municipality.as_str(), neighborhood.as_str()),
        ("Goiânia", "Setor Bueno")
    );
}

#[tokio::test]
async fn the_sources_calibration_decides_whether_its_price_field_is_comparable() {
    for (confidence, comparable) in [(Confidence::Low, false), (Confidence::Medium, true)] {
        let d = deps();
        let mut source = FakeAdapter::new("alpha").with_listings(1);
        source.config.price_field_confidence = confidence;
        let mut page = source.page(1, "Casa 3 quartos");
        page.price_text = Some("R$ 350.000".into());
        let source = source.script(1, Script::Page(Box::new(page)));
        run(&d, vec![source.arc()]).await;

        let stored =
            d.db.with_conn(|c| find_listing(c, "alpha", "https://alpha.test/imovel/1"))
                .unwrap()
                .unwrap();
        let price = stored.price.expect("the headline price is stored");
        assert_eq!(
            (price.classification, price.confidence),
            (PriceClassification::FullPrice, confidence)
        );
        let page =
            d.db.with_conn(|c| {
                list_properties(
                    c,
                    &PropertyQuery {
                        full_price_only: true,
                        ..PropertyQuery::default()
                    },
                )
            })
            .unwrap();
        assert_eq!(page.total == 1, comparable, "calibration {confidence}");
    }
}

#[tokio::test]
async fn the_per_source_listing_cap_is_enforced_and_reported() {
    let d = deps();
    let mut source = FakeAdapter::new("alpha").with_listings(5);
    source.config.max_listings_per_run = 2;
    let report = run(&d, vec![source.arc()]).await;
    let o = outcome(&report, "alpha");
    assert_eq!((o.discovered, o.stored, o.complete), (2, 2, false));
    assert!(
        o.warnings.iter().any(|w| w.contains("limit")),
        "{:?}",
        o.warnings
    );
}

#[tokio::test]
async fn the_request_can_lower_the_cap_but_never_raise_it() {
    let d = deps();
    let source = FakeAdapter::new("alpha").with_listings(5);
    let report = run_collection(
        &d,
        &[source.arc()],
        &SearchRequest {
            max_listings: Some(3),
            ..SearchRequest::default()
        },
        &CancelToken::new(),
        None,
    )
    .await
    .unwrap();
    assert_eq!(outcome(&report, "alpha").stored, 3);

    let mut capped = FakeAdapter::new("beta").with_listings(5);
    capped.config.max_listings_per_run = 2;
    let report = run_collection(
        &d,
        &[capped.arc()],
        &SearchRequest {
            max_listings: Some(1_000),
            ..SearchRequest::default()
        },
        &CancelToken::new(),
        None,
    )
    .await
    .unwrap();
    assert_eq!(outcome(&report, "beta").stored, 2);
}

#[tokio::test]
async fn duplicate_references_are_fetched_once() {
    let d = deps();
    let source = FakeAdapter::new("alpha").with_listings(2);
    let base = source.url(1);
    *source.discovery.lock().unwrap() = Ok(Discovery {
        refs: vec![
            ListingRef {
                url: base.clone(),
                source_listing_id: None,
            },
            ListingRef {
                url: format!("{base}?utm_source=mail#x"),
                source_listing_id: None,
            },
            ListingRef {
                url: source.url(2),
                source_listing_id: None,
            },
        ],
        complete: true,
        warnings: vec![],
    });
    let source = Arc::new(source);
    let report = run(&d, vec![Arc::clone(&source) as Arc<dyn SourceAdapter>]).await;
    assert_eq!(source.started.load(Ordering::SeqCst), 2);
    assert_eq!(
        (
            outcome(&report, "alpha").discovered,
            outcome(&report, "alpha").stored
        ),
        (2, 2)
    );
}

#[tokio::test]
async fn references_with_unusable_urls_are_skipped_with_a_warning() {
    let d = deps();
    let source = FakeAdapter::new("alpha").with_listings(1);
    *source.discovery.lock().unwrap() = Ok(Discovery {
        refs: vec![
            ListingRef {
                url: source.url(1),
                source_listing_id: None,
            },
            ListingRef {
                url: "javascript:alert(1)".into(),
                source_listing_id: None,
            },
            ListingRef {
                url: "https://alpha.test/ok?x=1".into(),
                source_listing_id: None,
            },
        ],
        complete: true,
        warnings: vec![],
    });
    let report = run(&d, vec![source.arc()]).await;
    let o = outcome(&report, "alpha");
    assert_eq!(o.stored, 1);
    assert!(o.skipped >= 1);
    assert!(
        o.warnings.iter().any(|w| w.contains("javascript")),
        "{:?}",
        o.warnings
    );
}

#[tokio::test]
async fn a_panicking_adapter_is_contained_and_reported() {
    let d = deps();
    let bad = FakeAdapter::new("bad")
        .with_listings(2)
        .script(1, Script::Panic)
        .script(2, Script::Panic);
    let report = run(
        &d,
        vec![bad.arc(), FakeAdapter::new("good").with_listings(1).arc()],
    )
    .await;
    let o = outcome(&report, "bad");
    assert_eq!(
        (o.kind, o.error_kind.as_deref()),
        (SourceOutcomeKind::Failed, Some("internal_error"))
    );
    assert_eq!(outcome(&report, "good").kind, SourceOutcomeKind::Results);
    assert_eq!(report.status, RunStatus::Completed);
}

#[tokio::test]
async fn progress_is_reported_in_order_and_ends_finished() {
    let d = deps();
    let events: Arc<Mutex<Vec<ProgressEvent>>> = Arc::default();
    let sink: ProgressSink = {
        let events = Arc::clone(&events);
        Arc::new(move |e| events.lock().unwrap().push(e))
    };
    let report = run_collection(
        &d,
        &[FakeAdapter::new("alpha").with_listings(3).arc()],
        &SearchRequest::default(),
        &CancelToken::new(),
        Some(sink),
    )
    .await
    .unwrap();
    let events = events.lock().unwrap();
    assert_eq!(events.first().unwrap().phase, Phase::Discovering);
    let last = events.last().unwrap();
    assert_eq!(
        (last.phase, last.discovered, last.processed, last.stored),
        (Phase::Finished, 3, 3, 3)
    );
    assert!(
        events
            .iter()
            .all(|e| e.run_id == report.run_id && e.adapter_id == "alpha")
    );
    let processed: Vec<u32> = events.iter().map(|e| e.processed).collect();
    assert!(processed.windows(2).all(|w| w[0] <= w[1]), "{processed:?}");
}

#[tokio::test]
async fn stored_listings_can_be_read_back_with_their_evidence() {
    let d = deps();
    run(&d, vec![FakeAdapter::new("alpha").with_listings(1).arc()]).await;
    let l =
        d.db.with_conn(|c| find_listing(c, "alpha", "https://alpha.test/imovel/1"))
            .unwrap()
            .unwrap();
    let full = d.db.with_conn(|c| get_listing(c, l.id)).unwrap().unwrap();
    let text = full.raw_description.as_deref().unwrap();
    for mention in &full.facts.as_ref().unwrap().prices {
        let span = mention.evidence.span;
        assert_eq!(&text[span.start..span.end], mention.evidence.snippet);
    }
    assert_eq!(
        full.price.unwrap().classification,
        PriceClassification::FullPrice
    );
}

// ---- images are queued for processing --------------------------------------------------------

fn adapter_with_images(id: &str, urls: &[&str], cap: Option<usize>) -> FakeAdapter {
    let mut adapter = FakeAdapter::new(id).with_listings(1);
    if let Some(cap) = cap {
        adapter.config.max_images_per_listing = cap;
    }
    let mut page = adapter.page(1, "Casa 3 quartos. Valor total R$ 301 mil");
    page.image_urls = urls.iter().map(|u| (*u).to_owned()).collect();
    adapter.script(1, Script::Page(Box::new(page)))
}

fn queued_images(deps: &CollectionDeps) -> Vec<(i64, String, String)> {
    deps.db
        .with_conn(|c| {
            let mut statement = c.prepare(
                "SELECT position, source_url, processing_status FROM property_images ORDER BY listing_id, position",
            )?;
            let rows = statement
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            Ok(rows)
        })
        .unwrap()
}

#[tokio::test]
async fn a_stored_listing_queues_its_images_for_processing() {
    let d = deps();
    run(
        &d,
        vec![
            adapter_with_images(
                "alpha",
                &["https://alpha.test/a.jpg", "https://alpha.test/b.jpg"],
                None,
            )
            .arc(),
        ],
    )
    .await;
    assert_eq!(
        queued_images(&d),
        vec![
            (
                0,
                "https://alpha.test/a.jpg".to_owned(),
                "pending".to_owned()
            ),
            (
                1,
                "https://alpha.test/b.jpg".to_owned(),
                "pending".to_owned()
            ),
        ]
    );
}

#[tokio::test]
async fn running_again_does_not_queue_the_same_images_twice() {
    let d = deps();
    for _ in 0..2 {
        run(
            &d,
            vec![
                adapter_with_images(
                    "alpha",
                    &["https://alpha.test/a.jpg", "https://alpha.test/b.jpg"],
                    None,
                )
                .arc(),
            ],
        )
        .await;
    }
    assert_eq!(queued_images(&d).len(), 2);
}

#[tokio::test]
async fn a_listing_never_queues_more_images_than_the_source_allows() {
    let d = deps();
    let urls: Vec<String> = (0..6)
        .map(|i| format!("https://alpha.test/{i}.jpg"))
        .collect();
    let refs: Vec<&str> = urls.iter().map(String::as_str).collect();
    run(&d, vec![adapter_with_images("alpha", &refs, Some(3)).arc()]).await;
    let queued = queued_images(&d);
    assert_eq!(queued.len(), 3);
    assert_eq!(
        queued[2].1, "https://alpha.test/2.jpg",
        "the first ones, in the order given"
    );
}

#[tokio::test]
async fn only_plain_web_addresses_are_queued() {
    let d = deps();
    run(
        &d,
        vec![
            adapter_with_images(
                "alpha",
                &[
                    "javascript:alert(1)",
                    "data:image/png;base64,AAAA",
                    "https://alpha.test/ok.jpg",
                    "file:///c:/secret.png",
                ],
                None,
            )
            .arc(),
        ],
    )
    .await;
    let queued = queued_images(&d);
    assert_eq!(queued.len(), 1, "{queued:?}");
    assert_eq!(queued[0].1, "https://alpha.test/ok.jpg");
}

#[tokio::test]
async fn a_listing_without_images_queues_nothing() {
    let d = deps();
    run(&d, vec![FakeAdapter::new("alpha").with_listings(2).arc()]).await;
    assert_eq!(count(&d, "SELECT COUNT(*) FROM property_images"), 0);
}
