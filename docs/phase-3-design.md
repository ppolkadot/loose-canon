# Phase 3 design: local ranking and optional Gemini prioritisation

Status: written 2026-10-28 by the implementing session, after the user asked to continue with the
remaining phases. It implements section 7 of the specification ("Two-Tier Ranking Engine") and the
Phase 3 bullet list. Decisions that the specification leaves open are marked **(decision)** so the
owner can reverse them.

## Goals and boundaries

* A deterministic **local score** (0-100) computed in Rust from the user's preferences and the
  collected listings. It works with no network, no key and no AI. It is labelled
  "Pontuação local" / "Compatibilidade com seus critérios", never an investment score.
* A separate **data-confidence** indicator (low / medium / high) with the reasons it is not higher.
* Every score stores its components and evidence, so a result can always be explained.
* **Optional** Gemini prioritisation of the top 15-30 candidates: compact structured input, strict
  validation of the whole response, caching, quota/error handling, and a result that is always
  presented as supplementary. It never overwrites the local score and a failure never affects the
  local ranking.
* Out of scope here: the interface (Phase 4), source adapters (Phase 2), any vision model.

## Local score

Four components with the specification's initial weights (hypotheses, configurable, stored with
the ranking version): price per comparable m² 45 %, cross-listing price difference 25 %,
bedroom/suite/parking fit 15 %, freshness 15 %.

* **A component that cannot be computed is *unavailable*, never zero.** The score is the weighted
  mean over the available components (weights renormalised), and the missing share lowers the
  confidence. A property with no available component has no score.
* **Price per m²** uses the property's *established* full price: the median of the comparable
  prices of its active listings **(decision: median, not the lowest, so one outlier listing cannot
  pick the price)**. Built-area and lot-area price per m² are computed **separately**, each against
  properties with the same area kind and the same property type (unknown matches only unknown), and
  never when the area kind is unknown. The sub-score is the percentile position inside that segment
  (cheaper per m² = higher), computed only with at least `min_comparables` (default 5) comparables;
  the segment is the neighbourhood, falling back to the municipality (recorded, and capped at
  medium confidence). The component is the mean of the available sub-scores. A low price per m² is
  a signal, not a verdict that the property is a good purchase.
* **Cross-listing difference** exists only for a property that the user has merged (an accepted
  match) and that has at least two established full prices on active listings of different sources:
  the spread `(high - low) / high`, mapped linearly to 0-100 at a configurable cap (default 10 %).
  Ágio, down payments, instalments and unclassified prices never enter (they are not established
  prices). It is labelled a price *difference between advertisements*, not a verified discount.
* **Fit** scores against stated preferences only (bedrooms range, minimum suites, minimum parking):
  a criterion that is met scores 100 and more is not rewarded; a shortfall decays linearly; an
  unknown value makes that criterion unavailable. With no preference stated the component is
  unavailable.
* **Freshness** is based on the earliest `first_seen_at` among the active listings, never on
  `last_scraped_at` **(decision: linear decay over 60 days; it measures how long the application
  has known the ad, not when it was posted, and says so)**. A listing last scraped long ago is not
  "stale" in score but lowers the confidence (it may be gone).
* Only **active** listings count. A property with no active listing is not ranked.

## Data confidence

Starts at high and is lowered, with a recorded reason for each lowering: no price or no area kind
(low); less than half the weight available (low), less than three quarters (at most medium); fewer
than 10 comparables, a municipality-level segment, an unknown property type, a conflicting
canonical value, comparable prices that are only medium confidence, or data last scraped more than
14 days ago (each at most medium). The current parser never extracts a property type, so `high` is
unreachable for parsed listings until a type extractor exists; that is intended.

## Storage

Migration 0003 (additive): `app_settings` (key/value JSON: search preferences, ranking weights, AI
settings; **never the API key**), `properties.ranking_version` and `scored_at`, `ai_prioritizations`
(one row per distinct request fingerprint, holding the validated response) and `ai_property_notes`
(per property: rank, explanation, positives, concerns, uncertainties, and the fingerprint of the
property data it was based on, so a note is shown as out of date when the data changed). Nothing is
deleted. The score lives in the existing `local_score`, `local_score_json`, `data_confidence`
columns.

## Gemini

* REST from Rust (`reqwest`), `generateContent` with the key in the `x-goog-api-key` header, JSON
  response mode with a response schema. The **model name is a setting** **(decision: default
  `gemini-2.5-flash`; I could not verify the current model list from here, so the default is
  unverified)**. The base URL is configurable so tests use a local server.
* The key is stored in the Windows credential store (`keyring`), never in SQLite, files, logs or
  the repository, and is never returned to the interface (only "configured / not configured").
* **Input:** only a compact record per candidate (id, price and its classification, built and lot
  area separately, rooms, neighbourhood, type, listing count, source price spread, facts with
  their evidence, unknown and conflicting facts, local score and confidence, a bounded excerpt of
  the description with phone numbers, e-mail addresses and URLs removed). No contact data.
* **Prompt:** Portuguese instructions, versioned (`PROMPT_VERSION`); the model must separate what
  the text states from what it infers and must not claim features the evidence does not support.
* **Validation** of the complete response before anything is shown: valid JSON of the exact
  schema; the set of ids equals the set sent (no unknown, duplicate or missing id); ranks are a
  permutation of 1..n; explanation present and bounded. **(decision)** A positive signal whose
  quoted evidence does not occur in the text sent is dropped and counted as a warning (an
  unsupported claim is never displayed); everything else that fails rejects the whole response and
  the deterministic ranking stands.
* **Cache** keyed by a fingerprint of the model, prompt version and every candidate's relevant
  data; changed property data changes the fingerprint (miss) and marks old notes out of date.
* **Errors:** no key, AI disabled, quota (HTTP 429, never retried, retry-after reported), timeout,
  network, server error (one bounded retry), safety block, truncated or malformed output,
  rejected output, cancelled. Each is reported as its own kind; none changes the local ranking.
* One AI request at a time, cancellable, bounded in time and size.

## Verification plan

Red-first tests for every rule above; a hand-built, hand-reviewed sample of properties with the
expected ordering and confidence (synthetic, and labelled so: it checks the rules, not market
reality); the local ranking is exercised with the AI disabled and with no key; the Gemini client
is tested against a local HTTP server that replays success, quota, server error, safety block,
truncation and malformed output. **The live Gemini API is not exercised** (no key is available);
the request shape follows the public REST description and is unverified against the live service.
