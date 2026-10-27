# Phase 2 design: source collection and conservative matching

Status: proposed for user review, 2026-10-27. No Phase 2 implementation has begun.

## Goal and boundaries

Collect and retain residential property advertisements from permitted sources in the Goiânia region, normalize them locally, and identify possible duplicate properties with evidence and reversible decisions. Existing local data remains available offline. Remote collection requires connectivity; AI, ranking, the finished interface, and Windows packaging remain outside this phase. Rust owns all collection, persistence, parsing, and matching; React only calls typed Tauri commands.

Evaluate 62imoveis, OLX, ZAP Imóveis, and local brokerages. Start with two sources that can be accessed permissibly and reliably, chosen after evaluation. Review the others and defer additional adapters until the first two are reliable. If fewer than two qualify or access requires a third-party agreement, report the gap and continue independent infrastructure and tests; do not represent Phase 2 as complete or substitute mock sources for real integrations. No outreach, account creation, purchases, or sharing credentials is implied by this design.

## Alternatives and recommendation

1. **Recommended: independent source adapters with a shared, bounded pipeline.** Adapters describe their actual access method and return a common raw listing; normalization, persistence, source health, and matching remain source-independent. This costs some initial interface work but makes failures visible and enables isolated tests.
2. **One general scraper for all websites.** Faster to prototype, but site-specific rules, access conditions, and changing markup make a single generic collector brittle and hard to validate.
3. **A single external aggregation provider.** Reduces adapter maintenance if a suitable licensed provider exists, but introduces a dependency, possible cost, data coverage questions, and uncertain access. Evaluate such feeds as source options rather than assume one exists.

For matching, use deterministic evidence and a review queue. Image similarity is candidate evidence, never proof. Initially disable automatic merges until the evaluation set demonstrates that an explicit rule is safe; reviewable candidates and reversible user decisions can work while calibration is incomplete.

## Source access and discovery

Create a dated source-evaluation record for each candidate: applicable terms, robots rules, authorized API/feed options, scope of access, rate limits, data and image permissions, discovery method, rendering needs, stable identity, field coverage, and observed limitations. Verify exact paths and terms immediately before integration; robots permission alone is not authorization. Do not bypass logins, anti-bot systems, or restrictions. Instagram and Facebook require permitted APIs or approved providers.

The preliminary review found a [Grupo OLX integration portal](https://developers.grupozap.com/) and its [feed connection guide](https://developers.grupozap.com/feeds/connection_guideline.html); the guide describes sending feeds to the group, not an entitlement to search third-party listings. The [OLX terms page](https://ajuda.olx.com.br/s/article/termos-e-condicoes-de-uso) and [62imoveis privacy policy](https://www.62imoveis.com.br/politica-de-privacidade) are starting points, not evidence that automated collection is permitted. Brokerage candidates and specific authorized collection methods remain to be established.

Each adapter has a stable ID and implements discovery and retrieval using its permitted method. It returns source IDs, original URLs, raw structured fields, description, location, image references, and extraction diagnostics, with the source field of each value retained. URL normalization must preserve identity and meaningful parameters; deduplicate by adapter and normalized URL without combining distinct advertisements. Stable platform IDs may provide additional identity evidence but are not assumed globally unique.

## Collection, persistence, and health

The pipeline is: search request → adapter discovery → bounded fetch → source extraction → local normalization and parser → transactional upsert and price observation → image processing → match candidates. Define a source adapter contract and shared raw listing type, with explicit errors and no source-specific assumptions in the shared layer. Choose HTTP, HTML, URL, and image libraries against current APIs and actual source needs; introduce a browser only if an authorized source demonstrably requires it.

Per-source configuration sets request concurrency, timeouts, bounded retries with backoff, and caching, respecting any stricter provider limits. Use global bounds as well, validate redirects and external URLs, and impose response, image-size, and decoding limits. Never execute listing content. Record each run and per-source outcome as **results**, **successful empty**, **failed**, or **unreliable/unparseable**, with counts and useful error details. A partial result retains its warnings and is not presented as a complete search. Existing saved results remain available after failure or cancellation.

Keep network and image work off the UI thread and outside the SQLite mutex. Use short transactions to create or update a listing, preserve original source identity, record extracted facts and provenance, and append a changed price observation atomically with the associated listing update. Repeated identical observations must not create false price changes; unavailable ads retain history. Add versioned migrations instead of rewriting schema v1. Keep matching decisions, source status, and user status durable; never silently erase conflicts.

Seed a validated, extensible region configuration for the municipalities and example neighborhoods in the original specification. Stable seed IDs make repeated import idempotent without deleting user data. Commands expose collection, progress and cancellation where feasible, source outcomes, paginated saved results, and the full-price filter. Provide typed contracts and minimal diagnostics sufficient to exercise them; the polished search UI remains Phase 4.

## Price classification and calibration

Maintain the existing parser API and add an explicit, source-attributed structured asking-price hint. Apply it only to the designated field. Explicit financing labels and contradictions in the field or description take precedence; conflicting evidence remains unknown or conflicting rather than becoming a comparable full price. Preserve the original snippet and source field, bump the parser version, and add adversarial tests, including an ágio placed in a headline price field.

The existing Medium/High labels are heuristics, not established probabilities. Build a labeled, sanitized sample of permitted real advertisements across available sources and price types. Measure full-price classification precision by source and rule/confidence band, report sample sizes, errors, and uncertainty, and use separate evaluation examples when tuning permits. The user requires less than 60% certainty to be Low. A small sample with a point estimate above 60% alone does not prove a calibrated individual probability; where evidence is insufficient or a rule fails the threshold, keep it Low or otherwise prevent it from qualifying as comparable. Do not treat a proposed 85% High boundary as an approved requirement.

The full-price-only filter includes only one non-conflicting full price meeting the established minimum confidence; ágio, installments, down payments, balances, unknown amounts, and low-confidence candidates cannot qualify. Price displays and history retain classification and provenance.

## Image and matching pipeline

Cache permitted image data under the app cache using safe relative paths, bounded downloads, decoded dimensions and byte sizes. Compute and version a 64-bit perceptual hash; store it according to the existing schema and use Hamming distance only to retrieve candidates. Corrupt, unavailable, or disallowed images produce explicit processing outcomes, not a failed search or a match.

Generate candidate pairs using compatible location/metadata and/or image hashes. Compare independent evidence: image hashes, text and source identity, location, compatible area kinds, property type, rooms, and comparable prices. Preserve missing and contradictory evidence. Similar price, neighborhood, area, or image alone can never establish identity. Candidate generation thresholds are configuration/experimental values, not proof of identity.

Outcomes are high confidence, possible/review, and low/separate. Store the decision, inputs, evidence, rule version, and conflicts. Automatic merging is disabled until evaluation supports a narrowly specified multiple-signal rule; if no such rule is supported, leave all candidates separate or for review and report the limitation. Merge and undo are atomic: retain listing ownership, mark the absorbed property with the existing soft-merge pointer, keep an audit record, and restore relationships on undo. Prevent merge cycles and make repeated decisions idempotent. Define how favorites/hidden status and later rediscovery follow a merge without losing user choices; test merge chains and undo before enabling auto-merge.

## Validation and completion

Use test-first changes for parser hints, adapter contracts, repositories, migrations, source failure states, rate and cancellation behavior, image limits, matching, and merge/undo. Run Rust formatting, locked tests and Clippy, frontend typecheck/build, and relevant integration tests. Use sanitized recorded examples and authorized live checks for each implemented source; report what was actually tested, including source permission evidence and any access limitations. No personal contact details or private credentials belong in fixtures or the repository.

Create a manually labeled matching evaluation set covering same-property cross-source and different-angle ads, similar properties and condominiums, image transformations, and conflicting metadata. Report false-positive merges and missed duplicates separately, with counts, denominators and dataset limitations. Prefer preventing a false merge over maximizing recall. The evaluation must reflect real source data where authorized; synthetic tests alone do not establish field performance.

Phase 2 is complete only when two permitted real adapters work reliably, per-source outcomes are accurate, history and user decisions survive repeated runs, matching and reversal are verified, and price and match evaluations are reported with their limitations. If access, examples, or confidence evidence are insufficient, report the work as incomplete and identify the specific dependency. Stop for explicit Phase 3 approval.
