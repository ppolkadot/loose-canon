-- Migration 3: application settings, ranking provenance and the AI prioritisation cache.
--
-- Additive only: nothing from schema v1/v2 is dropped or rewritten. Conventions are those of
-- schema.sql (STRICT tables, UTC millisecond timestamps, history-like references RESTRICT so a
-- cascade can never delete history).

-- ---------------------------------------------------------------------------------------------
-- app_settings: small key/value JSON documents (search preferences, ranking weights, AI options).
-- SECRETS NEVER GO HERE: the optional Gemini key lives in the operating system's credential store.
-- ---------------------------------------------------------------------------------------------
CREATE TABLE app_settings (
    key        TEXT PRIMARY KEY CHECK (length(key) > 0 AND length(key) <= 100),
    value_json TEXT NOT NULL CHECK (json_valid(value_json)),
    updated_at TEXT NOT NULL
) STRICT;

-- ---------------------------------------------------------------------------------------------
-- Which version of the ranking rules produced `properties.local_score`, and when. The score,
-- its explanation (`local_score_json`) and the data confidence already have columns in schema v1.
-- ---------------------------------------------------------------------------------------------
ALTER TABLE properties ADD COLUMN ranking_version TEXT;
ALTER TABLE properties ADD COLUMN scored_at TEXT;

-- ---------------------------------------------------------------------------------------------
-- ai_prioritizations: one row per distinct request. `fingerprint` is the SHA-256 (hex) of the
-- model, the prompt version and every candidate's relevant data, so changed data is a cache miss.
-- `response_json` is the validated response (never the raw, unchecked one).
-- ---------------------------------------------------------------------------------------------
CREATE TABLE ai_prioritizations (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    fingerprint     TEXT NOT NULL UNIQUE CHECK (length(fingerprint) = 64),
    model           TEXT NOT NULL CHECK (length(model) > 0),
    prompt_version  TEXT NOT NULL CHECK (length(prompt_version) > 0),
    candidate_count INTEGER NOT NULL CHECK (candidate_count >= 1),
    response_json   TEXT NOT NULL CHECK (json_valid(response_json)),
    -- Things dropped or adjusted while validating (for example an unsupported claim).
    warnings_json   TEXT NOT NULL DEFAULT '[]' CHECK (json_valid(warnings_json)),
    created_at      TEXT NOT NULL
) STRICT;

-- ---------------------------------------------------------------------------------------------
-- ai_property_notes: the AI's note for one property in one prioritization. `property_fingerprint`
-- identifies the property data the note was based on; when the property's current fingerprint
-- differs, the note is out of date and is shown as such. Supplementary: never replaces the score.
-- ---------------------------------------------------------------------------------------------
CREATE TABLE ai_property_notes (
    property_id          INTEGER NOT NULL REFERENCES properties (id) ON DELETE RESTRICT,
    prioritization_id    INTEGER NOT NULL REFERENCES ai_prioritizations (id) ON DELETE RESTRICT,
    property_fingerprint TEXT NOT NULL CHECK (length(property_fingerprint) = 64),
    ai_rank              INTEGER NOT NULL CHECK (ai_rank >= 1),
    explanation          TEXT NOT NULL CHECK (length(explanation) > 0),
    positives_json       TEXT NOT NULL CHECK (json_valid(positives_json)),
    concerns_json        TEXT NOT NULL CHECK (json_valid(concerns_json)),
    uncertainties_json   TEXT NOT NULL CHECK (json_valid(uncertainties_json)),
    PRIMARY KEY (property_id, prioritization_id)
) STRICT;

CREATE INDEX idx_ai_notes_prioritization ON ai_property_notes (prioritization_id, ai_rank);
