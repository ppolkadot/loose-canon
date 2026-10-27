//! The Rust side of the contract with `src/lib/api.ts`: the exact JSON keys of every DTO the
//! frontend reads, taken from **real** values (a real database, real repositories). The lists
//! below are copied from the TypeScript interfaces; if a DTO changes, this fails until the
//! interface and these lists are updated together.

use std::collections::BTreeSet;

use serde_json::Value;

use crate::{
    db::{
        Database,
        matches::{
            MatchQuery, NewMatch, accept_match, list_matches, merge_events, store_candidate,
        },
        properties::{PropertyQuery, list_properties},
        regions::{DEFAULT_SEED_JSON, list_regions, parse_seed, seed_regions},
        runs::{finish_run, get_run, record_outcome, start_run},
        testkit::at,
    },
    matching::testkit::{FULL_AD, add_typed, hash_image, place},
    models::{
        characteristics::PropertyType,
        collection::{RunStatus, SourceOutcome, SourceOutcomeKind},
        matching::{ImagePair, MatchCategory, MatchEvidence, Signal, SignalKind},
    },
};

fn keys(value: &Value) -> BTreeSet<&str> {
    value
        .as_object()
        .unwrap_or_else(|| panic!("expected an object, got {value}"))
        .keys()
        .map(String::as_str)
        .collect()
}

#[track_caller]
fn assert_keys(value: &Value, expected: &[&str], what: &str) {
    let expected: BTreeSet<&str> = expected.iter().copied().collect();
    assert_eq!(
        keys(value),
        expected,
        "{what}: the JSON keys differ from src/lib/api.ts"
    );
}

fn evidence() -> MatchEvidence {
    MatchEvidence {
        rule_version: "match_rules/1".into(),
        signals: vec![Signal {
            kind: SignalKind::SameMunicipality,
            detail: "goiania".into(),
        }],
        conflicts: vec![],
        unknown: vec![],
        image_pairs: vec![ImagePair {
            image_a: 1,
            image_b: 2,
            distance: 3,
        }],
        notes: vec![],
    }
}

#[test]
fn saved_results_match_the_typescript_contract() {
    let db = Database::open_in_memory().unwrap();
    let (listing, property) = add_typed(&db, "alpha", 1, FULL_AD, PropertyType::House);
    place(&db, property, "Goiânia", Some("Setor Bueno"));
    hash_image(&db, listing, "https://img.test/a.png", 7);
    let page = db
        .with_conn(|c| list_properties(c, &PropertyQuery::default()))
        .unwrap();
    let json = serde_json::to_value(&page).unwrap();

    assert_keys(&json, &["items", "total", "offset", "limit"], "Page");
    let item = &json["items"][0];
    assert_keys(
        item,
        &[
            "id",
            "status",
            "statusConflict",
            "municipality",
            "neighborhood",
            "propertyType",
            "bedrooms",
            "suites",
            "parkingSpaces",
            "lotArea",
            "builtArea",
            "usableArea",
            "matchReviewStatus",
            "listings",
            "comparablePrices",
            "priceSpread",
            "canonicalProvenance",
        ],
        "PropertyView",
    );
    assert_keys(
        &item["listings"][0],
        &[
            "listingId",
            "sourcePlatform",
            "adapterId",
            "url",
            "title",
            "price",
            "status",
            "extractionStatus",
            "firstSeenAt",
            "lastSeenAt",
            "lastVerifiedAt",
            "lastScrapedAt",
        ],
        "ListingView",
    );
    assert_keys(
        &item["listings"][0]["price"],
        &["amount", "classification", "confidence"],
        "PriceView",
    );
    assert_keys(
        &item["comparablePrices"][0],
        &["listingId", "adapterId", "amount", "confidence"],
        "ComparablePrice",
    );
    // The value shapes the interface promises: money and areas are plain numbers, the spread a pair.
    assert!(item["listings"][0]["price"]["amount"].is_i64());
    assert!(item["builtArea"].is_u64());
    assert!(
        item["priceSpread"]
            .as_array()
            .is_some_and(|pair| pair.len() == 2)
    );
    assert_eq!(item["status"], "new");
    assert_eq!(
        item["propertyType"],
        serde_json::Value::Null,
        "the parser does not extract a type yet"
    );
}

/// A database holding one stored candidate pair; returns it with the match id.
fn database_with_a_candidate() -> (Database, i64) {
    let db = Database::open_in_memory().unwrap();
    let (a, _) = add_typed(&db, "alpha", 1, FULL_AD, PropertyType::House);
    let (b, _) = add_typed(&db, "beta", 1, FULL_AD, PropertyType::House);
    let evidence = evidence();
    let new = NewMatch {
        listing_a_id: a,
        listing_b_id: b,
        category: MatchCategory::High,
        min_image_distance: Some(3),
        evidence: &evidence,
        inputs: serde_json::json!({}),
    };
    let id = db
        .with_conn(|c| store_candidate(c, &new, at(1)))
        .unwrap()
        .match_id;
    (db, id)
}

#[test]
fn match_candidates_match_the_typescript_contract() {
    let (db, _) = database_with_a_candidate();
    let page = db
        .with_conn(|c| list_matches(c, &MatchQuery::default()))
        .unwrap();
    let view = &serde_json::to_value(&page).unwrap()["items"][0];
    assert_keys(
        view,
        &[
            "id",
            "category",
            "decisionStatus",
            "decisionSource",
            "decisionNote",
            "a",
            "b",
            "evidence",
            "minImageDistance",
            "createdAt",
            "reviewedAt",
            "revertedAt",
            "merge",
        ],
        "MatchView",
    );
    assert_keys(
        &view["a"],
        &[
            "listingId",
            "propertyId",
            "groupRoot",
            "adapterId",
            "url",
            "title",
            "price",
        ],
        "ListingBrief",
    );
    // Stored documents keep their stored snake_case shape.
    assert_keys(
        &view["evidence"],
        &[
            "rule_version",
            "signals",
            "conflicts",
            "unknown",
            "image_pairs",
            "notes",
        ],
        "MatchEvidence",
    );
    assert_keys(
        &view["evidence"]["signals"][0],
        &["kind", "detail"],
        "MatchSignal",
    );
    assert_keys(
        &view["evidence"]["image_pairs"][0],
        &["image_a", "image_b", "distance"],
        "image pair",
    );
    assert_eq!(view["category"], "high");
    assert_eq!(view["decisionStatus"], "review_required");
}

#[test]
fn a_merge_and_its_audit_events_match_the_typescript_contract() {
    let (db, id) = database_with_a_candidate();
    db.with_conn(|c| accept_match(c, id, None, at(2))).unwrap();
    let merged = &serde_json::to_value(
        db.with_conn(|c| list_matches(c, &MatchQuery::default()))
            .unwrap(),
    )
    .unwrap()["items"][0];
    assert_keys(
        &merged["merge"],
        &["survivingPropertyId", "absorbedPropertyId"],
        "MergeInfo",
    );
    let events = serde_json::to_value(db.with_conn(|c| merge_events(c, id)).unwrap()).unwrap();
    assert_keys(
        &events[0],
        &[
            "id",
            "matchId",
            "event",
            "survivingPropertyId",
            "absorbedPropertyId",
            "actor",
            "ruleVersion",
            "note",
            "occurredAt",
            "snapshot",
        ],
        "MergeEventRecord",
    );
    assert_eq!(events[0]["event"], "merged");
}

#[test]
fn collection_runs_and_regions_match_the_typescript_contract() {
    let db = Database::open_in_memory().unwrap();
    let run = db
        .with_conn(|c| start_run(c, &serde_json::json!({ "region_slugs": [] }), at(0)))
        .unwrap();
    let outcome = SourceOutcome {
        adapter_id: "alpha".into(),
        kind: SourceOutcomeKind::Results,
        complete: true,
        discovered: 1,
        stored: 1,
        unparseable: 0,
        failed: 0,
        skipped: 0,
        warnings: vec![],
        error_kind: None,
        error_detail: None,
        started_at: at(0),
        finished_at: at(1),
    };
    db.with_conn(|c| record_outcome(c, run, &outcome)).unwrap();
    db.with_conn(|c| finish_run(c, run, RunStatus::Completed, at(2), None))
        .unwrap();
    let summary =
        serde_json::to_value(db.with_conn(|c| get_run(c, run)).unwrap().unwrap()).unwrap();

    assert_keys(&summary, &["run", "outcomes"], "RunSummary");
    assert_keys(
        &summary["run"],
        &["id", "startedAt", "finishedAt", "status", "request", "note"],
        "RunRecord",
    );
    assert_keys(
        &summary["outcomes"][0],
        &["id", "runId", "outcome"],
        "StoredOutcome",
    );
    assert_keys(
        &summary["outcomes"][0]["outcome"],
        &[
            "adapterId",
            "kind",
            "complete",
            "discovered",
            "stored",
            "unparseable",
            "failed",
            "skipped",
            "warnings",
            "errorKind",
            "errorDetail",
            "startedAt",
            "finishedAt",
        ],
        "SourceOutcome",
    );
    assert_eq!(summary["run"]["status"], "completed");
    assert_eq!(summary["outcomes"][0]["outcome"]["kind"], "results");
    // The saved request is a stored document and keeps its snake_case keys.
    assert!(summary["run"]["request"].get("region_slugs").is_some());

    db.with_conn(|c| seed_regions(c, &parse_seed(DEFAULT_SEED_JSON)?))
        .unwrap();
    let regions = serde_json::to_value(db.with_conn(|c| list_regions(c, true)).unwrap()).unwrap();
    assert_keys(
        &regions[0],
        &[
            "id",
            "slug",
            "stateCode",
            "municipality",
            "name",
            "regionType",
            "parentId",
            "enabled",
            "aliases",
        ],
        "RegionRow",
    );
}

#[test]
fn events_and_reports_match_the_typescript_contract() {
    use crate::{
        images::pipeline::ImageReport,
        matching::run::MatchRunReport,
        sources::pipeline::{Phase, ProgressEvent},
    };
    let progress = serde_json::to_value(ProgressEvent {
        run_id: 1,
        adapter_id: "alpha".into(),
        phase: Phase::Fetching,
        discovered: 2,
        processed: 1,
        stored: 1,
    })
    .unwrap();
    assert_keys(
        &progress,
        &[
            "runId",
            "adapterId",
            "phase",
            "discovered",
            "processed",
            "stored",
        ],
        "ProgressEvent",
    );
    assert_eq!(progress["phase"], "fetching");

    let images = serde_json::to_value(ImageReport::default()).unwrap();
    assert_keys(
        &images,
        &[
            "attempted",
            "hashed",
            "failed",
            "skipped",
            "retryLater",
            "unavailableSource",
            "notAttempted",
            "internalErrors",
            "cancelled",
        ],
        "ImageReport",
    );

    let matching = serde_json::to_value(MatchRunReport::default()).unwrap();
    assert_keys(
        &matching,
        &[
            "ruleVersion",
            "listings",
            "candidatePairs",
            "skippedDecided",
            "compared",
            "high",
            "possible",
            "lowStored",
            "lowDiscarded",
            "created",
            "updated",
            "unchanged",
            "decisionsKept",
            "oversizedBlocks",
            "oversizedImageBuckets",
            "commonHashes",
            "cancelled",
        ],
        "MatchRunReport",
    );
}

#[test]
fn finished_collections_and_sources_match_the_typescript_contract() {
    use crate::service::{CollectionFinished, SourceInfo};

    let finished = serde_json::to_value(CollectionFinished {
        run_id: None,
        status: None,
        outcomes: vec![],
        error: None,
        follow_up_errors: vec![],
        images: None,
        matching: None,
    })
    .unwrap();
    assert_keys(
        &finished,
        &[
            "runId",
            "status",
            "outcomes",
            "error",
            "followUpErrors",
            "images",
            "matching",
        ],
        "CollectionFinished",
    );

    let source = serde_json::to_value(SourceInfo {
        adapter_id: "alpha".into(),
        platform: "Alpha".into(),
        latest_outcome: None,
    })
    .unwrap();
    assert_keys(
        &source,
        &["adapterId", "platform", "latestOutcome"],
        "SourceInfo",
    );
}
