use serde_json::json;

use super::*;
use crate::{
    db::{
        Database,
        listings::{ListingWrite, upsert_listing},
        properties::{effective_status, set_user_status},
        testkit::{at, mem, normalized},
    },
    error::Error,
    models::{
        listing::UserStatus,
        matching::{ImagePair, Signal, SignalKind},
    },
};

/// Creates a listing (and its property candidate); returns `(listing id, property id)`.
fn listing(db: &Database, adapter: &str, n: u32, text: &str) -> (i64, i64) {
    let l = normalized(adapter, &format!("https://{adapter}.test/{n}"), text, 0);
    let outcome = db
        .with_conn(|c| {
            upsert_listing(
                c,
                &ListingWrite {
                    listing: &l,
                    source_fields: None,
                    location: None,
                },
            )
        })
        .unwrap();
    (outcome.listing_id, outcome.property_id)
}

fn plain(db: &Database, n: u32) -> (i64, i64) {
    listing(db, "alpha", n, "Casa Valor total R$ 350 mil")
}

fn evidence() -> MatchEvidence {
    MatchEvidence {
        rule_version: "match_rules/1".into(),
        signals: vec![Signal {
            kind: SignalKind::SameMunicipality,
            detail: "goiania".into(),
        }],
        conflicts: vec![],
        unknown: vec!["bedrooms unknown in listing B".into()],
        image_pairs: vec![ImagePair {
            image_a: 1,
            image_b: 2,
            distance: 3,
        }],
        notes: vec![],
    }
}

fn propose_with(
    db: &Database,
    a: i64,
    b: i64,
    category: MatchCategory,
    ev: &MatchEvidence,
) -> Stored {
    let new = NewMatch {
        listing_a_id: a,
        listing_b_id: b,
        category,
        min_image_distance: Some(3),
        evidence: ev,
        inputs: json!({ "a": { "neighborhood": "setor bueno" }, "b": { "neighborhood": "setor bueno" } }),
    };
    db.with_conn(|c| store_candidate(c, &new, at(10))).unwrap()
}

fn propose(db: &Database, a: i64, b: i64, category: MatchCategory) -> i64 {
    propose_with(db, a, b, category, &evidence()).match_id
}

fn accept(db: &Database, id: i64) -> Result<AcceptOutcome> {
    db.with_conn(|c| accept_match(c, id, Some("looks right"), at(20)))
}

fn undo(db: &Database, id: i64) -> Result<UndoOutcome> {
    db.with_conn(|c| undo_merge(c, id, None, at(30)))
}

fn reject(db: &Database, id: i64) -> Result<RejectOutcome> {
    db.with_conn(|c| reject_match(c, id, None, at(25)))
}

fn merged_into(db: &Database, property: i64) -> Option<i64> {
    db.with_conn(|c| {
        Ok(c.query_row(
            "SELECT merged_into_property_id FROM properties WHERE id = ?1",
            [property],
            |r| r.get(0),
        )?)
    })
    .unwrap()
}

fn members(db: &Database, root: i64) -> Vec<i64> {
    db.with_conn(|c| {
        let mut s = c.prepare(
            "SELECT member_id FROM property_groups WHERE root_id = ?1 ORDER BY member_id",
        )?;
        let rows = s
            .query_map([root], |r| r.get(0))?
            .collect::<std::result::Result<Vec<i64>, _>>()?;
        Ok(rows)
    })
    .unwrap()
}

fn text_column(db: &Database, sql: &str, id: i64) -> String {
    db.with_conn(|c| Ok(c.query_row(sql, [id], |r| r.get(0))?))
        .unwrap()
}

fn review_status(db: &Database, property: i64) -> String {
    text_column(
        db,
        "SELECT match_review_status FROM properties WHERE id = ?1",
        property,
    )
}

fn events(db: &Database, id: i64) -> Vec<MergeEventRecord> {
    db.with_conn(|c| merge_events(c, id)).unwrap()
}

fn row(db: &Database, id: i64) -> MatchView {
    db.with_conn(|c| get_match(c, id))
        .unwrap()
        .expect("match exists")
}

fn refused(error: &Error) -> bool {
    matches!(error, Error::MatchRefused(_))
}

// ------------------------------------------------------------------------------------------
// Storing proposals
// ------------------------------------------------------------------------------------------

#[test]
fn rust_enums_and_the_sql_constraints_accept_the_same_values() {
    let db = mem();
    let (a, _) = plain(&db, 1);
    let (b, _) = plain(&db, 2);
    let insert = |category: &str, status: &str, source: Option<&str>| {
        db.with_conn(|c| {
            let reviewed = source.map(|_| "2026-01-01T00:00:00.000Z");
            c.execute(
                "INSERT INTO property_matches
                    (listing_a_id, listing_b_id, evidence_json, confidence_category, decision_status, decision_source, reviewed_at)
                 VALUES (?1, ?2, '{}', ?3, ?4, ?5, ?6)",
                rusqlite::params![a, b, category, status, source, reviewed],
            )?;
            c.execute("DELETE FROM property_matches", [])?;
            Ok(())
        })
    };
    for category in MatchCategory::ALL {
        insert(category.as_str(), "candidate", None).unwrap_or_else(|e| panic!("{category}: {e}"));
    }
    for status in DecisionStatus::ALL {
        let decided = matches!(status, DecisionStatus::Accepted | DecisionStatus::Rejected);
        insert("possible", status.as_str(), decided.then_some("user"))
            .unwrap_or_else(|e| panic!("{status}: {e}"));
    }
    for source in DecisionSource::ALL {
        insert("high", "accepted", Some(source.as_str()))
            .unwrap_or_else(|e| panic!("{source}: {e}"));
    }
    // The schema itself only lets the matcher decide on its own at high confidence.
    assert!(insert("possible", "accepted", Some("auto")).is_err());
}

#[test]
fn a_proposal_is_stored_with_its_evidence_rule_version_and_inputs() {
    let db = mem();
    let (a, _) = plain(&db, 1);
    let (b, _) = plain(&db, 2);
    let stored = propose_with(&db, a, b, MatchCategory::Possible, &evidence());
    assert_eq!(stored.outcome, StoreOutcome::Created);
    let view = row(&db, stored.match_id);
    assert_eq!(
        (view.category, view.decision_status),
        (MatchCategory::Possible, DecisionStatus::ReviewRequired)
    );
    assert_eq!(view.evidence, evidence());
    assert_eq!(view.evidence.rule_version, "match_rules/1");
    assert_eq!((view.a.listing_id, view.b.listing_id), (a, b));
    assert_eq!(view.min_image_distance, Some(3));
    assert_eq!((view.decision_source, view.merge), (None, None));
    let inputs = text_column(
        &db,
        "SELECT metadata_signals_json FROM property_matches WHERE id = ?1",
        stored.match_id,
    );
    assert!(inputs.contains("setor bueno"), "{inputs}");
}

#[test]
fn only_review_worthy_categories_wait_for_the_user() {
    let db = mem();
    let (a, _) = plain(&db, 1);
    let (b, _) = plain(&db, 2);
    let (c, _) = plain(&db, 3);
    let possible = propose(&db, a, b, MatchCategory::Possible);
    let high = propose(&db, a, c, MatchCategory::High);
    let low = propose(&db, b, c, MatchCategory::Low);
    assert_eq!(
        row(&db, possible).decision_status,
        DecisionStatus::ReviewRequired
    );
    assert_eq!(
        row(&db, high).decision_status,
        DecisionStatus::ReviewRequired
    );
    assert_eq!(row(&db, low).decision_status, DecisionStatus::Candidate);
}

#[test]
fn storing_the_same_proposal_again_changes_nothing() {
    let db = mem();
    let (a, _) = plain(&db, 1);
    let (b, _) = plain(&db, 2);
    let first = propose_with(&db, a, b, MatchCategory::Possible, &evidence());
    let again = propose_with(&db, a, b, MatchCategory::Possible, &evidence());
    assert_eq!(
        (again.match_id, again.outcome),
        (first.match_id, StoreOutcome::Unchanged)
    );
}

#[test]
fn a_revised_proposal_updates_an_undecided_pair() {
    let db = mem();
    let (a, _) = plain(&db, 1);
    let (b, _) = plain(&db, 2);
    let first = propose_with(&db, a, b, MatchCategory::Low, &evidence());
    let mut richer = evidence();
    richer.notes.push("a new photo matched".into());
    let revised = propose_with(&db, a, b, MatchCategory::High, &richer);
    assert_eq!(
        (revised.match_id, revised.outcome),
        (first.match_id, StoreOutcome::Updated)
    );
    let view = row(&db, first.match_id);
    assert_eq!(
        (view.category, view.decision_status),
        (MatchCategory::High, DecisionStatus::ReviewRequired)
    );
    assert_eq!(view.evidence, richer);
}

#[test]
fn a_decided_pair_is_never_re_proposed() {
    let db = mem();
    let (a, _) = plain(&db, 1);
    let (b, _) = plain(&db, 2);
    let id = propose(&db, a, b, MatchCategory::Possible);
    reject(&db, id).unwrap();
    let mut changed = evidence();
    changed.notes.push("new evidence".into());
    let again = propose_with(&db, a, b, MatchCategory::High, &changed);
    assert_eq!(
        (again.match_id, again.outcome),
        (id, StoreOutcome::DecisionKept)
    );
    let view = row(&db, id);
    assert_eq!(
        (view.category, view.decision_status),
        (MatchCategory::Possible, DecisionStatus::Rejected)
    );
    assert_eq!(
        view.evidence,
        evidence(),
        "the evidence the user judged is kept"
    );
}

#[test]
fn listings_must_be_given_in_ascending_order() {
    let db = mem();
    let (a, _) = plain(&db, 1);
    let (b, _) = plain(&db, 2);
    let ev = evidence();
    let new = NewMatch {
        listing_a_id: b,
        listing_b_id: a,
        category: MatchCategory::Possible,
        min_image_distance: None,
        evidence: &ev,
        inputs: json!({}),
    };
    let error = db
        .with_conn(|c| store_candidate(c, &new, at(1)))
        .unwrap_err();
    assert!(refused(&error), "{error}");
}

// ------------------------------------------------------------------------------------------
// Accepting: merge
// ------------------------------------------------------------------------------------------

#[test]
fn accepting_folds_the_newer_property_into_the_older_and_records_everything() {
    let db = mem();
    let (a, pa) = plain(&db, 1);
    let (b, pb) = plain(&db, 2);
    let id = propose(&db, a, b, MatchCategory::High);
    assert_eq!(
        accept(&db, id).unwrap(),
        AcceptOutcome::Merged {
            surviving: pa,
            absorbed: pb
        }
    );

    assert_eq!(merged_into(&db, pb), Some(pa));
    assert_eq!(merged_into(&db, pa), None);
    assert_eq!(members(&db, pa), vec![pa, pb]);
    let view = row(&db, id);
    assert_eq!(
        (view.decision_status, view.decision_source),
        (DecisionStatus::Accepted, Some(DecisionSource::User))
    );
    assert_eq!(view.decision_note.as_deref(), Some("looks right"));
    assert_eq!(
        view.merge,
        Some(MergeInfo {
            surviving_property_id: pa,
            absorbed_property_id: pb
        })
    );
    assert!(view.reviewed_at.is_some() && view.reverted_at.is_none());

    // Listings keep their original property.
    let owners: Vec<i64> = db
        .with_conn(|c| {
            let mut s = c.prepare("SELECT property_id FROM listing_links ORDER BY id")?;
            Ok(s.query_map([], |r| r.get(0))?
                .collect::<std::result::Result<_, _>>()?)
        })
        .unwrap();
    assert_eq!(owners, vec![pa, pb]);

    let log = events(&db, id);
    assert_eq!(log.len(), 1);
    assert_eq!(
        (
            log[0].event.as_str(),
            log[0].surviving_property_id,
            log[0].absorbed_property_id
        ),
        ("merged", pa, pb)
    );
    assert_eq!(
        (log[0].actor.as_str(), log[0].rule_version.as_deref()),
        ("user", Some("match_rules/1"))
    );
    assert!(
        log[0].snapshot["surviving"]["id"] == pa && log[0].snapshot["absorbed"]["id"] == pb,
        "{}",
        log[0].snapshot
    );
}

#[test]
fn the_older_property_survives_whichever_listing_is_a() {
    let db = mem();
    let (a, pa) = plain(&db, 1);
    let (b, pb) = plain(&db, 2);
    let (c, pc) = plain(&db, 3);
    // Merge the two newer ones first; then the oldest joins them.
    let bc = propose(&db, b, c, MatchCategory::High);
    assert_eq!(
        accept(&db, bc).unwrap(),
        AcceptOutcome::Merged {
            surviving: pb,
            absorbed: pc
        }
    );
    let ab = propose(&db, a, b, MatchCategory::High);
    assert_eq!(
        accept(&db, ab).unwrap(),
        AcceptOutcome::Merged {
            surviving: pa,
            absorbed: pb
        }
    );
    assert_eq!(
        members(&db, pa),
        vec![pa, pb, pc],
        "a chain: c into b into a"
    );
}

#[test]
fn the_older_group_survives_even_when_listing_a_sits_in_the_newer_group() {
    let db = mem();
    let (l1, p1) = plain(&db, 1);
    let (l2, p2) = plain(&db, 2);
    let (l3, p3) = plain(&db, 3);
    let m13 = propose(&db, l1, l3, MatchCategory::High);
    let m23 = propose(&db, l2, l3, MatchCategory::High);
    accept(&db, m13).unwrap(); // 3 -> 1
    // Listing a (2) is in the newer group, listing b (3) in the older one: the older root wins.
    assert_eq!(
        accept(&db, m23).unwrap(),
        AcceptOutcome::Merged {
            surviving: p1,
            absorbed: p2
        }
    );
    assert_eq!(members(&db, p1), vec![p1, p2, p3]);
}

#[test]
fn accepting_twice_is_a_no_op_with_a_single_audit_event() {
    let db = mem();
    let (a, _) = plain(&db, 1);
    let (b, _) = plain(&db, 2);
    let id = propose(&db, a, b, MatchCategory::High);
    accept(&db, id).unwrap();
    assert_eq!(accept(&db, id).unwrap(), AcceptOutcome::AlreadyMerged);
    assert_eq!(events(&db, id).len(), 1);
}

#[test]
fn a_pair_already_in_one_group_is_left_untouched() {
    let db = mem();
    let (a, _) = plain(&db, 1);
    let (b, _) = plain(&db, 2);
    let (c, _) = plain(&db, 3);
    let ab = propose(&db, a, b, MatchCategory::High);
    let ac = propose(&db, a, c, MatchCategory::High);
    let bc = propose(&db, b, c, MatchCategory::Possible);
    accept(&db, ab).unwrap();
    accept(&db, ac).unwrap();
    assert_eq!(accept(&db, bc).unwrap(), AcceptOutcome::AlreadySameGroup);
    assert_eq!(
        row(&db, bc).decision_status,
        DecisionStatus::ReviewRequired,
        "no decision was invented"
    );
    let bc_events = events(&db, bc);
    assert_eq!(bc_events.len(), 0, "{bc_events:?}");
    // Because it carries no merge of its own, undoing another merge cannot leave it inconsistent.
    undo(&db, ab).unwrap();
    assert_eq!(row(&db, bc).decision_status, DecisionStatus::ReviewRequired);
}

#[test]
fn a_missing_match_is_refused() {
    let db = mem();
    for result in [
        accept(&db, 99).map(|_| ()),
        reject(&db, 99).map(|_| ()),
        undo(&db, 99).map(|_| ()),
    ] {
        assert!(refused(&result.unwrap_err()));
    }
}

#[test]
fn merging_refreshes_the_canonical_values_of_the_group_and_undoing_restores_them() {
    let db = mem();
    let (a, pa) = listing(&db, "alpha", 1, "Casa 3 quartos Valor total R$ 350 mil");
    let (b, pb) = listing(&db, "beta", 1, "Casa 2 quartos Valor total R$ 350 mil");
    let bedrooms = |p: i64| -> Option<i64> {
        db.with_conn(|c| {
            Ok(
                c.query_row("SELECT bedrooms FROM properties WHERE id = ?1", [p], |r| {
                    r.get(0)
                })?,
            )
        })
        .unwrap()
    };
    assert_eq!((bedrooms(pa), bedrooms(pb)), (Some(3), Some(2)));
    let id = propose(&db, a, b, MatchCategory::Possible);
    accept(&db, id).unwrap();
    assert_eq!(
        bedrooms(pa),
        None,
        "the two ads disagree, so the group does not pick a winner"
    );
    undo(&db, id).unwrap();
    assert_eq!((bedrooms(pa), bedrooms(pb)), (Some(3), Some(2)));
}

// ------------------------------------------------------------------------------------------
// Undo
// ------------------------------------------------------------------------------------------

#[test]
fn undoing_clears_the_pointer_and_records_a_reverted_decision() {
    let db = mem();
    let (a, pa) = plain(&db, 1);
    let (b, pb) = plain(&db, 2);
    let id = propose(&db, a, b, MatchCategory::High);
    accept(&db, id).unwrap();
    assert_eq!(
        undo(&db, id).unwrap(),
        UndoOutcome::Reverted {
            surviving: pa,
            absorbed: pb
        }
    );

    assert_eq!(
        (merged_into(&db, pb), members(&db, pa), members(&db, pb)),
        (None, vec![pa], vec![pb])
    );
    let view = row(&db, id);
    assert_eq!(
        (view.decision_status, view.decision_source),
        (DecisionStatus::Rejected, Some(DecisionSource::User))
    );
    assert!(view.reverted_at.is_some());
    assert_eq!(
        view.merge,
        Some(MergeInfo {
            surviving_property_id: pa,
            absorbed_property_id: pb
        }),
        "what was undone stays on record"
    );
    let log = events(&db, id);
    assert_eq!(
        log.iter().map(|e| e.event.as_str()).collect::<Vec<_>>(),
        vec!["merged", "reverted"]
    );
}

#[test]
fn undoing_twice_is_a_no_op_with_a_single_reverted_event() {
    let db = mem();
    let (a, _) = plain(&db, 1);
    let (b, _) = plain(&db, 2);
    let id = propose(&db, a, b, MatchCategory::High);
    accept(&db, id).unwrap();
    undo(&db, id).unwrap();
    assert_eq!(undo(&db, id).unwrap(), UndoOutcome::AlreadyReverted);
    assert_eq!(events(&db, id).len(), 2);
}

#[test]
fn there_is_nothing_to_undo_without_a_live_merge() {
    let db = mem();
    let (a, _) = plain(&db, 1);
    let (b, _) = plain(&db, 2);
    let id = propose(&db, a, b, MatchCategory::High);
    assert!(refused(&undo(&db, id).unwrap_err()), "never merged");
    reject(&db, id).unwrap();
    assert!(
        refused(&undo(&db, id).unwrap_err()),
        "rejected without ever merging"
    );
}

#[test]
fn a_merge_can_be_redone_after_an_undo_and_every_cycle_is_audited() {
    let db = mem();
    let (a, pa) = plain(&db, 1);
    let (b, pb) = plain(&db, 2);
    let id = propose(&db, a, b, MatchCategory::High);
    for _ in 0..3 {
        assert_eq!(
            accept(&db, id).unwrap(),
            AcceptOutcome::Merged {
                surviving: pa,
                absorbed: pb
            }
        );
        undo(&db, id).unwrap();
    }
    accept(&db, id).unwrap();
    let kinds: Vec<String> = events(&db, id).into_iter().map(|e| e.event).collect();
    assert_eq!(
        kinds,
        vec![
            "merged", "reverted", "merged", "reverted", "merged", "reverted", "merged"
        ]
    );
    let view = row(&db, id);
    assert_eq!(
        (view.decision_status, view.reverted_at),
        (DecisionStatus::Accepted, None)
    );
    assert_eq!(members(&db, pa), vec![pa, pb]);
}

#[test]
fn chains_can_be_undone_in_any_order_without_breaking_the_rest() {
    let db = mem();
    let ids: Vec<(i64, i64)> = (1..=4).map(|n| plain(&db, n)).collect();
    let (p1, p2, p3, p4) = (ids[0].1, ids[1].1, ids[2].1, ids[3].1);
    let m12 = propose(&db, ids[0].0, ids[1].0, MatchCategory::High);
    let m34 = propose(&db, ids[2].0, ids[3].0, MatchCategory::High);
    let m13 = propose(&db, ids[0].0, ids[2].0, MatchCategory::High);
    accept(&db, m12).unwrap(); // 2 -> 1
    accept(&db, m34).unwrap(); // 4 -> 3
    assert_eq!(
        accept(&db, m13).unwrap(),
        AcceptOutcome::Merged {
            surviving: p1,
            absorbed: p3
        }
    ); // 3 -> 1
    assert_eq!(members(&db, p1), vec![p1, p2, p3, p4]);

    // Undo the middle link first: 3 leaves with its own child 4; 1 keeps 2.
    undo(&db, m13).unwrap();
    assert_eq!(
        (members(&db, p1), members(&db, p3)),
        (vec![p1, p2], vec![p3, p4])
    );
    // Redo it, then undo the earliest and the latest merges.
    accept(&db, m13).unwrap();
    undo(&db, m12).unwrap();
    assert_eq!(
        (members(&db, p1), members(&db, p2)),
        (vec![p1, p3, p4], vec![p2])
    );
    undo(&db, m34).unwrap();
    assert_eq!(
        (members(&db, p1), members(&db, p4)),
        (vec![p1, p3], vec![p4])
    );
    assert_eq!(
        (merged_into(&db, p3), merged_into(&db, p4)),
        (Some(p1), None)
    );
}

#[test]
fn merges_never_form_cycles_whatever_order_pairs_are_accepted() {
    let db = mem();
    let l: Vec<(i64, i64)> = (1..=4).map(|n| plain(&db, n)).collect();
    let pairs = [(0, 1), (2, 3), (1, 3), (0, 2), (0, 3), (1, 2)];
    let matches: Vec<i64> = pairs
        .iter()
        .map(|&(i, j)| propose(&db, l[i].0, l[j].0, MatchCategory::High))
        .collect();
    for id in matches {
        accept(&db, id).unwrap();
    }
    let root = l[0].1;
    assert_eq!(
        members(&db, root),
        l.iter().map(|x| x.1).collect::<Vec<_>>()
    );
    let roots: i64 = db
        .with_conn(|c| {
            Ok(c.query_row(
                "SELECT COUNT(*) FROM properties WHERE merged_into_property_id IS NULL",
                [],
                |r| r.get(0),
            )?)
        })
        .unwrap();
    assert_eq!(roots, 1, "exactly one root remains");
}

// ------------------------------------------------------------------------------------------
// User choices across merges
// ------------------------------------------------------------------------------------------

#[test]
fn each_side_keeps_its_own_status_through_a_merge_and_its_undo() {
    let db = mem();
    let (a, pa) = plain(&db, 1);
    let (b, pb) = plain(&db, 2);
    let status = |p: i64| text_column(&db, "SELECT user_status FROM properties WHERE id = ?1", p);
    db.with_conn(|c| set_user_status(c, pa, UserStatus::Favorite, at(1)))
        .unwrap();
    db.with_conn(|c| set_user_status(c, pb, UserStatus::Hidden, at(1)))
        .unwrap();
    let id = propose(&db, a, b, MatchCategory::High);
    accept(&db, id).unwrap();

    // The group shows the favourite and flags the disagreement; nothing is rewritten.
    let effective = db.with_conn(|c| effective_status(c, pa)).unwrap().unwrap();
    assert_eq!(
        (effective.status, effective.conflict),
        (UserStatus::Favorite, true)
    );
    assert_eq!(
        (status(pa).as_str(), status(pb).as_str()),
        ("favorite", "hidden")
    );
    let log = events(&db, id);
    assert_eq!(log[0].snapshot["surviving"]["user_status"], "favorite");
    assert_eq!(log[0].snapshot["absorbed"]["user_status"], "hidden");

    undo(&db, id).unwrap();
    assert_eq!(
        (status(pa).as_str(), status(pb).as_str()),
        ("favorite", "hidden")
    );
}

#[test]
fn a_choice_made_on_the_group_covers_every_member_and_is_not_undone_by_the_undo() {
    let db = mem();
    let (a, pa) = plain(&db, 1);
    let (b, pb) = plain(&db, 2);
    let id = propose(&db, a, b, MatchCategory::High);
    accept(&db, id).unwrap();
    db.with_conn(|c| set_user_status(c, pb, UserStatus::Hidden, at(40)))
        .unwrap();
    undo(&db, id).unwrap();
    let status = |p: i64| text_column(&db, "SELECT user_status FROM properties WHERE id = ?1", p);
    assert_eq!(
        (status(pa).as_str(), status(pb).as_str()),
        ("hidden", "hidden"),
        "the user hid the whole group"
    );
}

// ------------------------------------------------------------------------------------------
// Rejecting
// ------------------------------------------------------------------------------------------

#[test]
fn rejecting_keeps_the_pair_apart_and_is_idempotent() {
    let db = mem();
    let (a, pa) = plain(&db, 1);
    let (b, pb) = plain(&db, 2);
    let id = propose(&db, a, b, MatchCategory::Possible);
    assert_eq!(reject(&db, id).unwrap(), RejectOutcome::Rejected);
    assert_eq!(reject(&db, id).unwrap(), RejectOutcome::AlreadyRejected);
    let view = row(&db, id);
    assert_eq!(
        (view.decision_status, view.decision_source),
        (DecisionStatus::Rejected, Some(DecisionSource::User))
    );
    assert_eq!((view.merge, view.reverted_at), (None, None));
    assert_eq!((merged_into(&db, pa), merged_into(&db, pb)), (None, None));
    assert!(
        events(&db, id).is_empty(),
        "nothing was merged, so nothing is audited as a merge"
    );
}

#[test]
fn a_live_merge_must_be_undone_before_it_can_be_rejected() {
    let db = mem();
    let (a, pa) = plain(&db, 1);
    let (b, pb) = plain(&db, 2);
    let id = propose(&db, a, b, MatchCategory::High);
    accept(&db, id).unwrap();
    assert!(refused(&reject(&db, id).unwrap_err()));
    assert_eq!(merged_into(&db, pb), Some(pa), "still merged");
    undo(&db, id).unwrap();
    assert_eq!(
        reject(&db, id).unwrap(),
        RejectOutcome::AlreadyRejected,
        "the undo already rejected it"
    );
}

#[test]
fn a_user_may_change_their_mind_after_rejecting() {
    let db = mem();
    let (a, pa) = plain(&db, 1);
    let (b, pb) = plain(&db, 2);
    let id = propose(&db, a, b, MatchCategory::Possible);
    reject(&db, id).unwrap();
    assert_eq!(
        accept(&db, id).unwrap(),
        AcceptOutcome::Merged {
            surviving: pa,
            absorbed: pb
        }
    );
    assert_eq!(row(&db, id).decision_status, DecisionStatus::Accepted);
}

// ------------------------------------------------------------------------------------------
// Atomicity
// ------------------------------------------------------------------------------------------

fn break_the_audit_log(db: &Database) {
    db.with_conn(|c| {
        c.execute_batch(
            "CREATE TRIGGER test_block_events BEFORE INSERT ON property_merge_events
             BEGIN SELECT RAISE(ABORT, 'audit log unavailable'); END;",
        )?;
        Ok(())
    })
    .unwrap();
}

#[test]
fn a_failure_while_merging_leaves_everything_as_it_was() {
    let db = mem();
    let (a, pa) = plain(&db, 1);
    let (b, pb) = plain(&db, 2);
    let id = propose(&db, a, b, MatchCategory::High);
    break_the_audit_log(&db);
    let error = accept(&db, id).unwrap_err();
    assert!(matches!(error, Error::Database(_)), "{error}");
    assert_eq!((merged_into(&db, pa), merged_into(&db, pb)), (None, None));
    let view = row(&db, id);
    assert_eq!(
        (view.decision_status, view.merge, view.reviewed_at),
        (DecisionStatus::ReviewRequired, None, None)
    );
}

#[test]
fn a_failure_while_undoing_leaves_the_merge_intact() {
    let db = mem();
    let (a, pa) = plain(&db, 1);
    let (b, pb) = plain(&db, 2);
    let id = propose(&db, a, b, MatchCategory::High);
    accept(&db, id).unwrap();
    break_the_audit_log(&db);
    assert!(undo(&db, id).is_err());
    assert_eq!(merged_into(&db, pb), Some(pa));
    let view = row(&db, id);
    assert_eq!(
        (view.decision_status, view.reverted_at),
        (DecisionStatus::Accepted, None)
    );
}

#[test]
fn an_undo_refuses_when_the_pointers_no_longer_agree_with_the_record() {
    let db = mem();
    let (a, _) = plain(&db, 1);
    let (b, pb) = plain(&db, 2);
    let (_, pc) = plain(&db, 3);
    let id = propose(&db, a, b, MatchCategory::High);
    accept(&db, id).unwrap();
    // Corrupt the state behind the application's back: the absorbed property points elsewhere.
    db.with_conn(|c| {
        c.execute(
            "UPDATE properties SET merged_into_property_id = ?1 WHERE id = ?2",
            rusqlite::params![pc, pb],
        )?;
        Ok(())
    })
    .unwrap();
    assert!(refused(&undo(&db, id).unwrap_err()));
    assert_eq!(merged_into(&db, pb), Some(pc), "nothing was changed");
    assert_eq!(events(&db, id).len(), 1);
}

// ------------------------------------------------------------------------------------------
// Review status of properties
// ------------------------------------------------------------------------------------------

#[test]
fn properties_show_when_a_match_awaits_review_and_when_it_was_reviewed() {
    let db = mem();
    let (a, pa) = plain(&db, 1);
    let (b, pb) = plain(&db, 2);
    let (c, pc) = plain(&db, 3);
    assert_eq!(review_status(&db, pa), "none");

    propose(&db, b, c, MatchCategory::Low);
    assert_eq!(
        (
            review_status(&db, pb).as_str(),
            review_status(&db, pc).as_str()
        ),
        ("none", "none"),
        "nothing to review"
    );

    let ab = propose(&db, a, b, MatchCategory::Possible);
    assert_eq!(
        (
            review_status(&db, pa).as_str(),
            review_status(&db, pb).as_str()
        ),
        ("review_required", "review_required")
    );
    assert_eq!(review_status(&db, pc), "none");

    reject(&db, ab).unwrap();
    assert_eq!(
        (
            review_status(&db, pa).as_str(),
            review_status(&db, pb).as_str()
        ),
        ("reviewed", "reviewed")
    );

    let ac = propose(&db, a, c, MatchCategory::High);
    assert_eq!(review_status(&db, pa), "review_required");
    accept(&db, ac).unwrap();
    assert_eq!(
        review_status(&db, pa),
        "reviewed",
        "the merged group has been reviewed"
    );
    undo(&db, ac).unwrap();
    assert_eq!(
        (
            review_status(&db, pa).as_str(),
            review_status(&db, pc).as_str()
        ),
        ("reviewed", "reviewed")
    );
}

// ------------------------------------------------------------------------------------------
// Reading
// ------------------------------------------------------------------------------------------

#[test]
fn matches_list_most_certain_first_and_page() {
    let db = mem();
    let l: Vec<i64> = (1..=4).map(|n| plain(&db, n).0).collect();
    let possible = propose(&db, l[0], l[1], MatchCategory::Possible);
    let high = propose(&db, l[0], l[2], MatchCategory::High);
    let low = propose(&db, l[2], l[3], MatchCategory::Low);
    let all = db
        .with_conn(|c| list_matches(c, &MatchQuery::default()))
        .unwrap();
    assert_eq!(all.total, 3);
    assert_eq!(
        all.items.iter().map(|m| m.id).collect::<Vec<_>>(),
        vec![high, possible, low]
    );

    let waiting = db
        .with_conn(|c| {
            list_matches(
                c,
                &MatchQuery {
                    status: Some(DecisionStatus::ReviewRequired),
                    ..MatchQuery::default()
                },
            )
        })
        .unwrap();
    assert_eq!(
        waiting.items.iter().map(|m| m.id).collect::<Vec<_>>(),
        vec![high, possible]
    );

    let second = db
        .with_conn(|c| {
            list_matches(
                c,
                &MatchQuery {
                    limit: 1,
                    offset: 1,
                    ..MatchQuery::default()
                },
            )
        })
        .unwrap();
    assert_eq!(
        (
            second.total,
            second.items.len(),
            second.items[0].id,
            second.offset,
            second.limit
        ),
        (3, 1, possible, 1, 1)
    );
}

#[test]
fn only_actionable_pairs_exclude_listings_already_in_one_group() {
    let db = mem();
    let l: Vec<i64> = (1..=3).map(|n| plain(&db, n).0).collect();
    let ab = propose(&db, l[0], l[1], MatchCategory::Possible);
    let ac = propose(&db, l[0], l[2], MatchCategory::High);
    let bc = propose(&db, l[1], l[2], MatchCategory::Possible);
    accept(&db, ab).unwrap();
    accept(&db, ac).unwrap();
    let waiting = |actionable_only: bool| {
        db.with_conn(|c| {
            list_matches(
                c,
                &MatchQuery {
                    status: Some(DecisionStatus::ReviewRequired),
                    actionable_only,
                    ..MatchQuery::default()
                },
            )
        })
        .unwrap()
        .items
        .into_iter()
        .map(|m| m.id)
        .collect::<Vec<_>>()
    };
    assert_eq!(waiting(false), vec![bc]);
    assert!(
        waiting(true).is_empty(),
        "b and c are now one property; there is nothing to decide"
    );
}

#[test]
fn a_match_view_names_both_listings_with_url_title_price_and_group() {
    let db = mem();
    let (a, pa) = listing(&db, "alpha", 1, "Casa Valor total R$ 350 mil");
    let (b, pb) = listing(&db, "beta", 2, "Casa Valor total R$ 360 mil");
    let id = propose(&db, a, b, MatchCategory::Possible);
    let view = row(&db, id);
    assert_eq!(
        (
            view.a.url.as_str(),
            view.a.adapter_id.as_str(),
            view.a.title.as_deref()
        ),
        ("https://alpha.test/1", "alpha", Some("Casa"))
    );
    assert_eq!(
        (view.b.url.as_str(), view.b.adapter_id.as_str()),
        ("https://beta.test/2", "beta")
    );
    assert_eq!(view.a.price.as_ref().unwrap().amount.centavos(), 35_000_000);
    assert_eq!(view.b.price.as_ref().unwrap().amount.centavos(), 36_000_000);
    assert_eq!((view.a.group_root, view.b.group_root), (pa, pb));
    accept(&db, id).unwrap();
    let merged = row(&db, id);
    assert_eq!(
        (merged.a.group_root, merged.b.group_root),
        (pa, pa),
        "the view follows the merge"
    );
}

#[test]
fn an_unknown_match_is_none() {
    assert_eq!(mem().with_conn(|c| get_match(c, 7)).unwrap(), None);
}

#[test]
fn decided_pairs_are_reported_so_a_run_does_not_re_propose_them() {
    let db = mem();
    let l: Vec<i64> = (1..=4).map(|n| plain(&db, n).0).collect();
    let accepted = propose(&db, l[0], l[1], MatchCategory::High);
    let rejected = propose(&db, l[0], l[2], MatchCategory::Possible);
    propose(&db, l[0], l[3], MatchCategory::Possible); // still waiting
    propose(&db, l[1], l[2], MatchCategory::Low); // kept apart, no decision
    accept(&db, accepted).unwrap();
    reject(&db, rejected).unwrap();
    let decided = db.with_conn(|c| decided_pairs(c)).unwrap();
    assert_eq!(
        decided,
        [(l[0], l[1]), (l[0], l[2])]
            .into_iter()
            .collect::<HashSet<_>>()
    );
}
