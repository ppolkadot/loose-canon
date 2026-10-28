use super::*;
use crate::{
    db::{
        Database,
        listings::mark_unavailable,
        settings::set_setting,
        testkit::{at, mem},
    },
    error::Error,
    matching::testkit::{FULL_AD, add},
    ranking::{
        score::{ComponentKind, UnavailableReason},
        store::load_score,
    },
};

fn run(db: &Database, secs: i64) -> Result<SaveReport> {
    db.with_conn(|c| recompute(c, at(secs)))
}

fn stored(db: &Database, id: i64) -> store::StoredScore {
    db.with_conn(|c| load_score(c, id))
        .unwrap()
        .expect("property exists")
}

fn component(db: &Database, id: i64, kind: ComponentKind) -> score::Component {
    stored(db, id)
        .details
        .expect("details are stored")
        .components
        .into_iter()
        .find(|c| c.kind == kind)
        .expect("all four components are always stored")
}

#[test]
fn an_empty_library_ranks_nothing_and_is_not_an_error() {
    let db = mem();
    assert_eq!(run(&db, 10).unwrap(), SaveReport::default());
}

#[test]
fn active_properties_are_scored_and_the_rest_carry_no_score() {
    let db = mem();
    let (_, live) = add(&db, "alpha", 1, "Casa", FULL_AD);
    let (gone_listing, gone) = add(&db, "alpha", 2, "Casa", FULL_AD);
    db.with_conn(|c| mark_unavailable(c, gone_listing, at(5), "HTTP 410"))
        .unwrap();

    let report = run(&db, 10).unwrap();
    assert_eq!(
        report.scored + report.unscored,
        1,
        "only the property with an active listing is ranked"
    );
    let s = stored(&db, live);
    assert!(s.details.is_some() && s.confidence.is_some());
    assert_eq!(s.scored_at, Some(at(10)));
    let none = stored(&db, gone);
    assert_eq!(
        (none.score, none.confidence, none.details),
        (None, None, None)
    );
}

#[test]
fn a_property_that_stops_being_active_loses_its_score_on_the_next_run() {
    let db = mem();
    let (listing, property) = add(&db, "alpha", 1, "Casa", FULL_AD);
    run(&db, 10).unwrap();
    assert!(stored(&db, property).details.is_some());

    db.with_conn(|c| mark_unavailable(c, listing, at(20), "HTTP 410"))
        .unwrap();
    let report = run(&db, 30).unwrap();
    assert_eq!(report.cleared, 1);
    assert_eq!(stored(&db, property).details, None);
}

#[test]
fn saved_preferences_decide_whether_fit_is_scored() {
    let db = mem();
    let (_, property) = add(&db, "alpha", 1, "Casa", FULL_AD);
    run(&db, 10).unwrap();
    let fit = component(&db, property, ComponentKind::Fit);
    assert_eq!(
        (fit.score, fit.unavailable_reason),
        (None, Some(UnavailableReason::NoPreferences))
    );

    let wants = SearchPreferences {
        bedrooms_min: Some(3),
        ..SearchPreferences::default()
    };
    db.with_conn(|c| set_setting(c, SEARCH_PREFERENCES_KEY, &wants, at(11)))
        .unwrap();
    run(&db, 12).unwrap();
    let fit = component(&db, property, ComponentKind::Fit);
    assert_eq!(
        fit.score,
        Some(100.0),
        "three bedrooms meet a minimum of three"
    );
}

#[test]
fn saved_weights_are_the_ones_stored_with_the_score() {
    let db = mem();
    let (_, property) = add(&db, "alpha", 1, "Casa", FULL_AD);
    let config = RankingConfig {
        weight_price_per_m2: 0.1,
        weight_cross_listing: 0.2,
        weight_fit: 0.3,
        weight_freshness: 0.4,
        ..RankingConfig::default()
    };
    db.with_conn(|c| set_setting(c, RANKING_CONFIG_KEY, &config, at(1)))
        .unwrap();
    run(&db, 10).unwrap();
    let weight = |kind| component(&db, property, kind).weight;
    assert_eq!(
        (
            weight(ComponentKind::PricePerM2),
            weight(ComponentKind::CrossListing),
            weight(ComponentKind::Fit),
            weight(ComponentKind::Freshness)
        ),
        (0.1, 0.2, 0.3, 0.4)
    );
}

#[test]
fn an_invalid_saved_configuration_is_an_error_that_leaves_the_scores_alone() {
    let db = mem();
    let (_, property) = add(&db, "alpha", 1, "Casa", FULL_AD);
    run(&db, 10).unwrap();
    let before = stored(&db, property);

    let broken = RankingConfig {
        weight_price_per_m2: 0.0,
        weight_cross_listing: 0.0,
        weight_fit: 0.0,
        weight_freshness: 0.0,
        ..RankingConfig::default()
    };
    db.with_conn(|c| set_setting(c, RANKING_CONFIG_KEY, &broken, at(11)))
        .unwrap();
    let error = run(&db, 20).unwrap_err();
    assert!(
        matches!(
            error,
            Error::InvalidConfig {
                what: "ranking configuration",
                ..
            }
        ),
        "{error}"
    );
    assert_eq!(stored(&db, property), before);
}

#[test]
fn invalid_saved_preferences_are_an_error_that_leaves_the_scores_alone() {
    let db = mem();
    let (_, property) = add(&db, "alpha", 1, "Casa", FULL_AD);
    run(&db, 10).unwrap();
    let before = stored(&db, property);

    let backwards = SearchPreferences {
        bedrooms_min: Some(4),
        bedrooms_max: Some(2),
        ..SearchPreferences::default()
    };
    db.with_conn(|c| set_setting(c, SEARCH_PREFERENCES_KEY, &backwards, at(11)))
        .unwrap();
    let error = run(&db, 20).unwrap_err();
    assert!(
        matches!(
            error,
            Error::InvalidConfig {
                what: "search preferences",
                ..
            }
        ),
        "{error}"
    );
    assert_eq!(stored(&db, property), before);
}

#[test]
fn recomputing_with_nothing_changed_gives_the_same_scores() {
    let db = mem();
    let ids: Vec<i64> = (1..=3)
        .map(|n| add(&db, "alpha", n, "Casa", FULL_AD).1)
        .collect();
    run(&db, 10).unwrap();
    let first: Vec<_> = ids
        .iter()
        .map(|id| (stored(&db, *id).score, stored(&db, *id).details))
        .collect();
    run(&db, 10).unwrap();
    let again: Vec<_> = ids
        .iter()
        .map(|id| (stored(&db, *id).score, stored(&db, *id).details))
        .collect();
    assert!(
        first.iter().all(|(_, details)| details.is_some()),
        "something was actually stored"
    );
    assert_eq!(first, again);
}
