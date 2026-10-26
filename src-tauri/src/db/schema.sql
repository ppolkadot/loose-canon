-- Loose Canon database schema, version 1 (see db/mod.rs for the migration strategy).
--
-- Conventions
--   * STRICT tables: SQLite rejects values whose type does not match the column.
--   * Money is integer centavos (BRL). Areas are integer hundredths of a square metre.
--     No floating point is used for either. Scores and similarities are REAL because they are
--     not money and are never compared for equality.
--   * Timestamps are UTC RFC 3339 text with milliseconds ('2026-01-31T12:00:00.000Z'), which
--     sorts chronologically as plain text. Defaults use strftime to produce the same shape.
--   * Unknown is NULL, never 0 or ''.
--   * Enumerations are TEXT with CHECK constraints. The Rust enums in src/models expose an
--     ALL constant and a test proves both sides accept exactly the same values.
--   * Foreign keys are ON (db::configure verifies it). History-like references are RESTRICT so
--     nothing can be deleted by accident; derived image metadata is CASCADE.
--   * Nothing here ever deletes data when an advertisement disappears: listings go
--     'unavailable' and keep their history.
--
-- This script is applied exactly once, inside a transaction, when PRAGMA user_version is 0.
-- It deliberately uses plain CREATE (no IF NOT EXISTS): running it over existing tables must
-- fail rather than blend with or replace someone's data.

-- Audit trail of applied migrations; must agree with PRAGMA user_version.
CREATE TABLE schema_migrations (
    version    INTEGER PRIMARY KEY CHECK (version > 0),
    name       TEXT NOT NULL CHECK (length(name) > 0),
    applied_at TEXT NOT NULL
) STRICT;

-- ---------------------------------------------------------------------------------------------
-- regions: configurable geographic coverage (loaded from a seed file in a later phase).
-- Adding a city or neighbourhood is an INSERT, not a code change.
-- ---------------------------------------------------------------------------------------------
CREATE TABLE regions (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    -- Stable key from the seed configuration, so re-seeding updates instead of duplicating.
    slug          TEXT NOT NULL UNIQUE CHECK (length(slug) > 0),
    country_code  TEXT NOT NULL DEFAULT 'BR' CHECK (length(country_code) = 2),
    state_code    TEXT NOT NULL CHECK (length(state_code) = 2),
    -- NULL only for state-level regions.
    municipality  TEXT,
    name          TEXT NOT NULL CHECK (length(name) > 0),
    region_type   TEXT NOT NULL CHECK (region_type IN
                    ('state', 'municipality', 'neighborhood', 'district', 'condominium', 'corridor', 'other')),
    parent_id     INTEGER REFERENCES regions (id) ON DELETE RESTRICT,
    enabled       INTEGER NOT NULL DEFAULT 1 CHECK (enabled IN (0, 1)),
    -- Optional extras (aliases, centroid, notes...). Free-form JSON.
    metadata_json TEXT CHECK (metadata_json IS NULL OR json_valid(metadata_json)),
    created_at    TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    updated_at    TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    CHECK (region_type = 'state' OR municipality IS NOT NULL),
    CHECK (parent_id IS NULL OR parent_id <> id)
) STRICT;

CREATE INDEX idx_regions_parent ON regions (parent_id);
CREATE INDEX idx_regions_lookup ON regions (state_code, municipality, region_type) WHERE enabled = 1;

-- ---------------------------------------------------------------------------------------------
-- properties: a real-world property, or a not-yet-resolved candidate for one.
--
-- Deliberately has NO price column: a property's comparable price is derived from its eligible
-- listings (classification, confidence, status), never frozen as "the lowest price seen".
-- Canonical characteristics are a best current view, not truth; canonical_provenance_json says
-- which listing/evidence supports each value and records disagreements between listings.
-- ---------------------------------------------------------------------------------------------
CREATE TABLE properties (
    id                        INTEGER PRIMARY KEY AUTOINCREMENT,
    region_id                 INTEGER REFERENCES regions (id) ON DELETE RESTRICT,
    municipality              TEXT,
    neighborhood              TEXT,
    property_type             TEXT CHECK (property_type IS NULL OR property_type IN
                                ('house', 'semi_detached', 'sobrado', 'apartment', 'lot', 'chacara', 'other')),
    bedrooms                  INTEGER CHECK (bedrooms IS NULL OR bedrooms BETWEEN 0 AND 100),
    suites                    INTEGER CHECK (suites IS NULL OR suites BETWEEN 0 AND 100),
    parking_spaces            INTEGER CHECK (parking_spaces IS NULL OR parking_spaces BETWEEN 0 AND 100),
    -- Lot, built and usable area are separate concepts and never substitute for one another.
    lot_area_centi_m2         INTEGER CHECK (lot_area_centi_m2 IS NULL OR lot_area_centi_m2 > 0),
    built_area_centi_m2       INTEGER CHECK (built_area_centi_m2 IS NULL OR built_area_centi_m2 > 0),
    usable_area_centi_m2      INTEGER CHECK (usable_area_centi_m2 IS NULL OR usable_area_centi_m2 > 0),
    characteristics_json      TEXT CHECK (characteristics_json IS NULL OR json_valid(characteristics_json)),
    canonical_provenance_json TEXT CHECK (canonical_provenance_json IS NULL OR json_valid(canonical_provenance_json)),
    -- Deterministic local ranking (written by a later phase), with the components that explain it.
    local_score               REAL CHECK (local_score IS NULL OR local_score BETWEEN 0 AND 100),
    local_score_json          TEXT CHECK (local_score_json IS NULL OR json_valid(local_score_json)),
    data_confidence           TEXT CHECK (data_confidence IS NULL OR data_confidence IN ('low', 'medium', 'high')),
    -- User decisions live on the property, so hiding survives re-scraping and rediscovery.
    user_status               TEXT NOT NULL DEFAULT 'new' CHECK (user_status IN ('new', 'favorite', 'hidden')),
    hidden_at                 TEXT,
    match_review_status       TEXT NOT NULL DEFAULT 'none' CHECK (match_review_status IN ('none', 'review_required', 'reviewed')),
    -- Soft merge: when set, this property has been folded into another. Listings keep pointing at
    -- their original property, so undoing a merge is clearing these two columns.
    merged_into_property_id   INTEGER REFERENCES properties (id) ON DELETE RESTRICT,
    merged_at                 TEXT,
    created_at                TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    updated_at                TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    CHECK ((user_status = 'hidden') = (hidden_at IS NOT NULL)),
    CHECK ((merged_into_property_id IS NULL) = (merged_at IS NULL)),
    CHECK (merged_into_property_id IS NULL OR merged_into_property_id <> id)
) STRICT;

CREATE INDEX idx_properties_region ON properties (region_id);
CREATE INDEX idx_properties_user_status ON properties (user_status);
CREATE INDEX idx_properties_place ON properties (municipality, neighborhood);
CREATE INDEX idx_properties_merged_into ON properties (merged_into_property_id) WHERE merged_into_property_id IS NOT NULL;

-- ---------------------------------------------------------------------------------------------
-- listing_links: one individual advertisement on one platform.
-- Keeps its own price and characteristics even when several ads describe the same property.
-- ---------------------------------------------------------------------------------------------
CREATE TABLE listing_links (
    id                   INTEGER PRIMARY KEY AUTOINCREMENT,
    -- NULL until the listing has been associated with a property.
    property_id          INTEGER REFERENCES properties (id) ON DELETE RESTRICT,
    source_platform      TEXT NOT NULL CHECK (length(source_platform) > 0),
    adapter_id           TEXT NOT NULL CHECK (length(adapter_id) > 0),
    -- The platform's own identifier for the ad, when it has one.
    source_listing_id    TEXT,
    url                  TEXT NOT NULL CHECK (length(url) > 0),
    -- Same ad seen again resolves to the same row via (adapter_id, normalized_url).
    normalized_url       TEXT NOT NULL CHECK (length(normalized_url) > 0),
    title                TEXT,
    raw_description      TEXT,
    location_text        TEXT,
    -- Headline price of this ad and what it is. Every amount found (ágio, down payment...) is in
    -- extracted_facts_json; the headline is the one chosen by NormalizedListing::headline_price.
    price_centavos       INTEGER CHECK (price_centavos IS NULL OR price_centavos >= 0),
    price_classification TEXT NOT NULL DEFAULT 'unknown' CHECK (price_classification IN
                           ('full_price', 'agio', 'down_payment', 'monthly_payment', 'outstanding_balance', 'unknown')),
    -- Confidence in the classification above.
    price_confidence     TEXT CHECK (price_confidence IS NULL OR price_confidence IN ('low', 'medium', 'high')),
    currency             TEXT NOT NULL DEFAULT 'BRL' CHECK (currency IN ('BRL')),
    -- Serialised models::listing::ExtractedFacts: every extracted value with its evidence.
    extracted_facts_json TEXT CHECK (extracted_facts_json IS NULL OR json_valid(extracted_facts_json)),
    parser_version       TEXT,
    listing_status       TEXT NOT NULL DEFAULT 'unknown' CHECK (listing_status IN ('active', 'unavailable', 'unknown')),
    extraction_status    TEXT NOT NULL DEFAULT 'pending' CHECK (extraction_status IN ('pending', 'ok', 'partial', 'failed')),
    extraction_error     TEXT,
    -- first_seen != last_scraped: a freshly scraped ad is not necessarily a newly posted one.
    first_seen_at        TEXT NOT NULL,
    last_seen_at         TEXT NOT NULL,
    -- Last time the ad was confirmed live AND parsed correctly (a fetch alone is not verification).
    last_verified_at     TEXT,
    last_scraped_at      TEXT,
    created_at           TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    updated_at           TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    UNIQUE (adapter_id, normalized_url),
    CHECK ((price_centavos IS NULL) = (price_confidence IS NULL)),
    CHECK (price_centavos IS NOT NULL OR price_classification = 'unknown'),
    CHECK (last_seen_at >= first_seen_at)
) STRICT;

CREATE INDEX idx_listing_links_property ON listing_links (property_id);
CREATE INDEX idx_listing_links_status_seen ON listing_links (listing_status, last_seen_at);
CREATE INDEX idx_listing_links_price ON listing_links (price_classification, price_centavos);

-- ---------------------------------------------------------------------------------------------
-- property_images: image METADATA (files live in the app cache, referenced by relative path).
-- ---------------------------------------------------------------------------------------------
CREATE TABLE property_images (
    id                INTEGER PRIMARY KEY AUTOINCREMENT,
    listing_id        INTEGER NOT NULL REFERENCES listing_links (id) ON DELETE CASCADE,
    position          INTEGER NOT NULL DEFAULT 0 CHECK (position >= 0),
    source_url        TEXT,
    -- Relative to the application cache directory; never an absolute or drive-qualified path.
    local_path        TEXT CHECK (local_path IS NULL OR
                        (length(local_path) > 0 AND instr(local_path, ':') = 0 AND substr(local_path, 1, 1) NOT IN ('/', char(92)))),
    width             INTEGER CHECK (width IS NULL OR width > 0),
    height            INTEGER CHECK (height IS NULL OR height > 0),
    -- 64-bit perceptual hash stored as the signed INTEGER with the same bit pattern
    -- (i64::from_ne_bytes(u64.to_ne_bytes())). Hamming distance is computed in Rust.
    phash             INTEGER,
    phash_algorithm   TEXT,
    -- One embedding per image row: little-endian f32 values, `embedding_dim` of them, produced by
    -- `embedding_model` (name@version). Embeddings from different models are never comparable.
    embedding         BLOB,
    embedding_model   TEXT,
    embedding_dim     INTEGER CHECK (embedding_dim IS NULL OR embedding_dim > 0),
    embedding_dtype   TEXT CHECK (embedding_dtype IS NULL OR embedding_dtype = 'f32le'),
    processing_status TEXT NOT NULL DEFAULT 'pending' CHECK (processing_status IN
                        ('pending', 'hashed', 'embedded', 'failed', 'skipped')),
    processing_error  TEXT,
    created_at        TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    processed_at      TEXT,
    UNIQUE (listing_id, source_url),
    CHECK ((phash IS NULL) = (phash_algorithm IS NULL)),
    CHECK ((embedding IS NULL) = (embedding_model IS NULL)
       AND (embedding IS NULL) = (embedding_dim IS NULL)
       AND (embedding IS NULL) = (embedding_dtype IS NULL)),
    CHECK (embedding IS NULL OR length(embedding) = embedding_dim * 4)
) STRICT;

CREATE INDEX idx_property_images_listing ON property_images (listing_id);
CREATE INDEX idx_property_images_phash ON property_images (phash) WHERE phash IS NOT NULL;
CREATE INDEX idx_property_images_pending ON property_images (processing_status) WHERE processing_status = 'pending';

-- ---------------------------------------------------------------------------------------------
-- property_matches: potential "these two ads are the same property" decisions.
--
-- A merge is soft (see properties.merged_into_property_id) and reversible: reverting sets
-- decision_status = 'rejected' (so the matcher does not redo it) and stamps reverted_at, keeping
-- the original evidence and the merge ids as a record of what was undone.
-- ---------------------------------------------------------------------------------------------
CREATE TABLE property_matches (
    id                          INTEGER PRIMARY KEY AUTOINCREMENT,
    listing_a_id                INTEGER NOT NULL REFERENCES listing_links (id) ON DELETE RESTRICT,
    listing_b_id                INTEGER NOT NULL REFERENCES listing_links (id) ON DELETE RESTRICT,
    phash_distance              INTEGER CHECK (phash_distance IS NULL OR phash_distance BETWEEN 0 AND 64),
    visual_similarity           REAL CHECK (visual_similarity IS NULL OR visual_similarity BETWEEN -1.0 AND 1.0),
    visual_model                TEXT,
    metadata_signals_json       TEXT CHECK (metadata_signals_json IS NULL OR json_valid(metadata_signals_json)),
    -- Every independent signal considered, including the ones that disagree.
    evidence_json               TEXT NOT NULL CHECK (json_valid(evidence_json)),
    confidence_category         TEXT NOT NULL CHECK (confidence_category IN ('high', 'possible', 'low')),
    confidence_score            REAL CHECK (confidence_score IS NULL OR confidence_score BETWEEN 0.0 AND 1.0),
    decision_status             TEXT NOT NULL DEFAULT 'candidate' CHECK (decision_status IN
                                  ('candidate', 'accepted', 'rejected', 'review_required')),
    -- Who decided: the matcher or the user. Present exactly when a decision exists.
    decision_source             TEXT CHECK (decision_source IS NULL OR decision_source IN ('auto', 'user')),
    decision_note               TEXT,
    merge_surviving_property_id INTEGER REFERENCES properties (id) ON DELETE RESTRICT,
    merge_absorbed_property_id  INTEGER REFERENCES properties (id) ON DELETE RESTRICT,
    created_at                  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    reviewed_at                 TEXT,
    reverted_at                 TEXT,
    UNIQUE (listing_a_id, listing_b_id),
    -- Canonical ordering so (a,b) and (b,a) cannot both exist.
    CHECK (listing_a_id < listing_b_id),
    CHECK ((visual_similarity IS NULL) = (visual_model IS NULL)),
    CHECK ((decision_status IN ('accepted', 'rejected')) = (decision_source IS NOT NULL AND reviewed_at IS NOT NULL)),
    -- The matcher may only merge on its own when the evidence is categorised as high confidence.
    CHECK (NOT (decision_status = 'accepted' AND decision_source = 'auto') OR confidence_category = 'high'),
    CHECK ((merge_surviving_property_id IS NULL) = (merge_absorbed_property_id IS NULL)),
    CHECK (merge_surviving_property_id IS NULL OR merge_surviving_property_id <> merge_absorbed_property_id),
    -- A recorded merge is either live (accepted) or reverted (rejected + reverted_at).
    CHECK (merge_surviving_property_id IS NULL
        OR (decision_status = 'accepted' AND reverted_at IS NULL)
        OR (decision_status = 'rejected' AND reverted_at IS NOT NULL)),
    CHECK (reverted_at IS NULL OR merge_surviving_property_id IS NOT NULL)
) STRICT;

CREATE INDEX idx_property_matches_listing_b ON property_matches (listing_b_id);
CREATE INDEX idx_property_matches_status ON property_matches (decision_status);

-- ---------------------------------------------------------------------------------------------
-- listing_price_history: every distinct price a listing has shown. Append-only.
-- ---------------------------------------------------------------------------------------------
CREATE TABLE listing_price_history (
    id                   INTEGER PRIMARY KEY AUTOINCREMENT,
    listing_id           INTEGER NOT NULL REFERENCES listing_links (id) ON DELETE RESTRICT,
    amount_centavos      INTEGER NOT NULL CHECK (amount_centavos >= 0),
    price_classification TEXT NOT NULL CHECK (price_classification IN
                           ('full_price', 'agio', 'down_payment', 'monthly_payment', 'outstanding_balance', 'unknown')),
    confidence           TEXT CHECK (confidence IS NULL OR confidence IN ('low', 'medium', 'high')),
    observed_at          TEXT NOT NULL,
    -- Extractor, source field and evidence snippet that justified this observation.
    provenance_json      TEXT NOT NULL CHECK (json_valid(provenance_json))
) STRICT;

CREATE INDEX idx_price_history_listing ON listing_price_history (listing_id, observed_at DESC, id DESC);

-- A refresh must never rewrite history. Enforced here so no code path can do it by accident.
CREATE TRIGGER listing_price_history_no_update BEFORE UPDATE ON listing_price_history
BEGIN
    SELECT RAISE(ABORT, 'listing_price_history is append-only');
END;

CREATE TRIGGER listing_price_history_no_delete BEFORE DELETE ON listing_price_history
BEGIN
    SELECT RAISE(ABORT, 'listing_price_history is append-only');
END;

-- ---------------------------------------------------------------------------------------------
-- updated_at maintenance (recursive triggers are off by default, and the WHEN clause stops
-- the trigger from re-firing itself even if they are turned on).
-- ---------------------------------------------------------------------------------------------
CREATE TRIGGER regions_touch_updated_at AFTER UPDATE ON regions
WHEN NEW.updated_at = OLD.updated_at
BEGIN
    UPDATE regions SET updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now') WHERE id = NEW.id;
END;

CREATE TRIGGER properties_touch_updated_at AFTER UPDATE ON properties
WHEN NEW.updated_at = OLD.updated_at
BEGIN
    UPDATE properties SET updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now') WHERE id = NEW.id;
END;

CREATE TRIGGER listing_links_touch_updated_at AFTER UPDATE ON listing_links
WHEN NEW.updated_at = OLD.updated_at
BEGIN
    UPDATE listing_links SET updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now') WHERE id = NEW.id;
END;
