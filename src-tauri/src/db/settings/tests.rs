use serde::Deserialize;

use super::*;
use crate::{
    db::testkit::{at, mem},
    error::Error,
};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Sample {
    bedrooms_min: Option<u8>,
    label: String,
}

fn sample(label: &str) -> Sample {
    Sample {
        bedrooms_min: Some(2),
        label: label.to_owned(),
    }
}

#[test]
fn a_setting_that_was_never_saved_is_none_or_the_default() {
    let db = mem();
    assert_eq!(
        db.with_conn(|c| get_setting::<Sample>(c, "prefs")).unwrap(),
        None
    );
    assert_eq!(
        db.with_conn(|c| get_setting_or_default::<Sample>(c, "prefs"))
            .unwrap(),
        Sample::default()
    );
    assert_eq!(
        db.with_conn(|c| setting_updated_at(c, "prefs")).unwrap(),
        None
    );
}

#[test]
fn a_saved_setting_round_trips() {
    let db = mem();
    db.with_conn(|c| set_setting(c, "prefs", &sample("a"), at(10)))
        .unwrap();
    assert_eq!(
        db.with_conn(|c| get_setting::<Sample>(c, "prefs")).unwrap(),
        Some(sample("a"))
    );
    assert_eq!(
        db.with_conn(|c| setting_updated_at(c, "prefs")).unwrap(),
        Some(at(10))
    );
}

#[test]
fn saving_again_replaces_the_value_and_the_time_not_the_row_count() {
    let db = mem();
    db.with_conn(|c| set_setting(c, "prefs", &sample("a"), at(10)))
        .unwrap();
    db.with_conn(|c| set_setting(c, "prefs", &sample("b"), at(20)))
        .unwrap();
    assert_eq!(
        db.with_conn(|c| get_setting::<Sample>(c, "prefs")).unwrap(),
        Some(sample("b"))
    );
    assert_eq!(
        db.with_conn(|c| setting_updated_at(c, "prefs")).unwrap(),
        Some(at(20))
    );
    let rows: i64 = db
        .with_conn(|c| Ok(c.query_row("SELECT COUNT(*) FROM app_settings", [], |r| r.get(0))?))
        .unwrap();
    assert_eq!(rows, 1);
}

#[test]
fn settings_are_independent_by_key() {
    let db = mem();
    db.with_conn(|c| set_setting(c, "one", &sample("1"), at(1)))
        .unwrap();
    db.with_conn(|c| set_setting(c, "two", &sample("2"), at(2)))
        .unwrap();
    assert_eq!(
        db.with_conn(|c| get_setting::<Sample>(c, "one")).unwrap(),
        Some(sample("1"))
    );
    assert_eq!(
        db.with_conn(|c| get_setting::<Sample>(c, "two")).unwrap(),
        Some(sample("2"))
    );
}

#[test]
fn a_document_of_the_wrong_shape_is_an_error_not_silently_the_default() {
    let db = mem();
    db.with_conn(|c| {
        c.execute(
            "INSERT INTO app_settings (key, value_json, updated_at) VALUES ('prefs', '{\"unexpected\": 1}', '2026-03-01T00:00:00.000Z')",
            [],
        )?;
        Ok(())
    })
    .unwrap();
    let error = db
        .with_conn(|c| get_setting::<Sample>(c, "prefs"))
        .unwrap_err();
    assert!(matches!(error, Error::Json(_)), "{error}");
    assert!(
        db.with_conn(|c| get_setting_or_default::<Sample>(c, "prefs"))
            .is_err()
    );
}

#[test]
fn an_invalid_key_is_refused_by_the_database() {
    let db = mem();
    assert!(
        db.with_conn(|c| set_setting(c, "", &sample("a"), at(1)))
            .is_err()
    );
    assert!(
        db.with_conn(|c| set_setting(c, &"k".repeat(101), &sample("a"), at(1)))
            .is_err()
    );
}
