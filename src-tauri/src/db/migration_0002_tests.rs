//! Tests for migration 0002 (collection runs, source outcomes, merge audit, merge guards).

use rusqlite::{Connection, ErrorCode, params};

use super::*;

const TS: &str = "2026-03-01T12:00:00.000Z";

fn v1_database() -> Connection {
    let mut conn = Connection::open_in_memory().unwrap();
    configure(&conn, false).unwrap();
    migrate_with(&mut conn, &MIGRATIONS[..1]).unwrap();
    assert_eq!(schema_version(&conn).unwrap(), 1);
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

fn check_failure(c: &Connection, description: &str, sql: &str) {
    match c.execute(sql, []) {
        Ok(_) => panic!("accepted but must be rejected: {description}\n{sql}"),
        Err(e) => {
            let message = e.to_string();
            let violation = matches!(&e, rusqlite::Error::SqliteFailure(f, _) if f.code == ErrorCode::ConstraintViolation);
            assert!(
                violation
                    && (message.contains("CHECK constraint failed")
                        || message.contains("NOT NULL constraint failed")),
                "{description}: expected a CHECK/NOT NULL failure, got: {message}"
            );
        }
    }
}

fn new_property(c: &Connection) -> i64 {
    c.execute("INSERT INTO properties DEFAULT VALUES", [])
        .unwrap();
    c.last_insert_rowid()
}

fn new_run(c: &Connection) -> i64 {
    c.execute(
        "INSERT INTO collection_runs (started_at, request_json) VALUES (?1, '{}')",
        [TS],
    )
    .unwrap();
    c.last_insert_rowid()
}

#[test]
fn upgrading_from_v1_keeps_every_existing_row_and_adds_the_new_structures() {
    let mut c = v1_database();
    c.execute(
        "INSERT INTO regions (slug, state_code, municipality, name, region_type)
         VALUES ('br-go-goiania', 'GO', 'Goiânia', 'Goiânia', 'municipality')",
        [],
    )
    .unwrap();
    let p = new_property(&c);
    c.execute(
        "INSERT INTO listing_links (property_id, source_platform, adapter_id, url, normalized_url, first_seen_at, last_seen_at)
         VALUES (?1, 'X', 'x', 'https://x.test/1', 'https://x.test/1', ?2, ?2)",
        params![p, TS],
    )
    .unwrap();
    let listing = c.last_insert_rowid();
    c.execute(
        "INSERT INTO listing_price_history (listing_id, amount_centavos, price_classification, observed_at, provenance_json)
         VALUES (?1, 100, 'full_price', ?2, '{}')",
        params![listing, TS],
    )
    .unwrap();
    assert!(!table_exists(&c, "collection_runs"));

    migrate(&mut c).unwrap();

    assert_eq!(schema_version(&c).unwrap(), LATEST_SCHEMA_VERSION);
    const { assert!(LATEST_SCHEMA_VERSION >= 2) };
    for table in [
        "regions",
        "properties",
        "listing_links",
        "listing_price_history",
    ] {
        assert_eq!(count(&c, table), 1, "{table} lost rows in the upgrade");
    }
    for table in [
        "collection_runs",
        "source_run_outcomes",
        "property_merge_events",
    ] {
        assert!(table_exists(&c, table), "missing {table}");
    }
    assert!(column_exists(&c, "listing_links", "source_fields_json"));
    assert!(column_exists(&c, "listing_links", "status_changed_at"));
    assert!(column_exists(&c, "listing_links", "status_reason"));
    assert_eq!(
        count(&c, "schema_migrations"),
        i64::try_from(MIGRATIONS.len()).unwrap()
    );
    let names: Vec<String> = c
        .prepare("SELECT name FROM schema_migrations ORDER BY version")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .map(|name| name.unwrap())
        .collect();
    assert_eq!(names[0], "initial_schema");
    assert_eq!(names[1], "collection_and_audit");
}

#[test]
fn raw_source_fields_must_be_valid_json_when_present() {
    let c = latest_database();
    c.execute(
        "INSERT INTO listing_links (source_platform, adapter_id, url, normalized_url, first_seen_at, last_seen_at, source_fields_json)
         VALUES ('X', 'x', 'u', 'u', ?1, ?1, '{\"price\":\"R$ 350.000\"}')",
        [TS],
    )
    .unwrap();
    check_failure(
        &c,
        "malformed source fields",
        &format!(
            "INSERT INTO listing_links (source_platform, adapter_id, url, normalized_url, first_seen_at, last_seen_at, source_fields_json)
             VALUES ('X', 'x', 'v', 'v', '{TS}', '{TS}', '{{oops')"
        ),
    );
}

#[test]
fn collection_runs_validate_status_and_completion() {
    let c = latest_database();
    let ok = "INSERT INTO collection_runs (started_at, request_json) VALUES ('2026-03-01T12:00:00.000Z', '{\"regions\":[\"go-goiania\"]}')";
    c.execute(ok, []).unwrap();
    for (description, sql) in [
        (
            "unknown status",
            "INSERT INTO collection_runs (started_at, request_json, status) VALUES ('2026-03-01T12:00:00.000Z', '{}', 'paused')",
        ),
        (
            "request that is not json",
            "INSERT INTO collection_runs (started_at, request_json) VALUES ('2026-03-01T12:00:00.000Z', 'nope')",
        ),
        (
            "finished run without a finish time",
            "INSERT INTO collection_runs (started_at, request_json, status) VALUES ('2026-03-01T12:00:00.000Z', '{}', 'completed')",
        ),
        (
            "running run with a finish time",
            "INSERT INTO collection_runs (started_at, request_json, finished_at) VALUES ('2026-03-01T12:00:00.000Z', '{}', '2026-03-01T12:01:00.000Z')",
        ),
        (
            "finished before it started",
            "INSERT INTO collection_runs (started_at, request_json, status, finished_at) VALUES ('2026-03-01T12:00:00.000Z', '{}', 'completed', '2026-03-01T11:00:00.000Z')",
        ),
    ] {
        check_failure(&c, description, sql);
    }
    for status in ["completed", "cancelled", "failed"] {
        c.execute(
            "INSERT INTO collection_runs (started_at, request_json, status, finished_at)
             VALUES ('2026-03-01T12:00:00.000Z', '{}', ?1, '2026-03-01T12:01:00.000Z')",
            [status],
        )
        .unwrap();
    }
}

#[test]
fn source_outcomes_distinguish_the_four_states_and_never_hide_failures() {
    let c = latest_database();
    let run = new_run(&c);
    let insert = |adapter: &str,
                  outcome: &str,
                  complete: i64,
                  discovered: i64,
                  error: Option<&str>| {
        c.execute(
            "INSERT INTO source_run_outcomes
                (run_id, adapter_id, outcome, complete, discovered_count, error_kind, started_at, finished_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)",
            params![run, adapter, outcome, complete, discovered, error, TS],
        )
    };
    insert("a", "results", 1, 12, None).unwrap();
    insert("b", "successful_empty", 1, 0, None).unwrap();
    insert("c", "failed", 0, 0, Some("timeout")).unwrap();
    insert("d", "unreliable", 0, 4, None).unwrap();
    // A partial result is allowed, but is not marked complete.
    insert("e", "results", 0, 7, None).unwrap();
    assert_eq!(count(&c, "source_run_outcomes"), 5);

    for (description, result) in [
        (
            "empty result from an incomplete search",
            insert("f", "successful_empty", 0, 0, None),
        ),
        (
            "empty result that discovered something",
            insert("g", "successful_empty", 1, 3, None),
        ),
        (
            "failure without an error kind",
            insert("h", "failed", 0, 0, None),
        ),
        ("unknown outcome", insert("i", "maybe", 1, 0, None)),
        ("negative count", insert("j", "results", 1, -1, None)),
    ] {
        let err = result.expect_err(description);
        assert!(
            err.to_string().contains("constraint failed"),
            "{description}: {err}"
        );
    }
    // One outcome per source per run.
    let dup = insert("a", "results", 1, 1, None).unwrap_err();
    assert!(dup.to_string().contains("UNIQUE"), "{dup}");
}

#[test]
fn runs_with_outcomes_cannot_be_deleted() {
    let c = latest_database();
    let run = new_run(&c);
    c.execute(
        "INSERT INTO source_run_outcomes (run_id, adapter_id, outcome, complete, started_at, finished_at)
         VALUES (?1, 'a', 'successful_empty', 1, ?2, ?2)",
        params![run, TS],
    )
    .unwrap();
    let err = c.execute("DELETE FROM collection_runs", []).unwrap_err();
    assert!(err.to_string().contains("FOREIGN KEY"), "{err}");
}

#[test]
fn merge_audit_is_append_only_and_validated() {
    let c = latest_database();
    let (a, b) = (new_property(&c), new_property(&c));
    c.execute(
        "INSERT INTO listing_links (property_id, source_platform, adapter_id, url, normalized_url, first_seen_at, last_seen_at)
         VALUES (?1, 'X', 'x', 'u1', 'u1', ?3, ?3), (?2, 'Y', 'y', 'u2', 'u2', ?3, ?3)",
        params![a, b, TS],
    )
    .unwrap();
    c.execute(
        "INSERT INTO property_matches (listing_a_id, listing_b_id, evidence_json, confidence_category)
         VALUES (1, 2, '{}', 'possible')",
        [],
    )
    .unwrap();
    let insert = |event: &str, snapshot: &str| {
        c.execute(
            "INSERT INTO property_merge_events
                (match_id, event, surviving_property_id, absorbed_property_id, actor, occurred_at, snapshot_json)
             VALUES (1, ?1, ?2, ?3, 'user', ?4, ?5)",
            params![event, a, b, TS, snapshot],
        )
    };
    insert("merged", "{\"statuses\":{}}").unwrap();
    insert("reverted", "{}").unwrap();
    assert!(insert("deleted", "{}").is_err(), "unknown event");
    assert!(insert("merged", "not json").is_err(), "invalid snapshot");

    let upd = c
        .execute("UPDATE property_merge_events SET actor = 'auto'", [])
        .unwrap_err();
    assert!(upd.to_string().contains("append-only"), "{upd}");
    let del = c
        .execute("DELETE FROM property_merge_events", [])
        .unwrap_err();
    assert!(del.to_string().contains("append-only"), "{del}");
    assert_eq!(count(&c, "property_merge_events"), 2);
}

#[test]
fn a_property_can_only_be_merged_into_a_root_property() {
    let c = latest_database();
    let (a, b, d) = (new_property(&c), new_property(&c), new_property(&c));
    let merge = |absorbed: i64, into: i64| {
        c.execute(
            "UPDATE properties SET merged_into_property_id = ?1, merged_at = ?2 WHERE id = ?3",
            params![into, TS, absorbed],
        )
    };
    merge(b, a).unwrap();
    // `b` is no longer a root, so nothing may point at it.
    let err = merge(d, b).unwrap_err();
    assert!(
        err.to_string().contains("merge target is itself merged"),
        "{err}"
    );
    // The same rule blocks every cycle: a (root) cannot be folded into its own absorbed child.
    let err = merge(a, b).unwrap_err();
    assert!(
        err.to_string().contains("merge target is itself merged"),
        "{err}"
    );
    // Self-merge is still refused by the original CHECK.
    check_failure(
        &c,
        "self merge",
        &format!(
            "UPDATE properties SET merged_into_property_id = {d}, merged_at = '{TS}' WHERE id = {d}"
        ),
    );
    // Inserting a row that already points at a merged property is refused too.
    let ins = c
        .execute(
            "INSERT INTO properties (merged_into_property_id, merged_at) VALUES (?1, ?2)",
            params![b, TS],
        )
        .unwrap_err();
    assert!(
        ins.to_string().contains("merge target is itself merged"),
        "{ins}"
    );
}

#[test]
fn chains_are_allowed_when_a_root_with_children_is_absorbed() {
    let c = latest_database();
    let (a, b, root) = (new_property(&c), new_property(&c), new_property(&c));
    let merge = |absorbed: i64, into: i64| {
        c.execute(
            "UPDATE properties SET merged_into_property_id = ?1, merged_at = ?2 WHERE id = ?3",
            params![into, TS, absorbed],
        )
    };
    merge(b, a).unwrap(); // b -> a
    merge(a, root).unwrap(); // a -> root, so b -> a -> root
    let depth: i64 = c
        .query_row(
            "WITH RECURSIVE chain(id, depth) AS (
                 SELECT ?1, 0
                 UNION ALL
                 SELECT p.merged_into_property_id, chain.depth + 1
                   FROM properties p JOIN chain ON p.id = chain.id
                  WHERE p.merged_into_property_id IS NOT NULL
             )
             SELECT MAX(depth) FROM chain",
            [b],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(depth, 2);
    // Undoing the outer merge restores `a` as a root without touching `b -> a`.
    c.execute(
        "UPDATE properties SET merged_into_property_id = NULL, merged_at = NULL WHERE id = ?1",
        [a],
    )
    .unwrap();
    let pointer: Option<i64> = c
        .query_row(
            "SELECT merged_into_property_id FROM properties WHERE id = ?1",
            [b],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(pointer, Some(a));
}

#[test]
fn property_groups_follow_merge_chains_and_effective_status_never_loses_a_choice() {
    let c = latest_database();
    let (a, b, d, other) = (
        new_property(&c),
        new_property(&c),
        new_property(&c),
        new_property(&c),
    );
    let merge = |absorbed: i64, into: i64| {
        c.execute(
            "UPDATE properties SET merged_into_property_id = ?1, merged_at = ?2 WHERE id = ?3",
            params![into, TS, absorbed],
        )
        .unwrap();
    };
    let group = |root: i64| -> Vec<i64> {
        c.prepare("SELECT member_id FROM property_groups WHERE root_id = ?1 ORDER BY member_id")
            .unwrap()
            .query_map([root], |r| r.get(0))
            .unwrap()
            .map(|id| id.unwrap())
            .collect()
    };
    assert_eq!(group(a), vec![a], "an unmerged property is a group of one");
    merge(b, a);
    merge(a, d); // b -> a -> d
    assert_eq!(group(d), vec![a, b, d]);
    assert!(group(a).is_empty(), "an absorbed property is not a root");
    assert_eq!(group(other), vec![other]);

    let status = |root: i64| -> (String, i64) {
        c.query_row(
            "SELECT effective_status, status_conflict FROM property_group_status WHERE root_id = ?1",
            [root],
            |r| Ok((r.get(0).unwrap(), r.get(1).unwrap())),
        )
        .unwrap()
    };
    assert_eq!(status(d), ("new".to_owned(), 0));
    c.execute(
        "UPDATE properties SET user_status = 'hidden', hidden_at = ?1 WHERE id = ?2",
        params![TS, b],
    )
    .unwrap();
    assert_eq!(status(d), ("hidden".to_owned(), 0), "hidden beats new");
    c.execute(
        "UPDATE properties SET user_status = 'favorite' WHERE id = ?1",
        [a],
    )
    .unwrap();
    assert_eq!(
        status(d),
        ("favorite".to_owned(), 1),
        "favorite beats hidden, and the clash is flagged"
    );
    c.execute(
        "UPDATE properties SET user_status = 'new', hidden_at = NULL WHERE id = ?1",
        [b],
    )
    .unwrap();
    assert_eq!(status(d), ("favorite".to_owned(), 0));
}
