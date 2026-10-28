//! Tests for migration 0003 (app settings, ranking provenance, AI prioritisation cache).

use rusqlite::{Connection, ErrorCode, params};

use super::*;

const TS: &str = "2026-03-01T12:00:00.000Z";
const FINGERPRINT_A: &str = "a3f5c0d1e2b4a69788796a5b4c3d2e1f00112233445566778899aabbccddeeff";
const FINGERPRINT_B: &str = "b3f5c0d1e2b4a69788796a5b4c3d2e1f00112233445566778899aabbccddeeff";

fn v2_database() -> Connection {
    let mut conn = Connection::open_in_memory().unwrap();
    configure(&conn, false).unwrap();
    migrate_with(&mut conn, &MIGRATIONS[..2]).unwrap();
    assert_eq!(schema_version(&conn).unwrap(), 2);
    conn
}

fn latest_database() -> Connection {
    let mut conn = Connection::open_in_memory().unwrap();
    configure(&conn, false).unwrap();
    migrate(&mut conn).unwrap();
    conn
}

fn count(c: &Connection, table: &str) -> i64 {
    c.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
        .unwrap()
}

fn table_exists(c: &Connection, name: &str) -> bool {
    c.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
        [name],
        |r| r.get::<_, i64>(0),
    )
    .unwrap()
        == 1
}

fn column_exists(c: &Connection, table: &str, column: &str) -> bool {
    c.prepare(&format!("PRAGMA table_info({table})"))
        .unwrap()
        .query_map([], |r| r.get::<_, String>(1))
        .unwrap()
        .any(|name| name.unwrap() == column)
}

fn constraint_failure(c: &Connection, sql: &str, args: &[&dyn rusqlite::ToSql]) -> String {
    match c.execute(sql, args) {
        Ok(_) => panic!("accepted but must be rejected:\n{sql}"),
        Err(e) => {
            let violation = matches!(&e, rusqlite::Error::SqliteFailure(f, _) if f.code == ErrorCode::ConstraintViolation);
            assert!(
                violation,
                "expected a constraint violation, got: {e}\n{sql}"
            );
            e.to_string()
        }
    }
}

fn new_property(c: &Connection) -> i64 {
    c.execute("INSERT INTO properties DEFAULT VALUES", [])
        .unwrap();
    c.last_insert_rowid()
}

fn new_prioritization(c: &Connection, fingerprint: &str) -> i64 {
    c.execute(
        "INSERT INTO ai_prioritizations (fingerprint, model, prompt_version, candidate_count, response_json, created_at)
         VALUES (?1, 'gemini-test', 'v1', 2, '{}', ?2)",
        params![fingerprint, TS],
    )
    .unwrap();
    c.last_insert_rowid()
}

fn new_note(c: &Connection, property: i64, prioritization: i64, rank: i64) {
    c.execute(
        "INSERT INTO ai_property_notes
            (property_id, prioritization_id, property_fingerprint, ai_rank, explanation,
             positives_json, concerns_json, uncertainties_json)
         VALUES (?1, ?2, ?3, ?4, 'Resumo', '[]', '[]', '[]')",
        params![property, prioritization, FINGERPRINT_A, rank],
    )
    .unwrap();
}

#[test]
fn upgrading_from_v2_keeps_every_row_and_adds_the_new_structures() {
    let mut c = v2_database();
    let p = new_property(&c);
    c.execute(
        "UPDATE properties SET local_score = 61.5, data_confidence = 'medium' WHERE id = ?1",
        [p],
    )
    .unwrap();
    c.execute(
        "INSERT INTO collection_runs (started_at, request_json) VALUES (?1, '{}')",
        [TS],
    )
    .unwrap();
    assert!(!table_exists(&c, "app_settings"));

    migrate(&mut c).unwrap();

    assert_eq!(schema_version(&c).unwrap(), LATEST_SCHEMA_VERSION);
    const { assert!(LATEST_SCHEMA_VERSION >= 3) };
    assert_eq!(count(&c, "properties"), 1);
    assert_eq!(count(&c, "collection_runs"), 1);
    let score: f64 = c
        .query_row(
            "SELECT local_score FROM properties WHERE id = ?1",
            [p],
            |r| r.get(0),
        )
        .unwrap();
    assert!(
        (score - 61.5).abs() < f64::EPSILON,
        "an existing score survives the upgrade"
    );
    for table in ["app_settings", "ai_prioritizations", "ai_property_notes"] {
        assert!(table_exists(&c, table), "missing {table}");
    }
    assert!(column_exists(&c, "properties", "ranking_version"));
    assert!(column_exists(&c, "properties", "scored_at"));
    let (version, scored_at): (Option<String>, Option<String>) = c
        .query_row(
            "SELECT ranking_version, scored_at FROM properties WHERE id = ?1",
            [p],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!((version, scored_at), (None, None));
}

#[test]
fn upgrading_twice_changes_nothing() {
    let mut c = v2_database();
    migrate(&mut c).unwrap();
    c.execute(
        "INSERT INTO app_settings (key, value_json, updated_at) VALUES ('k', '{}', ?1)",
        [TS],
    )
    .unwrap();
    migrate(&mut c).unwrap();
    assert_eq!(count(&c, "app_settings"), 1);
    assert_eq!(schema_version(&c).unwrap(), LATEST_SCHEMA_VERSION);
}

#[test]
fn settings_need_a_key_and_valid_json_and_keys_are_unique() {
    let c = latest_database();
    let insert = "INSERT INTO app_settings (key, value_json, updated_at) VALUES (?1, ?2, ?3)";
    c.execute(
        insert,
        params!["search_preferences", r#"{"bedrooms_min":2}"#, TS],
    )
    .unwrap();
    constraint_failure(&c, insert, &[&"", &"{}", &TS]);
    constraint_failure(&c, insert, &[&"bad_json", &"{ nope", &TS]);
    constraint_failure(&c, insert, &[&"search_preferences", &"{}", &TS]);
    constraint_failure(&c, insert, &[&"x".repeat(101), &"{}", &TS]);
}

#[test]
fn a_prioritization_has_a_unique_sha256_sized_fingerprint_and_valid_documents() {
    let c = latest_database();
    new_prioritization(&c, FINGERPRINT_A);
    let insert = "INSERT INTO ai_prioritizations
                    (fingerprint, model, prompt_version, candidate_count, response_json, created_at)
                  VALUES (?1, ?2, ?3, ?4, ?5, ?6)";
    constraint_failure(&c, insert, &[&FINGERPRINT_A, &"m", &"v1", &1, &"{}", &TS]);
    constraint_failure(&c, insert, &[&"too-short", &"m", &"v1", &1, &"{}", &TS]);
    constraint_failure(&c, insert, &[&FINGERPRINT_B, &"m", &"v1", &0, &"{}", &TS]);
    constraint_failure(
        &c,
        insert,
        &[&FINGERPRINT_B, &"m", &"v1", &1, &"{ nope", &TS],
    );
    constraint_failure(&c, insert, &[&FINGERPRINT_B, &"", &"v1", &1, &"{}", &TS]);
    c.execute(insert, params![FINGERPRINT_B, "m", "v1", 1, "{}", TS])
        .unwrap();
    assert_eq!(count(&c, "ai_prioritizations"), 2);
}

#[test]
fn a_note_belongs_to_a_property_and_a_prioritization_once() {
    let c = latest_database();
    let (p1, p2) = (new_property(&c), new_property(&c));
    let prioritization = new_prioritization(&c, FINGERPRINT_A);
    new_note(&c, p1, prioritization, 1);
    new_note(&c, p2, prioritization, 2);
    let insert = "INSERT INTO ai_property_notes
                    (property_id, prioritization_id, property_fingerprint, ai_rank, explanation,
                     positives_json, concerns_json, uncertainties_json)
                  VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)";
    // The same property twice in one prioritization.
    constraint_failure(
        &c,
        insert,
        &[
            &p1,
            &prioritization,
            &FINGERPRINT_A,
            &3,
            &"x",
            &"[]",
            &"[]",
            &"[]",
        ],
    );
    // Rank starts at 1, the explanation is not empty, the fingerprint has its fixed size, JSON is valid.
    let p3 = new_property(&c);
    constraint_failure(
        &c,
        insert,
        &[
            &p3,
            &prioritization,
            &FINGERPRINT_A,
            &0,
            &"x",
            &"[]",
            &"[]",
            &"[]",
        ],
    );
    constraint_failure(
        &c,
        insert,
        &[
            &p3,
            &prioritization,
            &FINGERPRINT_A,
            &1,
            &"",
            &"[]",
            &"[]",
            &"[]",
        ],
    );
    constraint_failure(
        &c,
        insert,
        &[
            &p3,
            &prioritization,
            &"short",
            &1,
            &"x",
            &"[]",
            &"[]",
            &"[]",
        ],
    );
    constraint_failure(
        &c,
        insert,
        &[
            &p3,
            &prioritization,
            &FINGERPRINT_A,
            &1,
            &"x",
            &"nope",
            &"[]",
            &"[]",
        ],
    );
    // Unknown references.
    constraint_failure(
        &c,
        insert,
        &[
            &9999,
            &prioritization,
            &FINGERPRINT_A,
            &1,
            &"x",
            &"[]",
            &"[]",
            &"[]",
        ],
    );
    constraint_failure(
        &c,
        insert,
        &[&p3, &9999, &FINGERPRINT_A, &1, &"x", &"[]", &"[]", &"[]"],
    );
}

#[test]
fn history_is_never_deleted_by_cascade() {
    let c = latest_database();
    let property = new_property(&c);
    let prioritization = new_prioritization(&c, FINGERPRINT_A);
    new_note(&c, property, prioritization, 1);
    let message = constraint_failure(&c, "DELETE FROM properties WHERE id = ?1", &[&property]);
    assert!(message.contains("FOREIGN KEY"), "{message}");
    let message = constraint_failure(
        &c,
        "DELETE FROM ai_prioritizations WHERE id = ?1",
        &[&prioritization],
    );
    assert!(message.contains("FOREIGN KEY"), "{message}");
    assert_eq!(count(&c, "ai_property_notes"), 1);
}

#[test]
fn the_score_columns_can_carry_their_ranking_version() {
    let c = latest_database();
    let p = new_property(&c);
    c.execute(
        "UPDATE properties SET local_score = 72.0, local_score_json = '{}', data_confidence = 'high',
                ranking_version = 'ranking/1', scored_at = ?2 WHERE id = ?1",
        params![p, TS],
    )
    .unwrap();
    let (version, scored): (String, String) = c
        .query_row(
            "SELECT ranking_version, scored_at FROM properties WHERE id = ?1",
            [p],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!((version.as_str(), scored.as_str()), ("ranking/1", TS));
}
