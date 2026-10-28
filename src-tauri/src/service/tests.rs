use std::time::Duration;

use serde_json::json;
use tokio::sync::oneshot;

use super::*;
use crate::{
    db::{
        listings::find_listing,
        regions::list_regions,
        runs::{get_run, list_runs, start_run},
        testkit::at,
    },
    models::collection::SourceOutcomeKind,
    sources::{
        Discovery, Fetcher, ListingRef, RawListing, SourceConfig, SourceError, adapter::BoxFuture,
    },
};

#[derive(Clone, Copy)]
enum Behavior {
    /// Finds one listing and reads it.
    OneListing,
    /// Never finishes discovery on its own: only cancellation ends it.
    WaitForCancel,
}

struct FakeAdapter {
    id: String,
    config: SourceConfig,
    behavior: Behavior,
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
        cancel: &'a CancelToken,
    ) -> BoxFuture<'a, std::result::Result<Discovery, SourceError>> {
        Box::pin(async move {
            match self.behavior {
                Behavior::OneListing => Ok(Discovery {
                    refs: vec![ListingRef {
                        url: "https://example.test/anuncio/1".to_owned(),
                        source_listing_id: None,
                    }],
                    complete: true,
                    warnings: vec![],
                }),
                Behavior::WaitForCancel => {
                    cancel.cancelled().await;
                    Err(SourceError::Cancelled)
                }
            }
        })
    }

    fn fetch_listing<'a>(
        &'a self,
        _fetcher: &'a Fetcher,
        reference: &'a ListingRef,
        _cancel: &'a CancelToken,
    ) -> BoxFuture<'a, std::result::Result<RawListing, SourceError>> {
        Box::pin(async move {
            Ok(RawListing {
                url: reference.url.clone(),
                title: Some("Casa".to_owned()),
                description: Some("Casa com 3 quartos. Valor total R$ 350 mil".to_owned()),
                // An image host the source is not allowed to use: nothing is downloaded.
                image_urls: vec!["https://img.invalid/foto.png".to_owned()],
                ..RawListing::default()
            })
        })
    }
}

fn adapter(id: &str, behavior: Behavior) -> Arc<dyn SourceAdapter> {
    Arc::new(FakeAdapter {
        id: id.to_owned(),
        config: SourceConfig::new(vec!["example.test".into()]),
        behavior,
    })
}

fn service_with(adapters: Vec<Arc<dyn SourceAdapter>>) -> (Arc<AppService>, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let service = AppService::new(
        Arc::new(Database::open_in_memory().unwrap()),
        adapters,
        dir.path().to_path_buf(),
        Arc::new(|| at(100)),
    );
    (service, dir)
}

fn finished_channel() -> (FinishedSink, oneshot::Receiver<CollectionFinished>) {
    let (sender, receiver) = oneshot::channel();
    (
        Box::new(move |finished| {
            let _ = sender.send(finished);
        }),
        receiver,
    )
}

async fn finished(receiver: oneshot::Receiver<CollectionFinished>) -> CollectionFinished {
    tokio::time::timeout(Duration::from_secs(20), receiver)
        .await
        .expect("the collection finished in time")
        .expect("the finished callback was called")
}

#[test]
fn startup_seeds_the_regions_and_fails_runs_a_dead_process_left_running() {
    let (service, _dir) = service_with(vec![]);
    let run = service
        .db()
        .with_conn(|c| start_run(c, &json!({}), at(0)))
        .unwrap();

    let first = service.startup().unwrap();
    assert!(first.regions.inserted > 0, "{first:?}");
    assert_eq!(first.interrupted_runs, 1);
    let summary = service
        .db()
        .with_conn(|c| get_run(c, run))
        .unwrap()
        .unwrap();
    assert_eq!(summary.run.status, RunStatus::Failed);
    assert!(
        summary
            .run
            .note
            .as_deref()
            .is_some_and(|n| n.contains("interrupted"))
    );
    let regions = service.db().with_conn(|c| list_regions(c, false)).unwrap();
    assert_ne!(regions.len(), 0, "the catalogue was seeded");

    let second = service.startup().unwrap();
    assert_eq!(
        (second.regions.inserted, second.interrupted_runs),
        (0, 0),
        "startup is idempotent"
    );
    assert_eq!(
        service
            .db()
            .with_conn(|c| list_regions(c, false))
            .unwrap()
            .len(),
        regions.len()
    );
}

#[tokio::test]
async fn without_sources_a_collection_is_refused_and_creates_no_run() {
    let (service, _dir) = service_with(vec![]);
    let error = service
        .start_collection(SearchRequest::default(), None, None)
        .unwrap_err();
    assert!(matches!(error, Error::NoSourcesConfigured), "{error}");
    assert_eq!(error.kind(), "no_sources_configured");
    let runs = service.db().with_conn(|c| list_runs(c, 10)).unwrap();
    assert_eq!(runs.len(), 0, "{runs:?}");
    assert!(
        !service.collection_running(),
        "a refused start must not leave the service busy"
    );
}

#[tokio::test]
async fn a_collection_runs_in_the_background_and_images_and_matching_follow() {
    let (service, _dir) = service_with(vec![adapter("fake", Behavior::OneListing)]);
    let (sink, receiver) = finished_channel();
    service
        .start_collection(SearchRequest::default(), None, Some(sink))
        .unwrap();
    let done = finished(receiver).await;

    assert_eq!(done.error, None);
    assert_eq!(done.status, Some(RunStatus::Completed));
    assert_eq!(done.outcomes.len(), 1);
    assert_eq!(
        (done.outcomes[0].kind, done.outcomes[0].stored),
        (SourceOutcomeKind::Results, 1)
    );
    let images = done
        .images
        .expect("images were processed after the collection");
    assert_eq!(
        (images.attempted, images.skipped),
        (0, 1),
        "a host outside the allow-list is skipped, not fetched"
    );
    let matching = done.matching.expect("matching followed");
    assert_eq!(matching.listings, 1);
    assert!(!service.collection_running());
    let stored = service
        .db()
        .with_conn(|c| find_listing(c, "fake", "https://example.test/anuncio/1"))
        .unwrap();
    assert!(stored.is_some());
}

#[tokio::test]
async fn only_one_collection_runs_at_a_time_and_cancelling_stops_it() {
    let (service, _dir) = service_with(vec![adapter("slow", Behavior::WaitForCancel)]);
    let (sink, receiver) = finished_channel();
    service
        .start_collection(SearchRequest::default(), None, Some(sink))
        .unwrap();
    assert!(service.collection_running());

    let second = service
        .start_collection(SearchRequest::default(), None, None)
        .unwrap_err();
    assert!(
        matches!(second, Error::CollectionAlreadyRunning),
        "{second}"
    );
    assert_eq!(second.kind(), "collection_already_running");

    assert!(service.cancel_collection());
    let done = finished(receiver).await;
    assert_eq!(done.status, Some(RunStatus::Cancelled));
    assert!(!service.collection_running());

    // The service is usable again.
    let (sink, receiver) = finished_channel();
    service
        .start_collection(SearchRequest::default(), None, Some(sink))
        .unwrap();
    assert!(service.cancel_collection());
    assert_eq!(finished(receiver).await.status, Some(RunStatus::Cancelled));
}

#[tokio::test]
async fn cancelling_with_nothing_running_does_nothing() {
    let (service, _dir) = service_with(vec![adapter("fake", Behavior::OneListing)]);
    assert!(!service.cancel_collection());
    assert!(!service.collection_running());
}

#[tokio::test]
async fn sources_list_the_registered_adapters_with_their_latest_outcome() {
    let (service, _dir) = service_with(vec![adapter("fake", Behavior::OneListing)]);
    let before = service.sources().unwrap();
    assert_eq!(before.len(), 1);
    assert_eq!(
        (before[0].adapter_id.as_str(), before[0].platform.as_str()),
        ("fake", "Fake")
    );
    assert_eq!(
        before[0].latest_outcome, None,
        "never run is not the same as failed"
    );

    let (sink, receiver) = finished_channel();
    service
        .start_collection(SearchRequest::default(), None, Some(sink))
        .unwrap();
    finished(receiver).await;
    let after = service.sources().unwrap();
    let outcome = after[0]
        .latest_outcome
        .as_ref()
        .expect("the run's outcome is shown");
    assert_eq!(
        (outcome.kind, outcome.stored, outcome.complete),
        (SourceOutcomeKind::Results, 1, true)
    );
}

#[test]
fn the_service_serialises_for_the_frontend_in_camel_case() {
    let (service, _dir) = service_with(vec![adapter("fake", Behavior::OneListing)]);
    let info = serde_json::to_value(service.sources().unwrap()).unwrap();
    assert_eq!(info[0]["adapterId"], "fake");
    assert!(info[0]["latestOutcome"].is_null());
    let startup = serde_json::to_value(service.startup().unwrap()).unwrap();
    assert!(startup["interruptedRuns"].is_number());
}

#[tokio::test]
async fn the_service_is_already_free_when_the_finished_callback_runs() {
    let (service, _dir) = service_with(vec![adapter("fake", Behavior::OneListing)]);
    let (sender, receiver) = oneshot::channel();
    let again = Arc::clone(&service);
    let sink: FinishedSink = Box::new(move |_| {
        // The callback may start the next collection straight away.
        let started = again.start_collection(SearchRequest::default(), None, None);
        let _ = sender.send(started.map_err(|e| e.kind()));
    });
    service
        .start_collection(SearchRequest::default(), None, Some(sink))
        .unwrap();
    let started = tokio::time::timeout(Duration::from_secs(20), receiver)
        .await
        .expect("the callback ran in time")
        .expect("the callback reported");
    assert_eq!(
        started,
        Ok(()),
        "the callback could not start the next collection"
    );
    // Let that second collection finish before the test ends.
    for _ in 0..500 {
        if !service.collection_running() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(!service.collection_running());
}

// ---- local ranking -------------------------------------------------------------------------

mod ranking {
    use super::*;
    use crate::{
        db::{
            matches::{NewMatch, accept_match, get_match, store_candidate, undo_merge},
            properties::{PropertyQuery, list_properties},
            settings::set_setting,
        },
        matching::testkit::{FULL_AD, add},
        models::{
            confidence::Confidence,
            matching::{DecisionStatus, MatchCategory, MatchEvidence},
        },
        ranking::{
            config::{RANKING_CONFIG_KEY, RankingConfig},
            score::{ComponentKind, UnavailableReason},
            store::load_score,
        },
    };

    fn propose(service: &AppService, a: i64, b: i64) -> i64 {
        let evidence = MatchEvidence {
            rule_version: "match_rules/1".into(),
            signals: vec![],
            conflicts: vec![],
            unknown: vec![],
            image_pairs: vec![],
            notes: vec![],
        };
        let new = NewMatch {
            listing_a_id: a,
            listing_b_id: b,
            category: MatchCategory::High,
            min_image_distance: None,
            evidence: &evidence,
            inputs: json!({}),
        };
        service
            .db()
            .with_conn(|c| store_candidate(c, &new, at(1)))
            .unwrap()
            .match_id
    }

    fn cross_listing_reason(service: &AppService, property: i64) -> Option<UnavailableReason> {
        let stored = service
            .db()
            .with_conn(|c| load_score(c, property))
            .unwrap()
            .unwrap();
        stored
            .details
            .expect("the property is scored")
            .components
            .into_iter()
            .find(|c| c.kind == ComponentKind::CrossListing)
            .unwrap()
            .unavailable_reason
    }

    #[tokio::test]
    async fn a_finished_collection_leaves_the_library_scored() {
        let (service, _dir) = service_with(vec![adapter("fake", Behavior::OneListing)]);
        let (sink, receiver) = finished_channel();
        service
            .start_collection(SearchRequest::default(), None, Some(sink))
            .unwrap();
        let done = finished(receiver).await;

        assert_eq!(done.follow_up_errors, Vec::<String>::new());
        let ranking = done.ranking.expect("ranking followed the matching pass");
        assert_eq!(ranking.scored + ranking.unscored, 1);
        let page = service
            .db()
            .with_conn(|c| list_properties(c, &PropertyQuery::default()))
            .unwrap();
        assert!(page.items[0].score_details.is_some());
        assert!(page.items[0].data_confidence.is_some());
    }

    #[test]
    fn recomputing_on_demand_scores_what_is_stored() {
        let (service, _dir) = service_with(vec![]);
        let (_, property) = add(service.db(), "alpha", 1, "Casa", FULL_AD);
        let report = service.recompute_ranking().unwrap();
        assert_eq!(report.scored + report.unscored, 1);
        let stored = service
            .db()
            .with_conn(|c| load_score(c, property))
            .unwrap()
            .unwrap();
        assert_eq!(
            stored.scored_at,
            Some(at(100)),
            "the service clock decides the time"
        );
        assert!(stored.confidence.is_some_and(|c| c <= Confidence::Medium));
    }

    #[tokio::test]
    async fn merging_and_undoing_refresh_the_scores_in_the_same_call() {
        let (service, _dir) = service_with(vec![]);
        let (la, pa) = add(service.db(), "alpha", 1, "Casa", FULL_AD);
        let (lb, _) = add(service.db(), "beta", 1, "Casa", FULL_AD);
        let id = propose(&service, la, lb);
        service.recompute_ranking().unwrap();
        assert_eq!(
            cross_listing_reason(&service, pa),
            Some(UnavailableReason::NotVerified),
            "a proposal is not a verification"
        );

        service
            .blocking_then_rerank(move |c| accept_match(c, id, None, at(5)))
            .await
            .unwrap();
        assert_eq!(
            cross_listing_reason(&service, pa),
            None,
            "the accepted merge makes the price difference between sources a scored component"
        );

        service
            .blocking_then_rerank(move |c| undo_merge(c, id, None, at(9)))
            .await
            .unwrap();
        assert_eq!(
            cross_listing_reason(&service, pa),
            Some(UnavailableReason::NotVerified)
        );
    }

    #[tokio::test]
    async fn a_failed_refresh_never_hides_that_the_decision_was_saved() {
        let (service, _dir) = service_with(vec![]);
        let (la, _) = add(service.db(), "alpha", 1, "Casa", FULL_AD);
        let (lb, _) = add(service.db(), "beta", 1, "Casa", FULL_AD);
        let id = propose(&service, la, lb);
        let broken = RankingConfig {
            weight_price_per_m2: 0.0,
            weight_cross_listing: 0.0,
            weight_fit: 0.0,
            weight_freshness: 0.0,
            ..RankingConfig::default()
        };
        service
            .db()
            .with_conn(|c| set_setting(c, RANKING_CONFIG_KEY, &broken, at(2)))
            .unwrap();

        let error = service
            .blocking_then_rerank(move |c| accept_match(c, id, None, at(5)))
            .await
            .unwrap_err();
        assert_eq!(error.kind(), "ranking_refresh_failed", "{error}");
        let decided = service
            .db()
            .with_conn(|c| get_match(c, id))
            .unwrap()
            .expect("the match exists");
        assert_eq!(
            decided.decision_status,
            DecisionStatus::Accepted,
            "the merge decision stands"
        );
    }

    #[tokio::test]
    async fn a_failure_of_the_change_itself_is_reported_as_that_failure() {
        let (service, _dir) = service_with(vec![]);
        let error = service
            .blocking_then_rerank(|c| accept_match(c, 4242, None, at(5)))
            .await
            .unwrap_err();
        assert_ne!(error.kind(), "ranking_refresh_failed", "{error}");
    }

    fn fit_score(service: &AppService, property: i64) -> Option<f64> {
        service
            .db()
            .with_conn(|c| load_score(c, property))
            .unwrap()
            .unwrap()
            .details
            .expect("scored")
            .components
            .into_iter()
            .find(|c| c.kind == ComponentKind::Fit)
            .unwrap()
            .score
    }

    #[test]
    fn with_nothing_saved_the_preferences_are_empty_and_the_weights_are_the_defaults() {
        let (service, _dir) = service_with(vec![]);
        assert!(service.search_preferences().unwrap().is_empty());
        assert_eq!(service.ranking_config().unwrap(), RankingConfig::default());
    }

    #[tokio::test]
    async fn saved_preferences_are_kept_and_rescore_the_library_at_once() {
        let (service, _dir) = service_with(vec![]);
        let (_, property) = add(service.db(), "alpha", 1, "Casa", FULL_AD);
        service.recompute_ranking().unwrap();
        assert_eq!(fit_score(&service, property), None);

        let wants = SearchPreferences {
            bedrooms_min: Some(3),
            suites_min: Some(1),
            ..SearchPreferences::default()
        };
        let report = service
            .save_search_preferences(wants.clone())
            .await
            .unwrap();
        assert_eq!(report.scored + report.unscored, 1);
        assert_eq!(service.search_preferences().unwrap(), wants);
        assert_eq!(
            fit_score(&service, property),
            Some(100.0),
            "three bedrooms and a suite meet the stated minimums"
        );
    }

    #[tokio::test]
    async fn invalid_preferences_are_refused_and_nothing_is_saved() {
        let (service, _dir) = service_with(vec![]);
        let backwards = SearchPreferences {
            bedrooms_min: Some(5),
            bedrooms_max: Some(2),
            ..SearchPreferences::default()
        };
        let error = service
            .save_search_preferences(backwards)
            .await
            .unwrap_err();
        assert_eq!(error.kind(), "invalid_config", "{error}");
        assert!(service.search_preferences().unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_saved_configuration_changes_the_weights_and_an_invalid_one_is_refused() {
        let (service, _dir) = service_with(vec![]);
        add(service.db(), "alpha", 1, "Casa", FULL_AD);
        let mine = RankingConfig {
            weight_freshness: 0.9,
            ..RankingConfig::default()
        };
        service.save_ranking_config(mine.clone()).await.unwrap();
        assert_eq!(service.ranking_config().unwrap(), mine);

        let nothing = RankingConfig {
            weight_price_per_m2: 0.0,
            weight_cross_listing: 0.0,
            weight_fit: 0.0,
            weight_freshness: 0.0,
            ..RankingConfig::default()
        };
        let error = service.save_ranking_config(nothing).await.unwrap_err();
        assert_eq!(error.kind(), "invalid_config", "{error}");
        assert_eq!(
            service.ranking_config().unwrap(),
            mine,
            "the previous configuration is kept"
        );
    }
}
