use serde_json::json;

use super::*;
use crate::{
    db::testkit::{at, mem},
    error::{Error, ModelError},
    models::collection::SourceOutcomeKind,
};

fn outcome(adapter: &str, kind: SourceOutcomeKind, secs: i64) -> SourceOutcome {
    SourceOutcome {
        adapter_id: adapter.to_owned(),
        kind,
        complete: true,
        discovered: 0,
        stored: 0,
        unparseable: 0,
        failed: 0,
        skipped: 0,
        warnings: vec![],
        error_kind: (kind == SourceOutcomeKind::Failed).then(|| "timeout".to_owned()),
        error_detail: None,
        started_at: at(secs),
        finished_at: at(secs + 5),
    }
}

#[test]
fn a_run_starts_running_and_finishes_exactly_once() {
    let db = mem();
    let request = json!({ "regions": ["br-go-goiania"], "full_price_only": true });
    let id = db.with_conn(|c| start_run(c, &request, at(0))).unwrap();
    let run = db.with_conn(|c| get_run(c, id)).unwrap().unwrap().run;
    assert_eq!(
        (run.status, run.finished_at, run.request.clone()),
        (RunStatus::Running, None, request)
    );

    assert!(
        db.with_conn(|c| finish_run(c, id, RunStatus::Completed, at(60), Some("ok")))
            .unwrap()
    );
    let run = db.with_conn(|c| get_run(c, id)).unwrap().unwrap().run;
    assert_eq!(
        (run.status, run.finished_at, run.note.as_deref()),
        (RunStatus::Completed, Some(at(60)), Some("ok"))
    );

    // A second finish must not rewrite what already happened.
    assert!(
        !db.with_conn(|c| finish_run(c, id, RunStatus::Failed, at(90), Some("late")))
            .unwrap()
    );
    let run = db.with_conn(|c| get_run(c, id)).unwrap().unwrap().run;
    assert_eq!(
        (run.status, run.finished_at),
        (RunStatus::Completed, Some(at(60)))
    );
    assert!(
        !db.with_conn(|c| finish_run(c, 999, RunStatus::Completed, at(1), None))
            .unwrap()
    );
}

#[test]
fn a_run_cannot_be_finished_as_running() {
    let db = mem();
    let id = db.with_conn(|c| start_run(c, &json!({}), at(0))).unwrap();
    let err = db
        .with_conn(|c| finish_run(c, id, RunStatus::Running, at(1), None))
        .unwrap_err();
    assert!(
        matches!(err, Error::Model(ModelError::InvalidOutcome(_))),
        "{err}"
    );
}

#[test]
fn outcomes_are_stored_with_everything_needed_to_explain_them() {
    let db = mem();
    let run = db.with_conn(|c| start_run(c, &json!({}), at(0))).unwrap();
    let mut results = outcome("zeta", SourceOutcomeKind::Results, 0);
    results.discovered = 12;
    results.stored = 9;
    results.unparseable = 2;
    results.failed = 1;
    results.skipped = 0;
    results.complete = false;
    results.warnings = vec![
        "rate limited after page 2".into(),
        "2 listings unparseable".into(),
    ];
    let mut failed = outcome("alpha", SourceOutcomeKind::Failed, 1);
    failed.error_kind = Some("http_403".into());
    failed.error_detail = Some("blocked at the edge".into());
    db.with_conn(|c| {
        record_outcome(c, run, &results)?;
        record_outcome(c, run, &failed)?;
        record_outcome(
            c,
            run,
            &outcome("mid", SourceOutcomeKind::SuccessfulEmpty, 2),
        )?;
        Ok(())
    })
    .unwrap();

    let summary = db.with_conn(|c| get_run(c, run)).unwrap().unwrap();
    let adapters: Vec<&str> = summary
        .outcomes
        .iter()
        .map(|o| o.outcome.adapter_id.as_str())
        .collect();
    assert_eq!(adapters, vec!["alpha", "mid", "zeta"], "ordered by source");
    assert_eq!(summary.outcomes[0].outcome, failed);
    assert_eq!(summary.outcomes[2].outcome, results);
    assert_eq!(
        summary.outcomes[1].outcome.kind,
        SourceOutcomeKind::SuccessfulEmpty
    );
}

#[test]
fn one_outcome_per_source_per_run() {
    let db = mem();
    let run = db.with_conn(|c| start_run(c, &json!({}), at(0))).unwrap();
    db.with_conn(|c| record_outcome(c, run, &outcome("a", SourceOutcomeKind::Results, 0)))
        .unwrap();
    let err = db
        .with_conn(|c| record_outcome(c, run, &outcome("a", SourceOutcomeKind::Failed, 1)))
        .unwrap_err();
    assert!(matches!(err, Error::Database(_)), "{err}");
}

#[test]
fn invalid_outcomes_are_rejected_before_they_reach_the_database() {
    let db = mem();
    let run = db.with_conn(|c| start_run(c, &json!({}), at(0))).unwrap();
    let mut bad = outcome("a", SourceOutcomeKind::SuccessfulEmpty, 0);
    bad.complete = false;
    let err = db.with_conn(|c| record_outcome(c, run, &bad)).unwrap_err();
    assert!(
        matches!(err, Error::Model(ModelError::InvalidOutcome(_))),
        "{err}"
    );
    let outcomes = db.with_conn(|c| get_run(c, run)).unwrap().unwrap().outcomes;
    assert_eq!(outcomes.len(), 0, "{outcomes:?}");
}

#[test]
fn outcomes_can_only_be_added_to_a_run_that_is_still_running() {
    let db = mem();
    let run = db.with_conn(|c| start_run(c, &json!({}), at(0))).unwrap();
    db.with_conn(|c| finish_run(c, run, RunStatus::Cancelled, at(10), None))
        .unwrap();
    let err = db
        .with_conn(|c| record_outcome(c, run, &outcome("a", SourceOutcomeKind::Results, 0)))
        .unwrap_err();
    assert!(
        matches!(err, Error::Model(ModelError::InvalidOutcome(_))),
        "{err}"
    );
    let missing = db
        .with_conn(|c| record_outcome(c, 999, &outcome("a", SourceOutcomeKind::Results, 0)))
        .unwrap_err();
    assert!(
        matches!(missing, Error::Model(ModelError::InvalidOutcome(_))),
        "{missing}"
    );
}

#[test]
fn runs_left_running_by_a_dead_process_are_failed_at_startup() {
    let db = mem();
    let stale = db.with_conn(|c| start_run(c, &json!({}), at(0))).unwrap();
    let done = db.with_conn(|c| start_run(c, &json!({}), at(10))).unwrap();
    db.with_conn(|c| finish_run(c, done, RunStatus::Completed, at(20), None))
        .unwrap();

    assert_eq!(
        db.with_conn(|c| fail_interrupted_runs(c, at(1_000)))
            .unwrap(),
        1
    );
    let stale = db.with_conn(|c| get_run(c, stale)).unwrap().unwrap().run;
    assert_eq!(
        (stale.status, stale.finished_at),
        (RunStatus::Failed, Some(at(1_000)))
    );
    assert!(stale.note.unwrap().contains("interrupted"));
    let done = db.with_conn(|c| get_run(c, done)).unwrap().unwrap().run;
    assert_eq!(
        done.status,
        RunStatus::Completed,
        "a finished run is never touched"
    );
    assert_eq!(
        db.with_conn(|c| fail_interrupted_runs(c, at(2_000)))
            .unwrap(),
        0,
        "idempotent"
    );
}

#[test]
fn the_latest_outcome_of_each_source_supersedes_older_ones() {
    let db = mem();
    let first = db.with_conn(|c| start_run(c, &json!({}), at(0))).unwrap();
    db.with_conn(|c| {
        record_outcome(c, first, &outcome("a", SourceOutcomeKind::Failed, 0))?;
        record_outcome(c, first, &outcome("b", SourceOutcomeKind::Results, 0))?;
        finish_run(c, first, RunStatus::Completed, at(10), None)?;
        Ok(())
    })
    .unwrap();
    let second = db.with_conn(|c| start_run(c, &json!({}), at(100))).unwrap();
    db.with_conn(|c| {
        record_outcome(c, second, &outcome("a", SourceOutcomeKind::Results, 100))?;
        finish_run(c, second, RunStatus::Completed, at(110), None)?;
        Ok(())
    })
    .unwrap();

    let latest = db.with_conn(|c| latest_outcomes_by_source(c)).unwrap();
    let by_adapter: Vec<(&str, SourceOutcomeKind, i64)> = latest
        .iter()
        .map(|o| (o.outcome.adapter_id.as_str(), o.outcome.kind, o.run_id))
        .collect();
    assert_eq!(
        by_adapter,
        vec![
            ("a", SourceOutcomeKind::Results, second),
            ("b", SourceOutcomeKind::Results, first)
        ]
    );
}

#[test]
fn recent_problems_lists_failed_unreliable_and_incomplete_outcomes_newest_first() {
    let db = mem();
    let run = db.with_conn(|c| start_run(c, &json!({}), at(0))).unwrap();
    let mut partial = outcome("partial", SourceOutcomeKind::Results, 30);
    partial.complete = false;
    partial.discovered = 4;
    db.with_conn(|c| {
        record_outcome(c, run, &outcome("healthy", SourceOutcomeKind::Results, 0))?;
        record_outcome(
            c,
            run,
            &outcome("empty", SourceOutcomeKind::SuccessfulEmpty, 10),
        )?;
        record_outcome(c, run, &outcome("broken", SourceOutcomeKind::Failed, 20))?;
        record_outcome(c, run, &partial)?;
        record_outcome(c, run, &outcome("messy", SourceOutcomeKind::Unreliable, 40))?;
        Ok(())
    })
    .unwrap();
    let problems = db.with_conn(|c| recent_problems(c, 10)).unwrap();
    let names: Vec<&str> = problems
        .iter()
        .map(|p| p.outcome.adapter_id.as_str())
        .collect();
    assert_eq!(names, vec!["messy", "partial", "broken"]);
    assert_eq!(db.with_conn(|c| recent_problems(c, 2)).unwrap().len(), 2);
}

#[test]
fn runs_are_listed_newest_first() {
    let db = mem();
    for secs in [0, 100, 50] {
        db.with_conn(|c| start_run(c, &json!({ "at": secs }), at(secs)))
            .unwrap();
    }
    let runs = db.with_conn(|c| list_runs(c, 10)).unwrap();
    let starts: Vec<_> = runs.iter().map(|r| r.started_at).collect();
    assert_eq!(starts, vec![at(100), at(50), at(0)]);
    assert_eq!(db.with_conn(|c| list_runs(c, 1)).unwrap().len(), 1);
}

#[test]
fn every_run_status_and_outcome_kind_is_accepted_by_the_database() {
    let db = mem();
    for status in RunStatus::ALL {
        let id = db.with_conn(|c| start_run(c, &json!({}), at(0))).unwrap();
        if *status != RunStatus::Running {
            assert!(
                db.with_conn(|c| finish_run(c, id, *status, at(1), None))
                    .unwrap()
            );
        }
    }
    for kind in SourceOutcomeKind::ALL {
        let run = db.with_conn(|c| start_run(c, &json!({}), at(0))).unwrap();
        db.with_conn(|c| record_outcome(c, run, &outcome("a", *kind, 0)))
            .unwrap();
    }
}
