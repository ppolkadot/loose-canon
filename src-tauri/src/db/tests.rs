use std::str::FromStr;

use chrono::TimeZone;
use rusqlite::{ErrorCode, params};

use super::{price_history::*, *};
use crate::{
    models::{
        characteristics::PropertyType,
        confidence::Confidence,
        listing::{ExtractionStatus, ListingStatus, UserStatus},
        money::Money,
        price::{PriceClassification, PriceProvenance},
    },
    parser::parse_listing_text,
};

// ---- helpers -------------------------------------------------------------------------

fn mem() -> Database {
    Database::open_in_memory().expect("in-memory database")
}

const TS: &str = "2026-03-01T12:00:00.000Z";

fn insert_property(c: &Connection) -> i64 {
    c.execute("INSERT INTO properties DEFAULT VALUES", [])
        .unwrap();
    c.last_insert_rowid()
}

fn insert_listing_row(
    c: &Connection,
    property_id: Option<i64>,
    adapter: &str,
    url: &str,
) -> rusqlite::Result<i64> {
    c.execute(
        "INSERT INTO listing_links
            (property_id, source_platform, adapter_id, url, normalized_url, first_seen_at, last_seen_at)
         VALUES (?1, ?2, ?3, ?4, ?4, ?5, ?5)",
        params![property_id, adapter.to_uppercase(), adapter, url, TS],
    )?;
    Ok(c.last_insert_rowid())
}

fn insert_listing(c: &Connection, property_id: Option<i64>, adapter: &str, url: &str) -> i64 {
    insert_listing_row(c, property_id, adapter, url).expect("listing insert")
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

fn count(c: &Connection, table: &str) -> i64 {
    c.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
        .unwrap()
}

fn is_constraint_violation(e: &rusqlite::Error) -> bool {
    matches!(e, rusqlite::Error::SqliteFailure(f, _) if f.code == ErrorCode::ConstraintViolation)
}

fn assert_rejected(c: &Connection, description: &str, sql: &str) {
    match c.execute(sql, []) {
        Ok(_) => panic!("accepted but must be rejected: {description}\n{sql}"),
        Err(e) => assert!(
            is_constraint_violation(&e),
            "{description}: expected a constraint violation, got {e}"
        ),
    }
}

/// Like [`assert_rejected`], but the rejection must come from a CHECK or NOT NULL rule, so a
/// case cannot "pass" by accidentally tripping a foreign key instead of the rule under test.
fn assert_check_rejected(c: &Connection, description: &str, sql: &str) {
    match c.execute(sql, []) {
        Ok(_) => panic!("accepted but must be rejected: {description}\n{sql}"),
        Err(e) => {
            let message = e.to_string();
            assert!(
                is_constraint_violation(&e)
                    && (message.contains("CHECK constraint failed")
                        || message.contains("NOT NULL constraint failed")),
                "{description}: expected a CHECK/NOT NULL failure, got: {message}"
            );
        }
    }
}

fn at(secs: i64) -> DateTime<Utc> {
    Utc.timestamp_opt(1_780_000_000 + secs, 0).unwrap()
}

fn observation(reais: i64, class: PriceClassification, secs: i64) -> PriceObservation {
    PriceObservation {
        amount: Money::from_reais(reais).unwrap(),
        classification: class,
        confidence: Some(Confidence::High),
        observed_at: at(secs),
        provenance: PriceProvenance {
            extractor: "br_real_estate/1".into(),
            source_field: Some("description".into()),
            evidence: None,
        },
    }
}

// ---- initialisation & migrations -----------------------------------------------------

#[test]
fn fresh_database_is_migrated_to_latest_with_foreign_keys_enforced() {
    let db = mem();
    assert_eq!(db.schema_version().unwrap(), LATEST_SCHEMA_VERSION);
    assert!(db.foreign_keys_enabled().unwrap());
    db.with_conn(|c| {
        assert_eq!(
            count(c, "schema_migrations"),
            i64::try_from(MIGRATIONS.len()).unwrap(),
            "one audit row per migration"
        );
        let (version, name): (u32, String) = c
            .query_row(
                "SELECT version, name FROM schema_migrations ORDER BY version LIMIT 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!((version, name.as_str()), (1, "initial_schema"));
        Ok(())
    })
    .unwrap();
}

#[test]
fn all_required_tables_exist() {
    let db = mem();
    db.with_conn(|c| {
        for table in [
            "schema_migrations",
            "regions",
            "properties",
            "listing_links",
            "property_images",
            "property_matches",
            "listing_price_history",
        ] {
            assert!(table_exists(c, table), "missing table {table}");
        }
        Ok(())
    })
    .unwrap();
}

#[test]
fn migrations_are_contiguous_and_start_at_one() {
    for (i, m) in MIGRATIONS.iter().enumerate() {
        assert_eq!(
            m.version as usize,
            i + 1,
            "migration {} out of order",
            m.name
        );
        assert_ne!(m.sql.trim(), "");
    }
}

#[test]
fn reopening_a_file_database_keeps_data_and_does_not_reapply_the_schema() {
    let dir = tempfile::tempdir().unwrap();
    let path = database_file(dir.path());
    {
        let db = Database::open(&path).unwrap();
        db.with_conn(|c| {
            c.execute(
                "INSERT INTO regions (slug, state_code, municipality, name, region_type)
                 VALUES ('br-go-goiania', 'GO', 'Goiânia', 'Goiânia', 'municipality')",
                [],
            )?;
            Ok(())
        })
        .unwrap();
    }
    for _ in 0..3 {
        let db = Database::open(&path).unwrap();
        db.with_conn(|c| {
            assert_eq!(
                count(c, "regions"),
                1,
                "user data must survive re-initialisation"
            );
            assert_eq!(
                count(c, "schema_migrations"),
                i64::try_from(MIGRATIONS.len()).unwrap(),
                "schema must not be re-applied"
            );
            Ok(())
        })
        .unwrap();
        assert_eq!(db.schema_version().unwrap(), LATEST_SCHEMA_VERSION);
    }
}

#[test]
fn open_creates_missing_parent_directories() {
    let dir = tempfile::tempdir().unwrap();
    let path = database_file(&dir.path().join("a").join("b"));
    Database::open(&path).unwrap();
    assert!(path.exists());
}

#[test]
fn database_file_is_built_from_the_supplied_directory() {
    let p = database_file(Path::new("some-dir"));
    assert_eq!(p.parent().unwrap(), Path::new("some-dir"));
    assert_eq!(p.file_name().unwrap(), DATABASE_FILE_NAME);
}

#[test]
fn migrating_twice_on_one_connection_is_a_no_op() {
    let db = mem();
    db.with_conn(|c| {
        migrate(c)?;
        migrate(c)?;
        assert_eq!(
            count(c, "schema_migrations"),
            i64::try_from(MIGRATIONS.len()).unwrap()
        );
        Ok(())
    })
    .unwrap();
}

#[test]
fn database_from_a_newer_version_is_refused_and_left_untouched() {
    let dir = tempfile::tempdir().unwrap();
    let path = database_file(dir.path());
    drop(Database::open(&path).unwrap());
    {
        let c = Connection::open(&path).unwrap();
        c.pragma_update(None, "user_version", LATEST_SCHEMA_VERSION + 5)
            .unwrap();
    }
    match Database::open(&path) {
        Err(Error::SchemaTooNew { found, supported }) => {
            assert_eq!(
                (found, supported),
                (LATEST_SCHEMA_VERSION + 5, LATEST_SCHEMA_VERSION)
            );
        }
        Err(other) => panic!("wrong error: {other}"),
        Ok(_) => panic!("must refuse a newer database"),
    }
    let c = Connection::open(&path).unwrap();
    assert_eq!(schema_version(&c).unwrap(), LATEST_SCHEMA_VERSION + 5);
}

#[test]
fn inconsistent_migration_bookkeeping_is_refused() {
    let mut c = Connection::open_in_memory().unwrap();
    configure(&c, false).unwrap();
    c.pragma_update(None, "user_version", 1).unwrap();
    assert!(matches!(
        migrate(&mut c),
        Err(Error::MigrationStateInconsistent { .. })
    ));
}

#[test]
fn schema_over_an_existing_unversioned_database_fails_instead_of_overwriting() {
    let mut c = Connection::open_in_memory().unwrap();
    configure(&c, false).unwrap();
    c.execute_batch("CREATE TABLE regions (id INTEGER PRIMARY KEY, precious TEXT); INSERT INTO regions VALUES (1, 'keep me');")
        .unwrap();
    assert!(matches!(
        migrate(&mut c),
        Err(Error::Migration { version: 1, .. })
    ));
    let kept: String = c
        .query_row("SELECT precious FROM regions", [], |r| r.get(0))
        .unwrap();
    assert_eq!(kept, "keep me");
    assert_eq!(schema_version(&c).unwrap(), 0);
}

#[test]
fn a_failing_migration_rolls_back_completely_and_keeps_earlier_ones() {
    let migrations = [
        Migration {
            version: 1,
            name: "ok",
            sql: "CREATE TABLE schema_migrations (version INTEGER PRIMARY KEY, name TEXT NOT NULL, applied_at TEXT NOT NULL); CREATE TABLE a (x INTEGER);",
        },
        Migration {
            version: 2,
            name: "broken",
            sql: "CREATE TABLE b (x INTEGER); THIS IS NOT SQL;",
        },
    ];
    let mut c = Connection::open_in_memory().unwrap();
    configure(&c, false).unwrap();
    let err = migrate_with(&mut c, &migrations).unwrap_err();
    assert!(
        matches!(
            err,
            Error::Migration {
                version: 2,
                name: "broken",
                ..
            }
        ),
        "{err}"
    );
    assert!(table_exists(&c, "a"), "migration 1 must stay applied");
    assert!(
        !table_exists(&c, "b"),
        "half of migration 2 must not survive"
    );
    assert_eq!(schema_version(&c).unwrap(), 1);
    // Fixing the script lets the next start continue from version 1.
    let fixed = [
        Migration {
            version: 1,
            name: "ok",
            sql: migrations[0].sql,
        },
        Migration {
            version: 2,
            name: "fixed",
            sql: "CREATE TABLE b (x INTEGER);",
        },
    ];
    migrate_with(&mut c, &fixed).unwrap();
    assert!(table_exists(&c, "b"));
    assert_eq!(schema_version(&c).unwrap(), 2);
}

#[test]
fn timestamps_round_trip_and_sort_chronologically() {
    let t = at(0);
    assert_eq!(parse_timestamp(&format_timestamp(t)).unwrap(), t);
    assert!(format_timestamp(at(0)) < format_timestamp(at(1)));
    assert!(format_timestamp(t).ends_with('Z'));
    assert!(parse_timestamp("yesterday").is_err());
}

// ---- foreign keys and relationships -----------------------------------------------------

#[test]
fn foreign_keys_reject_orphans() {
    let db = mem();
    db.with_conn(|c| {
        let err = insert_listing_row(c, Some(999), "olx", "https://olx.example/1").unwrap_err();
        assert!(is_constraint_violation(&err), "{err}");
        assert_rejected(c, "history for a missing listing",
            "INSERT INTO listing_price_history (listing_id, amount_centavos, price_classification, observed_at, provenance_json)
             VALUES (999, 100, 'full_price', '2026-03-01T12:00:00.000Z', '{}')");
        assert_rejected(c, "image for a missing listing",
            "INSERT INTO property_images (listing_id) VALUES (999)");
        assert_rejected(c, "property in a missing region", "INSERT INTO properties (region_id) VALUES (999)");
        Ok(())
    })
    .unwrap();
}

#[test]
fn referenced_rows_cannot_be_deleted() {
    let db = mem();
    db.with_conn(|c| {
        let p = insert_property(c);
        let l = insert_listing(c, Some(p), "olx", "https://olx.example/1");
        assert_rejected(c, "deleting a property that has listings", &format!("DELETE FROM properties WHERE id = {p}"));
        c.execute(
            "INSERT INTO listing_price_history (listing_id, amount_centavos, price_classification, observed_at, provenance_json)
             VALUES (?1, 100, 'full_price', ?2, '{}')",
            params![l, TS],
        )?;
        assert_rejected(c, "deleting a listing that has price history", &format!("DELETE FROM listing_links WHERE id = {l}"));
        assert_eq!(count(c, "listing_links"), 1);
        Ok(())
    })
    .unwrap();
}

#[test]
fn one_property_keeps_every_advertisement_with_its_own_price() {
    let db = mem();
    db.with_conn(|c| {
        let p = insert_property(c);
        for (adapter, url, centavos) in [("olx", "https://olx.example/a", 33_000_000_i64), ("62imoveis", "https://62.example/b", 34_500_000)] {
            let l = insert_listing(c, Some(p), adapter, url);
            c.execute(
                "UPDATE listing_links SET price_centavos = ?1, price_confidence = 'high', price_classification = 'full_price' WHERE id = ?2",
                params![centavos, l],
            )?;
        }
        let rows: Vec<(String, i64)> = c
            .prepare("SELECT adapter_id, price_centavos FROM listing_links WHERE property_id = ?1 ORDER BY id")?
            .query_map([p], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<std::result::Result<_, _>>()?;
        assert_eq!(rows, vec![("olx".to_owned(), 33_000_000), ("62imoveis".to_owned(), 34_500_000)]);
        Ok(())
    })
    .unwrap();
}

#[test]
fn a_listing_can_exist_before_it_is_matched_to_any_property() {
    let db = mem();
    db.with_conn(|c| {
        let l = insert_listing(c, None, "olx", "https://olx.example/1");
        let p: Option<i64> = c.query_row(
            "SELECT property_id FROM listing_links WHERE id = ?1",
            [l],
            |r| r.get(0),
        )?;
        assert_eq!(p, None);
        Ok(())
    })
    .unwrap();
}

#[test]
fn soft_merge_is_reversible_and_never_moves_or_deletes_listings() {
    let db = mem();
    db.with_conn(|c| {
        let a = insert_property(c);
        let b = insert_property(c);
        let la = insert_listing(c, Some(a), "olx", "https://olx.example/1");
        let lb = insert_listing(c, Some(b), "zap", "https://zap.example/1");

        c.execute("UPDATE properties SET merged_into_property_id = ?1, merged_at = ?2 WHERE id = ?3", params![a, TS, b])?;
        c.execute(
            "INSERT INTO property_matches
                (listing_a_id, listing_b_id, evidence_json, confidence_category, decision_status, decision_source,
                 reviewed_at, merge_surviving_property_id, merge_absorbed_property_id)
             VALUES (?1, ?2, '{\"signals\":[\"phash\",\"area\",\"price\"]}', 'high', 'accepted', 'auto', ?3, ?4, ?5)",
            params![la, lb, TS, a, b],
        )?;
        let owner = |id: i64| -> rusqlite::Result<Option<i64>> {
            c.query_row("SELECT property_id FROM listing_links WHERE id = ?1", [id], |r| r.get(0))
        };
        assert_eq!((owner(la)?, owner(lb)?), (Some(a), Some(b)), "listings keep their original property");

        // Undo: clear the soft merge and record the reversal on the decision.
        c.execute("UPDATE properties SET merged_into_property_id = NULL, merged_at = NULL WHERE id = ?1", [b])?;
        c.execute("UPDATE property_matches SET decision_status = 'rejected', decision_source = 'user', reverted_at = ?1, reviewed_at = ?1", [TS])?;

        assert_eq!(count(c, "listing_links"), 2);
        assert_eq!((owner(la)?, owner(lb)?), (Some(a), Some(b)));
        let merged: Option<i64> = c.query_row("SELECT merged_into_property_id FROM properties WHERE id = ?1", [b], |r| r.get(0))?;
        assert_eq!(merged, None);
        let (status, reverted): (String, Option<String>) =
            c.query_row("SELECT decision_status, reverted_at FROM property_matches", [], |r| Ok((r.get(0)?, r.get(1)?)))?;
        assert_eq!((status.as_str(), reverted.as_deref()), ("rejected", Some(TS)));
        Ok(())
    })
    .unwrap();
}

#[test]
fn hiding_survives_rediscovery_of_the_same_advertisement() {
    let db = mem();
    db.with_conn(|c| {
        let p = insert_property(c);
        insert_listing(c, Some(p), "olx", "https://olx.example/1");
        c.execute("UPDATE properties SET user_status = 'hidden', hidden_at = ?1 WHERE id = ?2", params![TS, p])?;

        // A later scrape sees the same URL again: upsert on the natural key.
        c.execute(
            "INSERT INTO listing_links (property_id, source_platform, adapter_id, url, normalized_url, first_seen_at, last_seen_at)
             VALUES (NULL, 'OLX', 'olx', 'https://olx.example/1', 'https://olx.example/1', '2026-04-01T00:00:00.000Z', '2026-04-01T00:00:00.000Z')
             ON CONFLICT (adapter_id, normalized_url) DO UPDATE SET last_seen_at = excluded.last_seen_at",
            [],
        )?;

        assert_eq!(count(c, "listing_links"), 1, "rediscovery must not duplicate the listing");
        let (status, hidden_at, last_seen, property): (String, Option<String>, String, Option<i64>) = c.query_row(
            "SELECT p.user_status, p.hidden_at, l.last_seen_at, l.property_id
               FROM listing_links l JOIN properties p ON p.id = l.property_id",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )?;
        assert_eq!(status, "hidden");
        assert_eq!(hidden_at.as_deref(), Some(TS));
        assert_eq!(last_seen, "2026-04-01T00:00:00.000Z");
        assert_eq!(property, Some(p), "the existing property link must be kept");
        Ok(())
    })
    .unwrap();
}

#[test]
fn updated_at_is_maintained_on_update() {
    let db = mem();
    db.with_conn(|c| {
        let p = insert_property(c);
        c.execute(
            "UPDATE properties SET updated_at = '2020-01-01T00:00:00.000Z' WHERE id = ?1",
            [p],
        )?;
        c.execute("UPDATE properties SET bedrooms = 3 WHERE id = ?1", [p])?;
        let updated: String = c.query_row(
            "SELECT updated_at FROM properties WHERE id = ?1",
            [p],
            |r| r.get(0),
        )?;
        assert!(
            updated.as_str() > "2020-01-01T00:00:00.000Z",
            "updated_at was not refreshed: {updated}"
        );
        Ok(())
    })
    .unwrap();
}

// ---- uniqueness and CHECK constraints ---------------------------------------------------

#[test]
fn listing_url_is_unique_per_adapter_but_not_across_adapters() {
    let db = mem();
    db.with_conn(|c| {
        insert_listing(c, None, "olx", "https://example/1");
        let dup = insert_listing_row(c, None, "olx", "https://example/1").unwrap_err();
        assert!(is_constraint_violation(&dup));
        insert_listing(c, None, "zap", "https://example/1");
        Ok(())
    })
    .unwrap();
}

#[test]
fn region_slug_is_unique_and_regions_form_a_hierarchy() {
    let db = mem();
    db.with_conn(|c| {
        c.execute(
            "INSERT INTO regions (slug, state_code, municipality, name, region_type) VALUES ('go-goiania', 'GO', 'Goiânia', 'Goiânia', 'municipality')",
            [],
        )?;
        let parent = c.last_insert_rowid();
        c.execute(
            "INSERT INTO regions (slug, state_code, municipality, name, region_type, parent_id) VALUES ('go-goiania-bueno', 'GO', 'Goiânia', 'Setor Bueno', 'neighborhood', ?1)",
            [parent],
        )?;
        assert_rejected(c, "duplicate slug",
            "INSERT INTO regions (slug, state_code, municipality, name, region_type) VALUES ('go-goiania', 'GO', 'X', 'X', 'municipality')");
        assert_rejected(c, "deleting a parent region", &format!("DELETE FROM regions WHERE id = {parent}"));
        // A state-level region has no municipality; every other region needs one.
        c.execute("INSERT INTO regions (slug, state_code, name, region_type) VALUES ('go', 'GO', 'Goiás', 'state')", [])?;
        assert_check_rejected(c, "neighbourhood without municipality",
            "INSERT INTO regions (slug, state_code, name, region_type) VALUES ('x', 'GO', 'X', 'neighborhood')");
        Ok(())
    })
    .unwrap();
}

#[test]
fn check_constraints_reject_invalid_values() {
    let db = mem();
    db.with_conn(|c| {
        let p = insert_property(c);
        let p2 = insert_property(c);
        let l = insert_listing(c, Some(p), "olx", "https://example/1");
        let l2 = insert_listing(c, Some(p), "zap", "https://example/2");
        let bad: &[(&str, String)] = &[
            ("negative price", format!("UPDATE listing_links SET price_centavos = -1, price_confidence = 'high' WHERE id = {l}")),
            ("price without confidence", format!("UPDATE listing_links SET price_centavos = 100 WHERE id = {l}")),
            ("classified price without an amount", format!("UPDATE listing_links SET price_classification = 'agio' WHERE id = {l}")),
            ("unknown price classification", format!("UPDATE listing_links SET price_classification = 'discount' WHERE id = {l}")),
            ("non-BRL currency", format!("UPDATE listing_links SET currency = 'USD' WHERE id = {l}")),
            ("unknown listing status", format!("UPDATE listing_links SET listing_status = 'gone' WHERE id = {l}")),
            ("unknown extraction status", format!("UPDATE listing_links SET extraction_status = 'maybe' WHERE id = {l}")),
            ("last seen before first seen", format!("UPDATE listing_links SET last_seen_at = '2000-01-01T00:00:00.000Z' WHERE id = {l}")),
            ("invalid facts json", format!("UPDATE listing_links SET extracted_facts_json = '{{not json' WHERE id = {l}")),
            ("empty url", format!("UPDATE listing_links SET url = '' WHERE id = {l}")),
            ("score above 100", format!("UPDATE properties SET local_score = 100.5 WHERE id = {p}")),
            ("negative score", format!("UPDATE properties SET local_score = -1 WHERE id = {p}")),
            ("negative bedrooms", format!("UPDATE properties SET bedrooms = -1 WHERE id = {p}")),
            ("zero lot area", format!("UPDATE properties SET lot_area_centi_m2 = 0 WHERE id = {p}")),
            ("zero built area", format!("UPDATE properties SET built_area_centi_m2 = 0 WHERE id = {p}")),
            ("unknown property type", format!("UPDATE properties SET property_type = 'castle' WHERE id = {p}")),
            ("unknown user status", format!("UPDATE properties SET user_status = 'deleted' WHERE id = {p}")),
            ("unknown data confidence", format!("UPDATE properties SET data_confidence = 'certain' WHERE id = {p}")),
            ("unknown review status", format!("UPDATE properties SET match_review_status = 'done' WHERE id = {p}")),
            ("invalid canonical provenance json", format!("UPDATE properties SET canonical_provenance_json = 'nope' WHERE id = {p}")),
            ("property merged into itself", format!("UPDATE properties SET merged_into_property_id = {p}, merged_at = '{TS}' WHERE id = {p}")),
            ("unknown region type", "INSERT INTO regions (slug, state_code, municipality, name, region_type) VALUES ('r', 'GO', 'X', 'X', 'galaxy')".to_owned()),
            ("one-letter state code", "INSERT INTO regions (slug, state_code, municipality, name, region_type) VALUES ('r', 'G', 'X', 'X', 'municipality')".to_owned()),
            ("match of a listing with itself", format!("INSERT INTO property_matches (listing_a_id, listing_b_id, evidence_json, confidence_category) VALUES ({l}, {l}, '{{}}', 'low')")),
            ("match with reversed pair order", format!("INSERT INTO property_matches (listing_a_id, listing_b_id, evidence_json, confidence_category) VALUES ({l2}, {l}, '{{}}', 'low')")),
            ("match without evidence", format!("INSERT INTO property_matches (listing_a_id, listing_b_id, evidence_json, confidence_category) VALUES ({l}, {l2}, NULL, 'low')")),
            ("match with unknown confidence", format!("INSERT INTO property_matches (listing_a_id, listing_b_id, evidence_json, confidence_category) VALUES ({l}, {l2}, '{{}}', 'maybe')")),
            ("pHash distance above 64 bits", format!("INSERT INTO property_matches (listing_a_id, listing_b_id, evidence_json, confidence_category, phash_distance) VALUES ({l}, {l2}, '{{}}', 'low', 65)")),
            ("visual score without model", format!("INSERT INTO property_matches (listing_a_id, listing_b_id, evidence_json, confidence_category, visual_similarity) VALUES ({l}, {l2}, '{{}}', 'low', 0.9)")),
            ("decided match without provenance", format!("INSERT INTO property_matches (listing_a_id, listing_b_id, evidence_json, confidence_category, decision_status) VALUES ({l}, {l2}, '{{}}', 'high', 'accepted')")),
            ("automatic merge below high confidence", format!("INSERT INTO property_matches (listing_a_id, listing_b_id, evidence_json, confidence_category, decision_status, decision_source, reviewed_at) VALUES ({l}, {l2}, '{{}}', 'possible', 'accepted', 'auto', '{TS}')")),
            ("merge ids on an undecided match", format!("INSERT INTO property_matches (listing_a_id, listing_b_id, evidence_json, confidence_category, merge_surviving_property_id, merge_absorbed_property_id) VALUES ({l}, {l2}, '{{}}', 'high', {p}, {p2})")),
            ("merge onto the same property", format!("INSERT INTO property_matches (listing_a_id, listing_b_id, evidence_json, confidence_category, decision_status, decision_source, reviewed_at, merge_surviving_property_id, merge_absorbed_property_id) VALUES ({l}, {l2}, '{{}}', 'high', 'accepted', 'user', '{TS}', {p}, {p})")),
            ("merge ids with only one side filled", format!("INSERT INTO property_matches (listing_a_id, listing_b_id, evidence_json, confidence_category, decision_status, decision_source, reviewed_at, merge_surviving_property_id) VALUES ({l}, {l2}, '{{}}', 'high', 'accepted', 'user', '{TS}', {p})")),
            ("negative history amount", format!("INSERT INTO listing_price_history (listing_id, amount_centavos, price_classification, observed_at, provenance_json) VALUES ({l}, -5, 'full_price', '{TS}', '{{}}')")),
            ("history with unknown classification", format!("INSERT INTO listing_price_history (listing_id, amount_centavos, price_classification, observed_at, provenance_json) VALUES ({l}, 5, 'discount', '{TS}', '{{}}')")),
            ("history without provenance", format!("INSERT INTO listing_price_history (listing_id, amount_centavos, price_classification, observed_at, provenance_json) VALUES ({l}, 5, 'full_price', '{TS}', NULL)")),
            ("image pHash without algorithm", format!("INSERT INTO property_images (listing_id, phash) VALUES ({l}, 5)")),
            ("image embedding without model metadata", format!("INSERT INTO property_images (listing_id, embedding) VALUES ({l}, x'00000000')")),
            ("embedding whose size disagrees with its dimension", format!("INSERT INTO property_images (listing_id, embedding, embedding_model, embedding_dim, embedding_dtype) VALUES ({l}, x'0000803f', 'm', 2, 'f32le')")),
            ("absolute image path", format!("INSERT INTO property_images (listing_id, local_path) VALUES ({l}, '/etc/passwd')")),
            ("windows drive image path", format!("INSERT INTO property_images (listing_id, local_path) VALUES ({l}, 'C:\\Users\\x.jpg')")),
            ("unknown image status", format!("INSERT INTO property_images (listing_id, processing_status) VALUES ({l}, 'weird')")),
        ];
        for (description, sql) in bad {
            assert_check_rejected(c, description, sql);
        }
        Ok(())
    })
    .unwrap();
}

#[test]
fn merge_pointer_and_timestamp_must_be_set_together() {
    let db = mem();
    db.with_conn(|c| {
        let a = insert_property(c);
        let b = insert_property(c);
        assert_check_rejected(
            c,
            "pointer without timestamp",
            &format!("UPDATE properties SET merged_into_property_id = {a} WHERE id = {b}"),
        );
        assert_check_rejected(
            c,
            "timestamp without pointer",
            &format!("UPDATE properties SET merged_at = '{TS}' WHERE id = {b}"),
        );
        Ok(())
    })
    .unwrap();
}

#[test]
fn hidden_status_and_timestamp_must_agree() {
    let db = mem();
    db.with_conn(|c| {
        let p = insert_property(c);
        assert_check_rejected(
            c,
            "hidden without timestamp",
            &format!("UPDATE properties SET user_status = 'hidden' WHERE id = {p}"),
        );
        assert_check_rejected(
            c,
            "timestamp without hidden",
            &format!("UPDATE properties SET hidden_at = '{TS}' WHERE id = {p}"),
        );
        c.execute(
            "UPDATE properties SET user_status = 'hidden', hidden_at = ?1 WHERE id = ?2",
            params![TS, p],
        )?;
        // Restoring requires clearing both.
        c.execute(
            "UPDATE properties SET user_status = 'new', hidden_at = NULL WHERE id = ?1",
            [p],
        )?;
        Ok(())
    })
    .unwrap();
}

#[test]
fn match_pair_is_unique_and_reversal_requires_a_recorded_merge() {
    let db = mem();
    db.with_conn(|c| {
        let l1 = insert_listing(c, None, "olx", "https://example/1");
        let l2 = insert_listing(c, None, "zap", "https://example/2");
        let insert = "INSERT INTO property_matches (listing_a_id, listing_b_id, evidence_json, confidence_category, phash_distance)
                      VALUES (?1, ?2, '{\"phash\":3}', 'possible', 3)";
        c.execute(insert, [l1, l2])?;
        let dup = c.execute(insert, [l1, l2]).unwrap_err();
        assert!(is_constraint_violation(&dup));
        assert_check_rejected(c, "reverting a match that never merged", "UPDATE property_matches SET reverted_at = '2026-03-01T12:00:00.000Z'");
        Ok(())
    })
    .unwrap();
}

#[test]
fn image_rows_hold_a_full_64_bit_hash_and_a_versioned_embedding() {
    let db = mem();
    db.with_conn(|c| {
        let l = insert_listing(c, None, "olx", "https://example/1");
        let phash: u64 = 0xF0F0_F0F0_0F0F_0F0F; // top bit set: needs the full signed range
        let stored = i64::from_ne_bytes(phash.to_ne_bytes());
        let embedding: Vec<u8> = [0.5_f32, -1.0, 0.25].iter().flat_map(|f| f.to_le_bytes()).collect();
        c.execute(
            "INSERT INTO property_images (listing_id, source_url, local_path, phash, phash_algorithm, embedding, embedding_model, embedding_dim, embedding_dtype, processing_status)
             VALUES (?1, 'https://img.example/1.jpg', 'images/ab/1.jpg', ?2, 'dct64-v1', ?3, 'mobileclip-s0@test', 3, 'f32le', 'embedded')",
            params![l, stored, embedding],
        )?;
        let (back, dim, blob): (i64, i64, Vec<u8>) = c.query_row(
            "SELECT phash, embedding_dim, embedding FROM property_images",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )?;
        assert_eq!(u64::from_ne_bytes(back.to_ne_bytes()), phash);
        assert_eq!((dim, blob), (3, embedding));
        // Same source URL twice for one listing is a duplicate.
        let dup = c.execute("INSERT INTO property_images (listing_id, source_url) VALUES (?1, 'https://img.example/1.jpg')", [l]).unwrap_err();
        assert!(is_constraint_violation(&dup));
        // Image metadata goes away with its listing (it is derived data).
        c.execute("DELETE FROM listing_links WHERE id = ?1", [l])?;
        assert_eq!(count(c, "property_images"), 0);
        Ok(())
    })
    .unwrap();
}

#[test]
fn rust_enums_and_sql_constraints_accept_exactly_the_same_values() {
    let db = mem();
    db.with_conn(|c| {
        let l = insert_listing(c, None, "olx", "https://example/1");
        for class in PriceClassification::ALL {
            c.execute(
                "UPDATE listing_links SET price_classification = ?1, price_centavos = 100, price_confidence = 'low' WHERE id = ?2",
                params![class.as_str(), l],
            )?;
            c.execute(
                "INSERT INTO listing_price_history (listing_id, amount_centavos, price_classification, observed_at, provenance_json) VALUES (?1, 100, ?2, ?3, '{}')",
                params![l, class.as_str(), TS],
            )?;
        }
        let p = insert_property(c);
        for confidence in Confidence::ALL {
            c.execute("UPDATE listing_links SET price_confidence = ?1 WHERE id = ?2", params![confidence.as_str(), l])?;
            c.execute("UPDATE properties SET data_confidence = ?1 WHERE id = ?2", params![confidence.as_str(), p])?;
        }
        for status in ListingStatus::ALL {
            c.execute("UPDATE listing_links SET listing_status = ?1 WHERE id = ?2", params![status.as_str(), l])?;
        }
        for status in ExtractionStatus::ALL {
            c.execute("UPDATE listing_links SET extraction_status = ?1 WHERE id = ?2", params![status.as_str(), l])?;
        }
        for status in UserStatus::ALL {
            let hidden_at = (*status == UserStatus::Hidden).then_some(TS);
            c.execute("UPDATE properties SET user_status = ?1, hidden_at = ?2 WHERE id = ?3", params![status.as_str(), hidden_at, p])?;
        }
        for kind in PropertyType::ALL {
            c.execute("UPDATE properties SET property_type = ?1 WHERE id = ?2", params![kind.as_str(), p])?;
        }
        // Unknown text is refused by both sides.
        assert!(PriceClassification::from_str("discount").is_err());
        assert_rejected(c, "classification outside the enum", &format!("UPDATE listing_links SET price_classification = 'discount' WHERE id = {l}"));
        Ok(())
    })
    .unwrap();
}

#[test]
fn parsed_facts_round_trip_through_the_database_with_their_evidence() {
    let db = mem();
    let text = "Casa 3/4, 1 suíte, lote 300 m². Passo ágio de R$ 80 mil. Valor total R$ 350 mil.";
    let facts = parse_listing_text(text);
    db.with_conn(|c| {
        let l = insert_listing(c, None, "olx", "https://example/1");
        c.execute(
            "UPDATE listing_links SET raw_description = ?1, extracted_facts_json = ?2, parser_version = ?3 WHERE id = ?4",
            params![text, serde_json::to_string(&facts)?, facts.parser_version, l],
        )?;
        let (raw, json): (String, String) =
            c.query_row("SELECT raw_description, extracted_facts_json FROM listing_links", [], |r| Ok((r.get(0)?, r.get(1)?)))?;
        assert_eq!(raw, text);
        let back: crate::models::listing::ExtractedFacts = serde_json::from_str(&json)?;
        assert_eq!(back, facts);
        // Evidence still points into the stored original text.
        for mention in &back.prices {
            let span = mention.evidence.span;
            assert_eq!(&raw[span.start..span.end], mention.evidence.snippet);
        }
        // Native JSON functions can query it too.
        let agio: i64 = c.query_row(
            "SELECT COUNT(*) FROM listing_links, json_each(extracted_facts_json, '$.prices')
              WHERE json_extract(value, '$.price.classification') = 'agio'",
            [],
            |r| r.get(0),
        )?;
        assert_eq!(agio, 1);
        Ok(())
    })
    .unwrap();
}

// ---- price history --------------------------------------------------------------------

#[test]
fn first_observation_is_recorded() {
    let db = mem();
    db.with_conn(|c| {
        let l = insert_listing(c, None, "olx", "https://example/1");
        let outcome = record_observation(
            c,
            l,
            &observation(350_000, PriceClassification::FullPrice, 0),
        )?;
        assert!(matches!(outcome, RecordOutcome::Recorded { .. }));
        let rows = list_observations(c, l)?;
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].listing_id, l);
        assert_eq!(
            rows[0].observation,
            observation(350_000, PriceClassification::FullPrice, 0)
        );
        Ok(())
    })
    .unwrap();
}

#[test]
fn re_observing_an_unchanged_price_adds_nothing() {
    let db = mem();
    db.with_conn(|c| {
        let l = insert_listing(c, None, "olx", "https://example/1");
        let RecordOutcome::Recorded { id } = record_observation(
            c,
            l,
            &observation(350_000, PriceClassification::FullPrice, 0),
        )?
        else {
            panic!("first observation must be recorded");
        };
        for later in [60, 3_600, 86_400] {
            // Even a different confidence or timestamp is not a price change.
            let mut again = observation(350_000, PriceClassification::FullPrice, later);
            again.confidence = Some(Confidence::Low);
            assert_eq!(
                record_observation(c, l, &again)?,
                RecordOutcome::Unchanged { latest_id: id }
            );
        }
        assert_eq!(list_observations(c, l)?.len(), 1);
        Ok(())
    })
    .unwrap();
}

#[test]
fn price_changes_and_classification_changes_are_both_kept() {
    let db = mem();
    db.with_conn(|c| {
        let l = insert_listing(c, None, "olx", "https://example/1");
        let steps = [
            (350_000, PriceClassification::FullPrice, 0, true),
            (340_000, PriceClassification::FullPrice, 10, true), // price drop
            (340_000, PriceClassification::Agio, 20, true),      // same number, different meaning
            (340_000, PriceClassification::Agio, 30, false),     // repeat
            (350_000, PriceClassification::FullPrice, 40, true), // back to an earlier value: still a change
        ];
        for (reais, class, secs, expect_new) in steps {
            let outcome = record_observation(c, l, &observation(reais, class, secs))?;
            assert_eq!(
                matches!(outcome, RecordOutcome::Recorded { .. }),
                expect_new,
                "{reais} {class} @{secs}"
            );
        }
        let rows = list_observations(c, l)?;
        let seen: Vec<(i64, PriceClassification)> = rows
            .iter()
            .map(|r| {
                (
                    r.observation.amount.centavos() / 100,
                    r.observation.classification,
                )
            })
            .collect();
        assert_eq!(
            seen,
            vec![
                (350_000, PriceClassification::FullPrice),
                (340_000, PriceClassification::FullPrice),
                (340_000, PriceClassification::Agio),
                (350_000, PriceClassification::FullPrice),
            ]
        );
        Ok(())
    })
    .unwrap();
}

#[test]
fn histories_of_different_listings_are_independent() {
    let db = mem();
    db.with_conn(|c| {
        let l1 = insert_listing(c, None, "olx", "https://example/1");
        let l2 = insert_listing(c, None, "zap", "https://example/2");
        record_observation(
            c,
            l1,
            &observation(350_000, PriceClassification::FullPrice, 0),
        )?;
        let outcome = record_observation(
            c,
            l2,
            &observation(350_000, PriceClassification::FullPrice, 0),
        )?;
        assert!(
            matches!(outcome, RecordOutcome::Recorded { .. }),
            "same price on another listing is new history"
        );
        assert_eq!(
            (
                list_observations(c, l1)?.len(),
                list_observations(c, l2)?.len()
            ),
            (1, 1)
        );
        Ok(())
    })
    .unwrap();
}

#[test]
fn an_observation_older_than_the_latest_is_rejected_and_not_stored() {
    let db = mem();
    db.with_conn(|c| {
        let l = insert_listing(c, None, "olx", "https://example/1");
        record_observation(
            c,
            l,
            &observation(350_000, PriceClassification::FullPrice, 100),
        )?;
        let err = record_observation(
            c,
            l,
            &observation(300_000, PriceClassification::FullPrice, 50),
        )
        .unwrap_err();
        assert!(matches!(err, Error::OutOfOrderObservation { .. }), "{err}");
        assert_eq!(list_observations(c, l)?.len(), 1);
        Ok(())
    })
    .unwrap();
}

#[test]
fn observation_for_a_missing_listing_fails_without_writing() {
    let db = mem();
    db.with_conn(|c| {
        let err = record_observation(c, 999, &observation(1, PriceClassification::FullPrice, 0))
            .unwrap_err();
        assert!(matches!(err, Error::Database(_)), "{err}");
        assert_eq!(count(c, "listing_price_history"), 0);
        Ok(())
    })
    .unwrap();
}

#[test]
fn history_is_append_only_at_the_database_level() {
    let db = mem();
    db.with_conn(|c| {
        let l = insert_listing(c, None, "olx", "https://example/1");
        record_observation(
            c,
            l,
            &observation(350_000, PriceClassification::FullPrice, 0),
        )?;
        let upd = c
            .execute("UPDATE listing_price_history SET amount_centavos = 1", [])
            .unwrap_err();
        assert!(upd.to_string().contains("append-only"), "{upd}");
        let del = c
            .execute("DELETE FROM listing_price_history", [])
            .unwrap_err();
        assert!(del.to_string().contains("append-only"), "{del}");
        assert_eq!(
            list_observations(c, l)?[0].observation.amount.centavos(),
            35_000_000
        );
        Ok(())
    })
    .unwrap();
}

#[test]
fn provenance_and_missing_confidence_round_trip() {
    let db = mem();
    db.with_conn(|c| {
        let l = insert_listing(c, None, "olx", "https://example/1");
        let facts = parse_listing_text("ágio de R$ 80 mil");
        let mut obs = observation(80_000, PriceClassification::Agio, 0);
        obs.confidence = None;
        obs.provenance = PriceProvenance {
            extractor: facts.parser_version.clone(),
            source_field: None,
            evidence: Some(facts.prices[0].evidence.clone()),
        };
        record_observation(c, l, &obs)?;
        assert_eq!(list_observations(c, l)?[0].observation, obs);
        Ok(())
    })
    .unwrap();
}

#[test]
fn key_queries_use_indexes() {
    let db = mem();
    db.with_conn(|c| {
        for (query, expected_index) in [
            ("SELECT * FROM listing_price_history WHERE listing_id = 1 ORDER BY observed_at DESC, id DESC LIMIT 1", "idx_price_history_listing"),
            ("SELECT * FROM listing_links WHERE property_id = 1", "idx_listing_links_property"),
            ("SELECT * FROM property_images WHERE listing_id = 1", "idx_property_images_listing"),
            ("SELECT * FROM properties WHERE user_status = 'hidden'", "idx_properties_user_status"),
            ("SELECT * FROM listing_links WHERE adapter_id = 'olx' AND normalized_url = 'x'", "sqlite_autoindex_listing_links"),
        ] {
            let plan: Vec<String> = c
                .prepare(&format!("EXPLAIN QUERY PLAN {query}"))?
                .query_map([], |r| r.get::<_, String>(3))?
                .collect::<std::result::Result<_, _>>()?;
            let plan = plan.join(" | ");
            assert!(plan.contains(expected_index), "{query}\n  plan: {plan}");
        }
        Ok(())
    })
    .unwrap();
}
