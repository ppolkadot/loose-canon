-- Migration 2: collection runs and per-source outcomes, merge audit log, merge guards.
--
-- Additive only: nothing from schema v1 is dropped or rewritten, so an existing database keeps all
-- of its rows. Conventions are those of schema.sql (STRICT tables, UTC millisecond timestamps,
-- enumerations as TEXT + CHECK, history-like references RESTRICT).

-- Raw structured fields exactly as the source adapter returned them (e.g. {"price":"R$ 350.000"}).
-- Evidence spans name the field they index into; this column keeps that text so a span can always
-- be resolved later, even for fields (like the price field) that are not stored elsewhere.
ALTER TABLE listing_links ADD COLUMN source_fields_json TEXT
    CHECK (source_fields_json IS NULL OR json_valid(source_fields_json));

-- When the listing last changed status (active/unavailable) and why, e.g. "HTTP 410". Evidence for
-- "unavailable" must be explicit; absence from a search result is never enough.
ALTER TABLE listing_links ADD COLUMN status_changed_at TEXT;
ALTER TABLE listing_links ADD COLUMN status_reason TEXT;

-- ---------------------------------------------------------------------------------------------
-- collection_runs: one row per search/collection request.
-- ---------------------------------------------------------------------------------------------
CREATE TABLE collection_runs (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    started_at   TEXT NOT NULL,
    finished_at  TEXT,
    status       TEXT NOT NULL DEFAULT 'running' CHECK (status IN ('running', 'completed', 'cancelled', 'failed')),
    -- What was asked for (regions, price range, filter flags...). Free-form JSON.
    request_json TEXT NOT NULL CHECK (json_valid(request_json)),
    note         TEXT,
    CHECK ((status = 'running') = (finished_at IS NULL)),
    CHECK (finished_at IS NULL OR finished_at >= started_at)
) STRICT;

CREATE INDEX idx_collection_runs_started ON collection_runs (started_at DESC, id DESC);

-- ---------------------------------------------------------------------------------------------
-- source_run_outcomes: what happened for each source in a run.
--
-- The four outcomes are deliberately distinct states, so a source failure is never reported as
-- "no properties exist":
--   results           the source returned listings
--   successful_empty  the source answered completely and had nothing for the request
--   failed            the source could not be reached or refused / errored
--   unreliable        the source answered but its content could not be parsed reliably
-- `complete` is 0 for a partial search (cancelled, rate limited, truncated...); a partial result
-- keeps its warnings and must not be presented as a complete search.
-- ---------------------------------------------------------------------------------------------
CREATE TABLE source_run_outcomes (
    id                INTEGER PRIMARY KEY AUTOINCREMENT,
    run_id            INTEGER NOT NULL REFERENCES collection_runs (id) ON DELETE RESTRICT,
    adapter_id        TEXT NOT NULL CHECK (length(adapter_id) > 0),
    outcome           TEXT NOT NULL CHECK (outcome IN ('results', 'successful_empty', 'failed', 'unreliable')),
    complete          INTEGER NOT NULL CHECK (complete IN (0, 1)),
    discovered_count  INTEGER NOT NULL DEFAULT 0 CHECK (discovered_count >= 0),
    stored_count      INTEGER NOT NULL DEFAULT 0 CHECK (stored_count >= 0),
    unparseable_count INTEGER NOT NULL DEFAULT 0 CHECK (unparseable_count >= 0),
    failed_count      INTEGER NOT NULL DEFAULT 0 CHECK (failed_count >= 0),
    skipped_count     INTEGER NOT NULL DEFAULT 0 CHECK (skipped_count >= 0),
    warnings_json     TEXT NOT NULL DEFAULT '[]' CHECK (json_valid(warnings_json)),
    error_kind        TEXT,
    error_detail      TEXT,
    started_at        TEXT NOT NULL,
    finished_at       TEXT NOT NULL,
    UNIQUE (run_id, adapter_id),
    -- "Nothing found" is only meaningful for a search that completed and found nothing.
    CHECK (outcome <> 'successful_empty' OR (complete = 1 AND discovered_count = 0)),
    -- A failure must say what failed.
    CHECK (outcome <> 'failed' OR error_kind IS NOT NULL),
    CHECK (finished_at >= started_at)
) STRICT;

CREATE INDEX idx_source_outcomes_adapter ON source_run_outcomes (adapter_id, id DESC);

-- ---------------------------------------------------------------------------------------------
-- property_merge_events: append-only audit of every merge and every undo.
--
-- property_matches holds the *current* decision; this log keeps the history, so repeated
-- merge/undo cycles stay reconstructable. snapshot_json records the user statuses and pointers
-- at that moment, so nothing a user chose can be lost.
-- ---------------------------------------------------------------------------------------------
CREATE TABLE property_merge_events (
    id                    INTEGER PRIMARY KEY AUTOINCREMENT,
    match_id              INTEGER NOT NULL REFERENCES property_matches (id) ON DELETE RESTRICT,
    event                 TEXT NOT NULL CHECK (event IN ('merged', 'reverted')),
    surviving_property_id INTEGER NOT NULL REFERENCES properties (id) ON DELETE RESTRICT,
    absorbed_property_id  INTEGER NOT NULL REFERENCES properties (id) ON DELETE RESTRICT,
    actor                 TEXT NOT NULL CHECK (actor IN ('auto', 'user')),
    rule_version          TEXT,
    note                  TEXT,
    occurred_at           TEXT NOT NULL,
    snapshot_json         TEXT NOT NULL CHECK (json_valid(snapshot_json)),
    CHECK (surviving_property_id <> absorbed_property_id)
) STRICT;

CREATE INDEX idx_merge_events_match ON property_merge_events (match_id, id);

CREATE TRIGGER property_merge_events_no_update BEFORE UPDATE ON property_merge_events
BEGIN
    SELECT RAISE(ABORT, 'property_merge_events is append-only');
END;

CREATE TRIGGER property_merge_events_no_delete BEFORE DELETE ON property_merge_events
BEGIN
    SELECT RAISE(ABORT, 'property_merge_events is append-only');
END;

-- ---------------------------------------------------------------------------------------------
-- Merge guards. A property may only be folded into a ROOT property (one that is not itself merged).
-- That single rule makes cycles impossible (a root has no pointer to follow) while still allowing
-- chains: a root that already absorbed others can later be absorbed by another root.
-- ---------------------------------------------------------------------------------------------
CREATE TRIGGER properties_merge_target_must_be_root_on_update
BEFORE UPDATE OF merged_into_property_id ON properties
WHEN NEW.merged_into_property_id IS NOT NULL
 AND EXISTS (SELECT 1 FROM properties t
              WHERE t.id = NEW.merged_into_property_id AND t.merged_into_property_id IS NOT NULL)
BEGIN
    SELECT RAISE(ABORT, 'merge target is itself merged');
END;

CREATE TRIGGER properties_merge_target_must_be_root_on_insert
BEFORE INSERT ON properties
WHEN NEW.merged_into_property_id IS NOT NULL
 AND EXISTS (SELECT 1 FROM properties t
              WHERE t.id = NEW.merged_into_property_id AND t.merged_into_property_id IS NOT NULL)
BEGIN
    SELECT RAISE(ABORT, 'merge target is itself merged');
END;

-- ---------------------------------------------------------------------------------------------
-- Merge groups. A group is a root property plus everything folded into it, directly or through
-- a chain. Listings keep pointing at their original property, so a listing's group is the group
-- of its property's root.
-- ---------------------------------------------------------------------------------------------
CREATE VIEW property_groups AS
WITH RECURSIVE tree (root_id, member_id) AS (
    SELECT id, id FROM properties WHERE merged_into_property_id IS NULL
    UNION ALL
    SELECT tree.root_id, p.id
      FROM properties p
      JOIN tree ON p.merged_into_property_id = tree.member_id
)
SELECT root_id, member_id FROM tree;

-- Effective user status of a group: favorite beats hidden beats new, so merging never silently
-- loses a user's choice. `status_conflict` is 1 when members disagree in the favorite/hidden sense
-- (surface it to the user). Each member keeps its own stored status, so undoing a merge restores
-- exactly what each side had.
CREATE VIEW property_group_status AS
SELECT g.root_id AS root_id,
       CASE WHEN SUM(p.user_status = 'favorite') > 0 THEN 'favorite'
            WHEN SUM(p.user_status = 'hidden') > 0 THEN 'hidden'
            ELSE 'new' END AS effective_status,
       CASE WHEN SUM(p.user_status = 'favorite') > 0 AND SUM(p.user_status = 'hidden') > 0
            THEN 1 ELSE 0 END AS status_conflict
  FROM property_groups g
  JOIN properties p ON p.id = g.member_id
 GROUP BY g.root_id;
