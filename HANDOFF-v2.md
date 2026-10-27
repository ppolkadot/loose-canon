# Loose Canon — Session Handoff v2 (Phase 2 work session)

Snapshot date: **2026-10-27**. Model used in the session: **Claude Sonnet 5.5**.
Audience: the next engineer or AI session that continues this project.

This file is the companion of `HANDOFF.md` (v1, end of Phase 1). v1 stays the reference for the
domain model, schema v1, the parser's original behaviour and the verbatim specification (its
Appendix B). **Where the two disagree, v2 wins** (Phase 2 status, test counts, parser version,
commands, file layout). Everything here is either verified by a command run during the session
(evidence in section 10) or explicitly labelled as an assumption, a proposal, or *not verified*.

> **Redaction note.** This file omits the user's account email address and machine-specific
> absolute paths / Windows user name. Paths are relative to the repository root (`<repo>`).

---

## 0. Status at a glance

| Item | State |
|---|---|
| Phase 1 — Foundation | Done and verified (see v1). Committed by the user in `b7e3221 first`. |
| Phase 2 — Collection and matching | **Implemented except the real source adapters; INCOMPLETE.** `sources::registered_adapters()` returns an empty list, so the plan's completion criterion "two real adapters work reliably" is **not met (0 of 2)**. Everything that does not depend on a real adapter is built and tested. Phase 2 must not be reported as complete. |
| Phase 3 — Ranking and optional Gemini | Not started. Needs explicit approval from the user. |
| Phase 4 — React UI and Windows packaging | Not started. The frontend is a diagnostics panel only. |
| Git | Branch `main`: `f0153e4 Initial commit`, `b7e3221 first` (Phase 1), `9527dab new path` (adds `docs/phase-2-design.md`). **All Phase 2 work is uncommitted** (21 modified tracked files, many new files, see section 6); the user has not asked for a commit. |
| Tests | `cargo test --locked`: **545 passed, 0 failed** (Phase 1 had 149). Breakdown in Appendix A. |
| Static checks | `cargo fmt --check` clean; `cargo clippy --all-targets --locked -- -D warnings` clean (pedantic on); `npm run typecheck` and `npm run build` pass. |
| Mutation checks | 73 mutations of the Phase 2 safety rules, each caught by a test (one equivalent mutant kept deliberately, section 9). |
| Not verified | The desktop app was **not launched** in this phase; no live network traffic was exercised (the fetcher is tested against a local TCP server and scripted adapters); the declared MSRV 1.90 was not built (toolchain is 1.99); performance only checked up to a 4000-listing candidate scan; no Windows packaging. |
| Automatic merging | **Does not exist.** No code path merges on its own; every merge is a user decision. |

### The owner's open decisions (details in section 13)

Confirm the merged-group status rule; supply a labelled sample of real advertisements (for price
calibration and for matching evaluation); decide on property-type extraction in the parser;
confirm the apartment cap; say when to commit; explicitly approve Phase 3.

---

## 1. How to resume (first 15 minutes)

```powershell
cd <repo>
npm install                       # only if node_modules is missing
npm run build                     # tauri embeds ../dist at compile time: build the frontend first
cd src-tauri
cargo fmt --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --locked               # expect 545 passed
cd ..
npm run typecheck
git status --short                # Phase 2 work is uncommitted; do not lose it
```

Reading order for a new session:

1. This file, sections 0, 3, 7 and 13.
2. `docs/phase-2-design.md` (the plan the user supplied and approved), then
   `docs/phase-2-alignment-review.md` (status report written at the end of this session).
3. `docs/matching.md` (matching rule, merge/undo semantics, evaluation harness).
4. `src-tauri/src/service/mod.rs` (how a collection flows end to end), then
   `src/lib/api.ts` (the typed contract with the frontend).
5. v1 `HANDOFF.md` sections 8-11 for the Phase 1 domain model, schema v1 and parser.

Memory files for this project live in the Claude memory directory
(`loose-canon-phase2-decisions`, `loose-canon-phase2-status`); they are short pointers, not a
substitute for this file.

---

## 2. Working rules that apply to this project (from the specification and the user)

* Rust owns the database and all logic. React only calls typed Tauri commands. No
  `@tauri-apps/plugin-sql`. SQL is parameterized; no secrets in the repository.
* No fabricated results and no placeholder pseudocode. Test doubles live only inside test
  modules; **the shipped registry contains no adapter** (section 8.4).
* Never silently overwrite conflicting data; never delete history; merges are reversible.
* AI (Gemini) is optional post-ranking enrichment only (Phase 3, not started).
* "No implementation beyond the current approved phase." The user approved Phase 2 on
  2026-10-27 conditional on the plan being judged adequate; Phase 3 is **not** approved.
* Do not commit unless the user asks.
* Money is integer centavos; areas are integer hundredths of m². No floats for either.

---

## 3. Decisions log

### 3.1 The user's answers and instructions

After Phase 1 (2026-10-26):

* **Price-field hint: approved** ("You can add the hint"). Now implemented (section 8.1).
* **Comparability threshold confirmed:** under 60 % certainty must never be classified as
  comparable. The current Medium/High bands are uncalibrated heuristics, so the hint's
  confidence defaults to `Low` until calibrated per source on labelled real data.
* **Bundle identifier:** the placeholder was rejected ("will be informed later") and the user has
  since supplied the real one, **`com.polariscompany.loosecanon`** (set in
  `src-tauri/tauri.conf.json` on 2026-10-27; do not change it without the user). It also names
  the per-user app-data folder, so the application now looks for its database under that name:
  a development database created earlier under the previous placeholder folder is **not** read
  (it was left untouched, nothing was moved or deleted).
* A complete handoff (v1) was requested and written; it was reviewed for sensitive content
  (none withheld; email and local paths redacted).

On 2026-10-27:

* The user added `docs/phase-2-design.md`, asked for it to be analysed for misalignment with the
  project plan, and allowed implementation if it was judged adequate. It was judged adequate and
  aligned (it narrows rather than widens the specification), and implemented.
* The user asked for this handoff v2 covering the session and its context in full detail.
* After a sensitive-information check, the user (a) supplied the real bundle identifier
  `com.polariscompany.loosecanon` (applied to `src-tauri/tauri.conf.json`; the previous placeholder
  was removed from every file and from the memory notes) and (b) asked for `.env` to be added to
  `.gitignore` (done: `.env` and `.env.*`). The check found no secrets, keys, personal data or
  machine paths in the files that would be committed.

### 3.2 Decisions I made where the plan was silent (the owner may reverse any)

| # | Decision | Why / where |
|---|---|---|
| D1 | Hint confidence defaults to `Low`; raised only by per-source calibration. | The 60 % rule; no labelled data exists. `SourceConfig.price_field_confidence`. |
| D2 | A property can only be merged into a **root** property (triggers); the property with the **smaller id** survives. | Makes cycles impossible while allowing chains; deterministic regardless of which listing is `a`. |
| D3 | A merged group shows **favorite > hidden > new** with a conflict flag; each member keeps its own stored status. | Never lose a user choice; undo restores each side. **Assumption awaiting confirmation.** |
| D4 | A pair already in one merge group records **no decision** (`already_same_group`). | An undo of another merge can never leave such a pair inconsistent. |
| D5 | **Apartments are capped at `possible`** (never `high`). | Identical flats of one building share photos, address and floorplan. Found while building the evaluation set. |
| D6 | Metadata-only `low` results are **not stored**; `low` results that came from photos are stored as `candidate`. | There would be far too many metadata pairs; the photo-derived ones document what was considered. |
| D7 | `property_matches.confidence_score` stays `NULL`. | The rule is uncalibrated; no probability is invented. |
| D8 | All matching thresholds are experimental configuration with integer arithmetic only. | `MatchConfig`; versioned `match_rules/1`. |
| D9 | The collection pipeline queues each stored listing's image URLs (`register_images`). | Found missing by the end-to-end test (section 11). |
| D10 | The source registry is a function returning an empty `Vec`, guarded by a test that asserts it is empty. | Update the test when an adapter is added. |
| D11 | Evidence stores only counts for text similarity ("6 of 10 distinct terms"), never the words; terms exclude digits. | Phone numbers and prices cannot leak into stored evidence. |
| D12 | The perceptual hash is implemented in this repository (`images/phash.rs`, `dct64-v1`) with a golden test and integer-exact area-average weights. | The hash cannot change with a library upgrade; deterministic across platforms. |
| D13 | Structured DTOs returned to the frontend are `camelCase`; **stored documents** (match evidence, a run's saved request, canonical provenance, merge snapshots) keep their stored `snake_case` shape. | Stored JSON must stay stable. |
| D14 | One collection at a time; a finished collection runs the image job, then a matching pass. | `service::AppService`. |
| D15 | The matching pass never merges. | Sections 8.6 (the pass) and 8.7 (merges are user decisions). |

---

## 4. Session context and timeline

### 4.1 Setting

* Windows 11 desktop, the user works in Portuguese and English; UI strings are pt-BR.
* The session was long and its context was compacted once; the early part below is reconstructed
  from the compaction summary, the later part from direct observation.
* A SessionStart hook injects the "superpowers" skill introduction, and a HawkScan hook asks for a
  DAST scan after code changes. HawkScan was **not run**: its runtime and API key are not
  available in this environment, and this is a desktop application with no web server to scan.

### 4.2 Chronological timeline (Phase 2 session)

1. **Plan review.** Read `docs/phase-2-design.md`, checked it against the specification and the
   Phase 1 code, concluded it was adequate and aligned, and listed the gaps to resolve by
   decision (hint confidence, merge cycles, merged-group status, evaluation honesty).
2. **Price-field hint (test-first).** Parser version `br_real_estate/2`; `StructuredText`,
   `PriceFieldHint`, `parse_listing_with_price_hint`, `parse_structured_text`; evidence now carries
   the source field it came from; warning `price_from_structured_field`; adversarial tests
   including an ágio placed in a headline price field.
3. **Schema v2 and repositories.** Additive migration `0002_collection_and_audit.sql`; repositories
   for listings (atomic upsert with price observation), properties (canonical values, merge
   groups, user status, paginated read model), runs/outcomes, regions (seed + resolver), images,
   price history (`record_observation_in`).
4. **Source layer.** `SourceAdapter` trait, `SourceConfig`, bounded `Fetcher`, URL normalisation,
   shared normalisation, pipeline with four outcomes, test TCP server, scripted adapters.
5. **Image layer.** Bounded decoder, `dct64-v1` pHash, multi-index near-pair search, cache with
   traversal protection, `Fetcher::get_image` (own size limit, never cached in memory).
6. *(Context compaction happened here.)*
7. **Image job** (`images/pipeline.rs`): red-first with 11 tests, implemented, 3 mutations
   verified; then 4 more tests (refusal stops a source, queue starvation, decoder panic,
   unwritable cache). Found and fixed a queue-starvation defect: `pending_images` had no adapter
   filter, so images of unregistered adapters could fill a batch. It now takes the adapter ids
   and `count_pending_outside` reports the rest.
8. **Clippy cleanup of earlier code** (38 pedantic errors, none blanket-allowed): float casts in
   `phash.rs` replaced by exact integer weights (golden hash unchanged), type aliases, renamed
   bindings, `run_source` split into `plan_work` / `empty_outcome` / `Tally`, `upsert_in` split
   into `insert_new` / `update_existing`, `get_property_view` and `refresh_canonical` split, one
   narrowly scoped `#[expect]` on a test fixture.
9. **Matching** (red-first throughout): domain types, comparison rule, candidate generation,
   persistence and merge/undo, loader, run, evaluation harness (sections 8.6 and 8.7). Each stage was
   followed by a mutation pass; weak tests found that way were strengthened.
10. **Rule weakness found and fixed:** identical flats of one building would have scored `high`;
    apartments are now capped at `possible` (D5).
11. **Application service and commands.** `service::AppService`; 18 typed commands; camelCase
    DTOs; progress and completion events; startup wiring in `lib.rs`.
12. **Defect found by the end-to-end service test:** the collection pipeline never queued images
    (D9). Fixed red-first at the pipeline level (5 tests).
13. **Frontend contract and diagnostics.** `src/lib/api.ts`, `src/Diagnostics.tsx`, `src/App.tsx`.
    Six Rust contract tests serialise real DTOs and assert their exact JSON keys.
14. **Documentation:** `docs/matching.md`, `docs/phase-2-alignment-review.md`, README, manifest
    comment, banner in v1 `HANDOFF.md`, memory files.
15. **Final verification** of the whole tree (section 10) and this handoff.

---

## 5. Environment facts

| Fact | Value |
|---|---|
| OS | Windows 11 Pro, 10.0.26200 |
| Shells | Git Bash (POSIX) and PowerShell 5.1 (the Bash tool is primary; both exist) |
| Rust / cargo | 1.99.0 / 1.99.0, edition 2024. Declared MSRV in `Cargo.toml`: 1.90 (Tauri 2.12 minimum), **not built** |
| Node / npm | v24.19.0 / 11.17.0 |
| Tauri CLI | 2.12.4 (`npx tauri ...`) |
| Rust deps (locked) | tauri 2.12.5, rusqlite 0.41.0 (bundled SQLite via libsqlite3-sys 0.39.1), serde 1.0.229, serde_json 1.0.154, chrono 0.4.45, regex 1.13.1, thiserror 2.0.21, tokio 1.54.1 (`rt-multi-thread`, `time`, `sync`, `macros`), reqwest 0.13.5 (`native-tls`, `gzip`, `charset`, `system-proxy`, default features off), url 2.5.8, image 0.25.10 (`jpeg`, `png`, `webp` only); dev: tempfile 3.27.0 and tokio `net`, `io-util` |
| JS deps | react ^19.3.1, react-dom ^19.3.1, @tauri-apps/api ^2.12.4; dev: vite ^8.3.5, typescript ^7.0.2, @vitejs/plugin-react ^6.1.2, @tauri-apps/cli ^2.12.4 |
| Lints | `unsafe_code = "forbid"`; clippy `all` and `pedantic` at warn, run with `-D warnings`; allowed: `module_name_repetitions`, `missing_errors_doc`, `missing_panics_doc` |
| Not added on purpose | No HTML-parsing dependency exists (none is needed yet); Phase 3 needs a Gemini REST client (reqwest is present) and `keyring`; Phase 4 an optional ONNX runtime |
| Bundle identifier | `com.polariscompany.loosecanon` (supplied by the user; see 3.1). Names the app-data and cache folders. |
| Frontend dev port | 1420 (`vite.config.ts`, strict) |

### 5.1 Tooling pitfalls met in this session

* **Heredocs in the Bash tool** failed for some bodies (a parse error that ran nothing). Write
  multi-line content with the file-writing tool, then `cat`/`head`/`tail` it into place.
* **Perl one-liners containing backslashes** are mangled by the tool layer; use perl **script
  files**. A single-quoted perl string does **not** turn `\n` into a newline (use `"..."`/`qq`).
* `python3 -` (script on stdin) hung twice; avoid it.
* The edit tool requires the file to have been read in the session, and refuses if it changed on
  disk since. After `cargo fmt` or a scripted edit, read again before editing.
* `cargo fmt` re-flows code and changes function lengths: a function at 99 lines can become 104
  and trip `clippy::too_many_lines`. Run fmt **before** clippy.
* `Result` inside a `tests` module that does `use super::*` can resolve to the crate's
  one-argument alias; write `std::result::Result<T, E>` there.
* `with_conn` hands out `&mut Connection`; passing a function taking `&Connection` directly does
  not type-check; use a closure.
* Windows reports a refused local TCP connect as a timeout; tests accept `Network | Timeout`.
* Line-number guards in edit scripts were off by one three times (0- vs 1-based). The guards
  stopped each script before it changed anything; keep such guards.
* A transient "no verdict" failure of the permission classifier happened once; retrying once
  as-is worked.

### 5.2 The mutation-check helper (not in the repository)

A small perl script was used all session (kept in the session scratchpad, so it may not persist).
Recreate it if needed: read the target source; verify every `from` pattern occurs exactly once
**before touching anything**; for each mutation write the mutated file, run
`cargo test --lib <filter>`, classify as *caught* (a `... FAILED` line), *SURVIVED* or *did not
compile* (output contains `could not compile`; do not match a bare `error:`, because cargo prints
`error: test failed` on ordinary failures); always restore the original (an `END` block and an
INT handler), and diff the restored file against a backup. An early version died on an unmatched
pattern and left a mutated file in the tree; it was restored from backup and verified identical.

---

## 6. Repository state

* Branch `main`, HEAD `9527dab`. `git status`: 21 modified tracked files and the new files below.
  **Nothing from Phase 2 is committed.** `Cargo.lock` changed (new dependencies).
* Modified tracked files: `.gitignore` (`.env`, `.env.*`), `HANDOFF.md` (banner and the identifier
  lines), `README.md`, `src-tauri/tauri.conf.json` (bundle identifier), `src-tauri/Cargo.toml`,
  `src-tauri/Cargo.lock`, `src/App.tsx`, `src/lib/api.ts`, `src-tauri/src/lib.rs`, `error.rs`,
  `commands/mod.rs`, `db/mod.rs`, `db/price_history.rs`, `db/tests.rs`, `models/characteristics.rs`,
  `models/evidence.rs`, `models/mod.rs`, `models/price.rs`, `parser/br_real_estate.rs`,
  `parser/mod.rs`, `parser/text.rs`.
* New: `docs/matching.md`, `docs/phase-2-alignment-review.md`, `src/Diagnostics.tsx`,
  `src-tauri/data/regions.json`, and under `src-tauri/src/`: `commands/{collection,matches,
  properties,contract_tests}.rs`, `db/{images,listings,matches,properties,regions,runs,testkit,
  migration_0002_tests}.rs` (+ their `*/tests.rs` folders), `db/migrations/0002_*.sql`, `images/`,
  `matching/`, `service/`, `sources/`, `models/{collection,image,matching,region}.rs`,
  `parser/br_real_estate/hint_tests.rs`.
* `docs/phase-2-design.md` is the user's file and was **left untouched** (its status line still
  says no implementation has begun; that is out of date).
* Size: about 13,750 lines of Rust outside test files and 12,250 lines of test code; the
  frontend is small (api.ts, Diagnostics.tsx, App.tsx, main.tsx).
* No personal data, credentials or machine paths were found in any tracked or untracked text
  file (checked with a search before finishing).

### 6.1 File inventory (non-test lines; test files in Appendix A)

```
src-tauri/src/
  main.rs 6, lib.rs 63, error.rs 126
  commands/    mod.rs 137, collection.rs 239, matches.rs 248, properties.rs 165, contract_tests.rs (test)
  service/     mod.rs 289
  models/      area 204, characteristics 186, collection 152, confidence 31, evidence 167, image 27,
               listing 230, matching 194, money 118, price 375, region 41, mod 67
  parser/      br_real_estate 1317, number 378, text 185, mod 12 (+ hint_tests)
  sources/     adapter 96, cancel 104, config 299, error 115, fetch 436, normalize 158, pipeline 823,
               url_norm 178, testserver 203 (test-only), mod 46
  images/      cache 56, decode 152, index 101, phash 148, pipeline 415, mod 8
  matching/    candidates 130, compare 602, evaluation 345, load 167, run 199, testkit 92 (test-only),
               mod 28
  db/          mod 234, images 304, listings 534, matches 926, price_history 165, properties 767,
               regions 604, runs 318, testkit 46 (test-only), schema.sql, migrations/0002_*.sql
```

---

## 7. Architecture and data flow

```
React diagnostics panel (src/)                        typed invoke + events (src/lib/api.ts)
        |
commands/*       thin: validate/convert arguments, call the service or a repository
        |
service::AppService   startup, one collection at a time, cancel, blocking/offload helpers
        |  (tokio task started by start_collection)
        +-- sources::pipeline::run_collection
        |       per adapter: discover -> plan (dedupe, cap) -> bounded fetch -> normalise
        |       -> atomic upsert + price observation -> queue image URLs
        +-- images::pipeline::process_pending_images
        |       download (Fetcher::get_image) -> decode + hash (blocking pool) -> cache file -> record
        +-- matching::run::run_matching
                load -> candidate pairs -> compare -> store proposals (never merges)

user decisions:  commands::matches::{accept_match, reject_match, undo_merge}
                 -> db::matches (one IMMEDIATE transaction each, idempotent, audited)
```

### 7.1 Concurrency and locking rules (enforced by structure and tests)

* There is **one SQLite connection** behind a `Mutex` (`Database::with_conn`). DB closures are short
  and run on the blocking pool (`sources::pipeline::with_db`, `AppService::blocking/offload`).
  **Network and image work never hold the lock.**
* Concurrency bounds: per source `SourceConfig.max_concurrency` (default 2, 1..=8) plus a global
  semaphore (`CollectionDeps::DEFAULT_GLOBAL_CONCURRENCY` = 4) shared by pages and images; image
  decoding is additionally bounded by a CPU semaphore of `available_parallelism` clamped to 1..=4.
* Every wait (permits, rate-limit slot, request, body chunks, backoff sleep) observes the
  `CancelToken`.
* One collection at a time: a `Busy` drop guard clears the flag when the job ends, even if the job
  panics, and clears it **before** the completion callback runs.
* A source adapter is run in a nested task so a panic inside it becomes a failed outcome for that
  source instead of taking the run down.

### 7.2 Error type (`error.rs`) and the kinds the frontend sees

`Error` serialises as `{ kind, message }`. Kinds: `database`, `io`, `json`, `model`, `parse`,
`schema_too_new`, `migration`, `migration_state_inconsistent`, `foreign_keys_unavailable`,
`invalid_config`, `invalid_stored_value`, `out_of_order_observation`, `lock_poisoned`,
`no_sources_configured`, `collection_already_running`, `match_refused`. The interface special-cases
`no_sources_configured`, `collection_already_running`, `match_refused`, `invalid_config`
(`ERROR_KIND` in `api.ts`). `ModelError` adds `InvalidOutcome` (checked by
`SourceOutcome::validate`). `SourceError` (sources layer) is separate: `blocked`, `not_found`,
`rate_limited`, `timeout`, `network`, `server_error`, `unexpected_status`, `too_large`,
`disallowed`, `too_many_redirects`, `unparseable`, `cancelled`, `other`; only `timeout`, `network`
and `server_error` are retryable.

---

## 8. Reference by area

### 8.1 Parser and the price-field hint (`src/parser`, `models/evidence.rs`)

* `PARSER_VERSION = "br_real_estate/2"` (was `/1`). Public API: `parse_listing_text`,
  `parse_structured_text(StructuredText { field, text })`,
  `parse_listing_with_price_hint(description, &PriceFieldHint { field, confidence })`.
* `Evidence` gained an optional `field` (the source field the span indexes into); old stored JSON
  without it still deserialises. `WarningKind::PriceFromStructuredField` was added.
* **Hint rule** (`headline_price_mentions`): it applies only to a price field holding **exactly one**
  amount with no label of its own. An explicit label in the field (`ágio`, `/mês`, `valor total`...)
  is stronger and is left as parsed. A lone unlabelled amount becomes a `FullPrice` at the
  **source's calibrated confidence**, unless it is below R$ 10.000 (`MIN_PROMOTABLE_FULL_PRICE_CENTAVOS`)
  or not larger than an ágio / down payment / instalment / balance labelled in the description
  (then it stays unclassified and gets the `conflicting_labels` warning). Hint prices are placed
  **before** the description's prices; characteristics come from the description only.
* Pre-existing behaviour kept: an unlabelled amount larger than every labelled financing component
  becomes a *low-confidence* full-price candidate (never passes the full-price filter);
  `MIN_COMPARABLE_CONFIDENCE = Medium`; a full price qualifies only if it is the single
  non-conflicting full price at that confidence.
* **The parser never extracts a property type** (`property_type` is hard-coded `Fact::Unknown`,
  `br_real_estate.rs`). This is a known limitation (section 12).
* Tests: 85 in `parser::br_real_estate` (66 in `tests.rs` + 19 in `hint_tests.rs`), plus `number`
  5 and `text` 6 (those two use inline test modules).

### 8.2 Schema v2 (`db/migrations/0002_collection_and_audit.sql`) and migration mechanics

Additive only (v1 untouched). `MIGRATIONS` is an ordered array (v1 = `schema.sql`, v2 = 0002);
each runs in its own transaction that also bumps `PRAGMA user_version` and appends to
`schema_migrations`; plain `CREATE` (never `IF NOT EXISTS`); a database newer than the build is
refused (`schema_too_new`). `LATEST_SCHEMA_VERSION = 2`.

* `listing_links` gains `source_fields_json` (raw structured fields exactly as the adapter gave
  them, so evidence spans naming a field other than the text can always be resolved),
  `status_changed_at`, `status_reason`.
* `collection_runs` (`running|completed|cancelled|failed`; `finished_at` iff not running).
* `source_run_outcomes` (`results|successful_empty|failed|unreliable`, `complete` 0/1, counts,
  `warnings_json`, `error_kind/detail`; CHECKs: `successful_empty` requires `complete = 1` and
  zero discovered; `failed` requires an `error_kind`).
* `property_merge_events`: append-only (update/delete triggers abort); `merged|reverted`,
  surviving/absorbed property, actor, rule version, note, `snapshot_json`.
* Triggers `properties_merge_target_must_be_root_on_update/_insert`: a property can only point at a
  root, so cycles are impossible and chains are allowed.
* Views: `property_groups(root_id, member_id)` (recursive: a root and everything folded into it)
  and `property_group_status(root_id, effective_status, status_conflict)` (favorite > hidden > new;
  conflict = both favorite and hidden among members).
* Schema v1 tables used by Phase 2: `properties` (soft merge columns, `user_status`,
  `match_review_status`, canonical columns, `canonical_provenance_json`), `listing_links`,
  `property_images` (hash stored as signed integer with the same bit pattern, algorithm name,
  `processing_status pending|hashed|embedded|failed|skipped`), `property_matches` (CHECKs:
  `listing_a_id < listing_b_id`; an automatic acceptance requires `high`; a recorded merge is live
  (accepted) or reverted (rejected + `reverted_at`)), `listing_price_history` (append-only).

### 8.3 Repositories (`src/db`)

* **`Database`**: `open(path)`, `open_in_memory()`, `with_conn(|&mut Connection| ..)`,
  `schema_version()`, `foreign_keys_enabled()`. Pragmas: `busy_timeout` 5 s, `foreign_keys` ON
  (verified, else `foreign_keys_unavailable`), file databases use WAL and `synchronous = NORMAL`.
  Timestamps are UTC ISO-8601 with milliseconds (`format_timestamp` / `parse_timestamp`).
* **`listings`**: `upsert_listing` runs in one `IMMEDIATE` transaction (content, price columns,
  price observation, canonical refresh). Policies: a **failed extraction never overwrites** content,
  price or `last_verified_at`; a page with **no price** keeps the last known price; a page whose
  full prices **contradict** clears the current price columns but keeps history; `first_seen_at`
  never moves, `last_seen_at`/`last_scraped_at` never go backwards; nothing is deleted. A new
  listing creates a property candidate. `mark_unavailable` only on explicit evidence (records time
  and reason). `find_listing`, `get_listing`, `listings_of_property`; `StoredListing`.
* **`price_history`**: `record_observation(_in)`; "same price" = same amount **and**
  classification (confidence/provenance/time alone are not a change); an older observation than
  the latest is refused (`out_of_order_observation`); rows are never updated or deleted.
* **`properties`**: `refresh_canonical(_chain)` recomputes bedrooms/suites/parking/areas from the
  group's listings; values listings disagree on stay unknown with the disagreement in
  `canonical_provenance_json`; rows are written only when something changed. `root_of`,
  `effective_status`, `set_user_status` (applies to the **whole group**; hiding keeps an existing
  `hidden_at`), `get_property_view`, `list_properties`.
  `list_properties(PropertyQuery)`: roots only; hidden excluded unless `include_hidden`;
  `favorites_only`; `region_slug` matches the region and everything below it; `full_price_only`
  and the price range consider only **established** prices (classification full price, confidence
  >= Medium, listing active); a property whose listings all failed extraction is hidden; ordered by
  most recent `last_seen_at` then id; page size default 50, capped at 200. `PropertyView` carries
  every listing of the group, `comparable_prices`, `price_spread` (min, max), canonical values
  and provenance, `match_review_status`.
* **`runs`**: `start_run`, `finish_run` (only if still running), `record_outcome`, `get_run`,
  `list_runs` (newest first), `latest_outcomes_by_source`, `recent_problems` (failed, unreliable or
  incomplete), `fail_interrupted_runs` (startup: a run cannot be running without a process).
* **`regions`**: `data/regions.json` (embedded as `DEFAULT_SEED_JSON`) seeds **13 regions**: 1
  state, 3 municipalities (Goiânia, Aparecida de Goiânia, Hidrolândia), 8 neighbourhoods, 1
  corridor. `parse_seed` validates (version, unknown fields, slug format, state codes, parents,
  cycles, depth, unique names within a scope, alias rules, metadata object). `seed_regions` is
  idempotent, in one transaction, never deletes and never changes `enabled` of an existing row.
  `LocationIndex::resolve(text)` returns the most specific region the text names, or `None` when
  nothing is named, the match is ambiguous, or the text contradicts itself (for example a
  neighbourhood of another municipality); longer names claim text first ("Aparecida de Goiânia" is
  never also "Goiânia").
* **`images`**: `register_images(listing, urls, max_per_listing, at)` (only plain http(s) URLs,
  cap counts existing rows, `INSERT OR IGNORE`, positions continue), `pending_images(adapter_ids,
  limit)` (only those adapters; others cannot use up the limit), `count_pending_outside`,
  `record_hashed`, `record_final(Failed|Skipped, reason)`, `record_transient` (stays pending with
  the reason visible), `images_of_listing`, `hashes()` (current algorithm only). Adapter ids are
  bound as parameters; only `?N` placeholders are generated into the SQL text.

### 8.4 Source layer (`src/sources`)

* **Adapter contract** (`adapter.rs`): `SourceAdapter { id, platform, config, discover, fetch_listing }`
  with `BoxFuture` methods; `SearchRequest { region_slugs, min_price, max_price, max_listings }`
  (stored with the run); `Discovery { refs, complete, warnings }`; `RawListing` (url,
  source_listing_id, title, description, location_text, `price_text` = the platform's structured
  asking-price field verbatim, `extra_fields`, `image_urls`, `ExtractionReport`). `NotFound` must
  only be returned on explicit evidence the ad is gone; `Unparseable` when a page was fetched but
  cannot be read reliably.
* **`SourceConfig` defaults** (`SourceConfig::new(hosts)`, validated by `validate()`):

  | Field | Default | Valid range |
  |---|---|---|
  | `max_concurrency` | 2 | 1..=8 |
  | `min_request_interval` | 1 s | (spacing of request *starts*) |
  | `request_timeout` / `connect_timeout` | 20 s / 10 s | > 0 |
  | `max_retries` | 2 | <= 5 |
  | `backoff_base` / `backoff_max` | 500 ms / 8 s | base > 0 and <= max |
  | `max_response_bytes` | 2 MiB | 1 KiB..=64 MiB |
  | `max_image_bytes` | 8 MiB | 1 KiB..=64 MiB |
  | `max_images_per_listing` | 6 | 1..=30 |
  | `max_redirects` | 5 | <= 10 |
  | `cache_ttl` / `cache_max_entries` / `cache_max_bytes` | 600 s / 256 / 32 MiB | entries 1..=10000, bytes 1 KiB..=256 MiB when on |
  | `price_field_confidence` | `Low` | calibration result; must stay `Low` until measured |
  | `max_listings_per_run` | 200 | 1..=5000 |
  | `user_agent` | `LooseCanon/<version> (local desktop application)` | non-empty single line |
  | `allowed_hosts` / `ignored_url_params` | given / empty | bare host names only |

* **`Fetcher`** (one per source; `new(config, global_semaphore)`, `get`, `get_image`):
  host allow-list checked for the request **and every redirect** (also refuses an https to http
  downgrade and too many redirects); per-source and global concurrency; spacing of request starts
  under a short lock; timeouts; retries for `timeout`/`network`/`server_error` only, with
  exponential backoff `base * 2^(n-1)` capped at `backoff_max`; status mapping: 2xx ok,
  401/403/451 `blocked`, 404/410 `not_found`, 408 `timeout`, 429 `rate_limited` (reads
  `Retry-After` seconds, clamped to 3600), 5xx `server_error`, others `unexpected_status`;
  size limit enforced by `Content-Length` pre-check **and** by counting decoded bytes as chunks
  arrive (compressed bombs stop); bounded in-memory cache for pages only (images are never cached
  in memory); error messages never contain URLs. `get_image` uses `max_image_bytes`.
* **URL identity** (`url_norm.rs`): identity of an ad is `(adapter_id, normalized_url)`. Normalising
  removes fragments, `utm_*`, `fbclid`, `gclid`, `dclid`, `msclkid`, `yclid`, `mc_cid`, `mc_eid`,
  `_ga`, `igshid` and per-source extras, sorts parameters, drops default ports and lower-cases the
  host; path text (including a trailing slash) is never rewritten and every other parameter is
  kept, so two different ads are never merged. Only http(s), no credentials, <= 2048 characters.
* **Normalisation** (`normalize.rs`): title and description are joined with a newline into one
  text (evidence field `"text"`); the structured price goes through the hint (field `"price"`,
  confidence from the source config); `extraction_status` is `Partial` when fields are missing, the
  adapter warned, or no price was found (reasons joined in `extraction_error`), else `Ok`;
  `last_verified_at` is set **only** for a clean read; the raw fields are kept as
  `source_fields_json`. Fails `Unparseable` when there is no text and no price, `Disallowed` when
  the listing's host is outside the allow-list.
* **Pipeline** (`pipeline.rs`, `run_collection(deps, adapters, request, cancel, progress)`): errors
  **before creating a run** when there are no adapters (`no_sources_configured`) or ids collide.
  Per source: discover; `plan_work` (dedupe by normalised URL, cap = min(request, config), at
  least 1; unusable references are skipped with a warning); fetch each listing under the
  per-source permit; `NotFound` marks an already-stored listing unavailable (nothing created for
  one never stored); `Unparseable` records a failed read on an existing listing without touching
  its content; `RateLimited` stops the source; cancellation stops it; stored through
  `upsert_listing` and **image URLs are queued** (`register_images`, capped, same DB closure).
  Outcome rules (`Tally`): `complete` = discovery complete, not truncated, not rate limited, not
  cancelled, nothing failed or unreadable. With nothing stored and problems: only unreadable pages
  -> `Unreliable`; otherwise `Failed` with the most frequent error kind (ties: alphabetical) or
  `rate_limited` / `cancelled`. With listings stored but failed + unreadable more than half of
  those discovered -> `Unreliable`; else `Results`. Nothing to fetch: references skipped ->
  `Unreliable`; discovery complete -> `SuccessfulEmpty`; incomplete discovery -> `Failed`
  (`incomplete_discovery`). The run status is `Cancelled` if the token was cancelled else
  `Completed`; a failing source is an *outcome*, never a failed run. `ProgressEvent { run_id,
  adapter_id, phase (discovering|fetching|finished), discovered, processed, stored }`.
* **Registry**: `sources::registered_adapters()` returns an **empty** `Vec`; a test asserts it.
  Test adapters exist only in test modules (`sources/pipeline/tests.rs`, `service/tests.rs`).
* **Test server** (`testserver.rs`, test-only): real TCP sockets, scripted replies, counters for
  hits, concurrency and request start times, optional per-reply delay.

### 8.5 Images (`src/images`)

* **Decode** (`decode.rs`, `hash_image(bytes, &ImageLimits)`): defaults 8 MiB, 10 000 x 10 000,
  40 000 000 pixels; format by **content** (JPEG, PNG, WebP only); dimensions read from the header
  before any pixel buffer; decode allocation capped; a JPEG without an end-of-image marker in its
  last 4096 bytes is `corrupt` (the decoder is lenient and would hash a truncated picture). Error
  kinds: `empty`, `too_large`, `unsupported_format`, `too_many_pixels`, `corrupt`.
* **Hash** (`phash.rs`, algorithm `dct64-v1`): composite onto white, integer luma
  `(299R + 587G + 114B)/1000`, area-average to 32x32 using **exact integer overlap weights**,
  2-D DCT-II keeping the 8x8 lowest frequencies, bit = coefficient > median of the 63 AC terms,
  most significant bit first. A golden test pins the value `0xee4c_b15b_0f64_c4b1` for the test
  scene with seed 7. Stored as the signed integer with the same bit pattern. If the algorithm ever
  changes, bump the algorithm name: hashes of different algorithms are never compared.
* **Index** (`index.rs`, `near_pairs(entries, max_distance)`): multi-index hashing with
  `max_distance + 1` chunks (supported up to 16); a pair is reported once, in the first chunk where
  the two agree; buckets above 256 are skipped and counted. Used only to **propose** candidates.
* **Cache** (`cache.rs`): files at `images/<listing id>/<position>.<ext>` under the app cache
  directory (Tauri `app_cache_dir`), written atomically (temp file, rename); the database stores
  only the relative path; `resolve` re-validates a stored path (no `\`, `:`, NUL, leading `/`,
  `..`, empty parts).
* **Job** (`pipeline.rs`, `process_pending_images(deps, sources, cache_root, batch, cancel)`):
  loads pending images **only for the given adapters**, builds every `Fetcher` first (invalid
  config is an error), then per source takes a permit **before** spawning each image task (so
  `max_concurrency = 1` is strictly sequential). Outcomes: hashed; `Failed` with a reason
  (`corrupt`, `unsupported_format`, `too_large`, `too_many_pixels`, `not_found`, `blocked`,
  `unexpected_status`, `too_many_redirects`, `decoder_crashed`); `Skipped` (`disallowed`: host
  outside the allow-list, nothing requested); transient (`timeout`, `network`, `server_error`,
  `rate_limited`, `cache_write_failed`) stays `pending` with the reason visible. A `blocked` or
  `rate_limited` response ends that source's work for the call (the rest stays pending, counted
  as `not_attempted`). Decoding runs on the blocking pool under the CPU semaphore; a decoder
  panic fails that one image (`decoder_crashed`); a database failure while saving is counted in
  `internal_errors`. `ImageReport { attempted, hashed, failed, skipped, retry_later,
  unavailable_source, not_attempted, internal_errors, cancelled }`. `run()` takes the decoder as a
  parameter only so the panic case can be tested.

### 8.6 Matching (`src/matching`, `docs/matching.md`)

Summary here; the document `docs/matching.md` has the full tables.

* **`MatchConfig`** defaults (`validate()` rejects contradictory values): `image_distance` 5,
  `strong_image_pairs` 2, `common_hash_listings` 4, `area_match_permille` 20,
  `area_conflict_permille` 100, `price_close_permille` 50, `price_far_permille` 300,
  `text_min_tokens` 8, `text_min_jaccard_percent` 60, `max_block_size` 200. Rule version
  `match_rules/1` (`RULE_VERSION`). Integer arithmetic only; differences are measured relative
  to the **smaller** value.
* **Comparison** (`compare.rs`): signals (same municipality, same neighbourhood, same type,
  lot/built/usable area match, rooms match, price close, text similar, near-duplicate images),
  conflicts (hard: municipality, type, area of the same kind, bedrooms, suites, parking; soft:
  neighbourhood, price), unknown fields, one-to-one image pairs (greedy, closest first), notes.
  Area kinds are never mixed; house / semi-detached / sobrado are compatible types; `other` is
  ignored; neighbourhood names only count with a known equal municipality; a listing that
  contradicts itself is reported as such. Evidence never stores the words of the ad.
* **Categories**: different municipality -> low; hard conflict -> possible only with >= 2 image
  pairs else low; soft conflict -> possible with >= 1 pair else low; `high` needs >= 2 pairs, same
  municipality **and** neighbourhood, an area or rooms match, no conflict and **no apartment** on
  either side; otherwise possible with >= 1 pair; without images possible only for same
  neighbourhood + same non-apartment type + area match + rooms match + similar text; else low.
* **Candidates** (`candidates.rs`): image near-pairs (hashes shared by >= 4 listings ignored) and
  same municipality + neighbourhood blocks (> 200 skipped); never the same listing, the same
  merge group or an already-decided pair.
* **Loader** (`load.rs`): every listing with a property, any status; place from the property row
  (folded); facts from `extracted_facts_json` (conflicting facts become unknown and are listed as
  `contradictory`); the price counts only when full price with confidence >= Medium; terms are
  folded words of >= 4 letters, letters only; images with the current hash algorithm.
* **Run** (`run.rs`, `run_matching(deps, config, cancel)`): validates the config, loads inputs and
  decided pairs, compares on the blocking pool (no DB lock), stores proposals in transactions of
  200 with a cancellation check between batches. `MatchRunReport` (camelCase): `ruleVersion`,
  `listings`, `candidatePairs`, `skippedDecided`, `compared`, `high`, `possible`, `lowStored`,
  `lowDiscarded`, `created`, `updated`, `unchanged`, `decisionsKept`, `oversizedBlocks`,
  `oversizedImageBuckets`, `commonHashes`, `cancelled`. A pass that is cancelled up front does not
  even load the library.
* **Evaluation** (`evaluation.rs`): `evaluate(&LabeledSet, &MatchConfig)` runs the application's own
  candidate generation and comparison and reports false merges (different properties rated high),
  false review items, missed duplicates (low **or never proposed**), duplicates not high, per
  scenario counts, every pair's outcome and limitations (provenance caveat, "fewer than 30 pairs"
  warning, rule-of-three bound when there are zero false merges). Inconsistent sets (duplicate
  ids, unknown listings, self pairs, repeated pairs) are refused. `LabeledSet` loads from JSON
  (`group_root` may be omitted; every listing is its own group). **Synthetic regression set**
  (14 pairs, in `matching/evaluation/tests.rs`): same-property 2 high / 2 possible / 1 low / 1 never
  proposed; different-property 1 high / 3 possible / 3 low / 1 never proposed; false merges 1 of 8
  (twin houses sharing marketing photos); false review items 4 of 8; missed duplicates 2 of 6.
  These describe constructed cases only.

### 8.7 Merge, reject, undo and match storage (`db/matches.rs`)

* `store_candidate(NewMatch{ listing_a < listing_b, category, min_image_distance, evidence, inputs })`
  -> `Created | Updated | Unchanged | DecisionKept`. High/possible become `review_required`, low
  becomes `candidate`; a pair that is `accepted` or `rejected` is never overwritten (the evidence
  the user judged is kept). `inputs` is `metadata_signals_json` (a summary of what was compared; no
  words, no ids).
* `accept_match` -> `Merged{surviving, absorbed} | AlreadyMerged | AlreadySameGroup`. Survivor =
  smaller root id. Sets the absorbed root's `merged_into_property_id`, records the decision (user),
  appends a `merged` event with a snapshot of both sides' status (and the group status after),
  refreshes canonical values and review status. A rejected pair may be accepted later.
* `reject_match` -> `Rejected | AlreadyRejected`; refuses while a merge is live.
* `undo_merge` -> `Reverted{surviving, absorbed} | AlreadyReverted`; clears the pointer, records
  `rejected` + `reverted_at` (keeps the merge ids as a record), appends `reverted`. It **refuses
  and changes nothing** if the absorbed property no longer points at the recorded survivor. Chains
  can be undone in any order (tested).
* `MatchRefused` is raised for: unknown match, listings given out of order, a live merge being
  rejected, nothing to undo, inconsistent pointers, an overlong note (command layer).
* `refresh_review_status`: a group root is `review_required` while a pair touching exactly one side
  of the group awaits the user, `reviewed` after any decision touching it, else `none`.
* Reading: `get_match`, `list_matches(MatchQuery{ status, actionable_only, limit, offset })`
  ordered high, possible, low then id (`actionable_only` excludes pairs already in one group),
  `merge_events(match_id)`, `decided_pairs()`. `MatchView` carries both `ListingBrief`s (url,
  title, headline price, current `group_root`), the stored evidence, distance, timestamps and the
  merge record.
* Every operation is a single `IMMEDIATE` transaction; tests prove a failure in the audit insert
  rolls everything back (both accept and undo).

### 8.8 Application service (`src/service/mod.rs`)

* `AppService::new(db, adapters, image_cache_dir, clock)` -> `Arc<AppService>`.
* `startup()`: seeds regions (idempotent) and fails runs left `running`; returns `StartupReport
  { regions: SeedReport, interrupted_runs }`.
* `sources()`: registered adapters with their latest outcome (`None` = never ran, which is not
  the same as failed).
* `start_collection(request, progress?, on_finished?)`: refuses with `no_sources_configured` (no
  run is created) or `collection_already_running`; otherwise spawns the job and returns at once.
  The job: `run_collection`, then `process_pending_images` (batch 200), then `run_matching`; the
  result is `CollectionFinished { runId, status, outcomes, error, followUpErrors, images,
  matching }` (`error` = the collection itself could not run; `followUpErrors` = image or matching
  problems after a successful collection). Must be called inside a Tokio runtime.
* `cancel_collection()`, `collection_running()`, `blocking(f)` (DB closure on the blocking pool),
  `offload(f)` (any `FnOnce(&AppService)` on the blocking pool), `run_matching_pass()`.
* `lib.rs::run()` builds the service in Tauri's `setup` (data dir -> database, cache dir -> image
  cache, `registered_adapters()`, `chrono::Utc::now` clock), runs `startup()`, then
  `app.manage(service)` (managed type `Arc<AppService>`).

### 8.9 Commands, events and the TypeScript contract (`src/commands`, `src/lib/api.ts`)

18 commands, all registered in `lib.rs` (Rust parameter names are camelCase on the JS side):

| Command | Arguments | Returns |
|---|---|---|
| `get_app_status` | - | `AppStatus` |
| `list_sources` | - | `SourceInfo[]` |
| `start_collection` | `args: CollectionRequest` | `void` (errors: `no_sources_configured`, `collection_already_running`) |
| `cancel_collection` | - | `boolean` (was one running) |
| `is_collection_running` | - | `boolean` |
| `list_collection_runs` | `limit?` (default 20, 1..=200) | `RunRecord[]` newest first |
| `get_collection_run` | `runId` | `RunSummary \| null` |
| `list_source_problems` | `limit?` | `StoredOutcome[]` |
| `list_regions` | `onlyEnabled?` (default true) | `RegionRow[]` |
| `list_properties` | `args: ListPropertiesArgs` | `Page<PropertyView>` |
| `set_property_status` | `propertyId`, `status` (`new\|favorite\|hidden`) | rows touched |
| `list_matches` | `args: ListMatchesArgs` | `Page<MatchView>` |
| `get_match` | `matchId` | `MatchView \| null` |
| `list_match_events` | `matchId` | `MergeEventRecord[]` |
| `accept_match` | `matchId`, `note?` | `AcceptOutcome` |
| `reject_match` | `matchId`, `note?` | `RejectOutcome` |
| `undo_merge` | `matchId`, `note?` | `UndoOutcome` |
| `run_matching_pass` | - | `MatchRunReport` |

* **Events:** `collection-progress` (`ProgressEvent`) and `collection-finished`
  (`CollectionFinished`). Both are also recoverable from the saved runs.
* **Argument validation:** region slugs `[a-z0-9-]{1,100}`, at most 50 per request; prices must be
  non-negative and min <= max; `maxListings` >= 1; notes are trimmed, an empty note is no note, and
  a note over 500 characters is refused (not cut); request/filter structs reject unknown JSON
  fields (`deny_unknown_fields`); page sizes are capped.
* **Outcome JSON:** tagged with `kind` (`merged`, `already_merged`, `already_same_group`,
  `rejected`, `already_rejected`, `reverted`, `already_reverted`); fields camelCase.
* **Conventions:** DTOs camelCase; enums are the stored snake_case text; money = centavos
  (number); areas = hundredths of m² (number); timestamps = ISO strings; `priceSpread` is a
  two-number array; stored documents keep snake_case. `api.ts` documents each interface.
* **Contract tests** (`commands/contract_tests.rs`, 6 tests) serialise real DTOs built from a real
  database and assert the exact JSON key sets. The expected lists are copied from `api.ts`; if a
  DTO changes, update both together. (The TypeScript side is not machine-checked against Rust.)
* `capabilities/default.json` grants only `core:default`, which is sufficient for app-defined
  commands and for listening to events.

### 8.10 Frontend (`src/`)

`main.tsx` (unchanged), `App.tsx` (status + `Diagnostics`), `Diagnostics.tsx` (sources with their
latest outcome and an honest message when none are configured; start / cancel / "find duplicates"
buttons; recent runs; saved properties with the full-price-only filter; the duplicate review queue
with accept / reject / undo; shows `collection-finished` results; maps the known error kinds to
Portuguese messages), `lib/api.ts` (every type and function). UI text is pt-BR. There is no
frontend test framework: only `tsc --noEmit` and `vite build` check it, and the panel was not
exercised in the running app.

---

## 9. Test strategy and mutation checks

* **Red first.** New behaviour got failing tests against a stub (a `todo!()` or a deliberately
  wrong body) before the implementation; the red run was observed. Exceptions are listed in
  section 11 (small argument conversions and two fixtures were written together with their tests,
  then mutation-checked).
* **Mutation checks** (helper in 5.2): for each safety rule written in this phase the rule was
  removed and a test was shown to fail. 73 mutations, all caught:

  | Area | Mutations | Notes |
  |---|---|---|
  | Image job | 3 | refusal stops a source; decoder panic is a per-image failure; transient errors stay pending |
  | Comparison rule | 11 | strong-image threshold, apartment exclusion, hard-conflict handling, placeholder hashes, one-to-one pairs, neighbourhood-needs-municipality, area kinds, metadata-only text requirement, soft conflicts, different municipality, location requirement for `high` |
  | Candidate generation | 6 | merge group, placeholder hashes, oversized blocks, common-hash boundary, both-ways flags; **1 equivalent mutant kept** (removing the explicit "same listing" guard is invisible because a listing always shares its own group root) |
  | Persistence, merge, undo | 14 | survivor choice, same-group, idempotency, pointer check, reject-while-merged, decided pairs kept, review status, canonical refresh after merge and undo, ordering, actionable filter |
  | Matching run | 7 | decided pairs, metadata-only low discarded, photo-derived low stored, cancellation between batches and up front, config validation, stored inputs contain no text |
  | Evaluation harness | 11 | each validation, group-root normalisation, missed/false accounting, sorted ids, config validation, uses the real rule |
  | Command arguments | 13 | price ordering, zero limits, slug validation, region count, history cap, filters reaching the query, note length/trim, status/actionable mapping |
  | Service | 6 | busy guard, flag cleared after a job, no-source refusal, startup recovery, cancel, callback runs after the service is free |
  | Image queuing in the pipeline | 2 | queuing, per-listing cap |

  Mutation passes exposed weak tests that were then strengthened: the "older root survives" case
  (listing `a` in the newer group), a run cancelled up front must not even load the library
  (`listings == 0`), the callback must be able to start the next collection, ids in the evaluation
  report must be sorted regardless of input order.
* **Flakiness:** timing-sensitive image tests were run 8 times and the source tests 6 times
  without a failure. Timing assertions use wide margins (cancel within 1 s of a 5 s delay).
* **End to end:** `service::tests` runs a scripted adapter through collection, image job and
  matching in a real in-memory database; this is what found the image-queuing defect.
* **Contract tests** (section 8.9) pin the JSON keys that cross the Rust/TypeScript boundary.
* Enum/SQL agreement tests (the macro `string_enum!` provides `ALL`) prove the Rust enums and the
  SQL `CHECK` lists accept exactly the same values; extend them when adding a value.

---

## 10. Verification evidence (commands actually executed on the final tree)

| Command | Result |
|---|---|
| `cargo fmt --check` | clean |
| `cargo clippy --all-targets --locked -- -D warnings` | clean (pedantic) |
| `cargo test --locked` | **545 passed, 0 failed** (library tests; the binary and doc-test targets have none) |
| `npm run typecheck` | pass |
| `npm run build` | pass (`tsc --noEmit` then `vite build`) |
| Personal-data / machine-path search over tracked and untracked text files | nothing found |
| Cleanliness | no stray files from the tooling; `dist`, `node_modules`, `src-tauri/target` are git-ignored |

### 10.1 Not verified (do not assume)

* The desktop window, the events and the diagnostics panel were **never run**; `get_app_status`
  and the new commands were exercised only through their service and argument layers.
* No live network traffic: source behaviour is tested with a local TCP server and scripted
  adapters. Real markup and real responses have never been seen by this code.
* MSRV 1.90 (declared) was not built; all work used 1.99.
* Performance beyond a 4000-listing candidate scan; `list_properties` runs one extra query set per
  returned property (N+1, acceptable at a page size of at most 200).
* Windows packaging (`tauri build` without `--debug --no-bundle`) under the new bundle identifier.
* Cross-platform behaviour (only Windows was used).

---

## 11. Problems, mistakes and corrections (candid log)

Product defects found and fixed:

1. **Image queue starvation** (found while reading before implementing the image job):
   `pending_images` had no adapter filter, so images of unregistered adapters could fill a batch
   and starve the rest. Fixed test-first (`pending_images(conn, adapter_ids, limit)`,
   `count_pending_outside`; 4 new DB tests plus a job-level test).
2. **The collection pipeline never queued images** (found by `service::tests` after every stage
   passed alone): `normalize` carried `image_urls` but nothing called `register_images`, so the
   image and matching chain would have had no input in production. Fixed test-first in
   `sources/pipeline.rs` (5 tests: queued, no duplicates on re-run, per-listing cap, plain
   http(s) only, none without images).
3. **Identical flats would have rated `high`** (found while designing the evaluation set):
   fixed with the apartment cap (D5) and a test.
4. **A weak mutation-test target:** a test named for "older property survives whichever listing is
   a" did not exercise listing `a` being in the newer group; a mutant survived; a real test was added.

Findings that are limitations rather than defects: the parser never extracts a property type
(section 12); the synthetic evaluation contains a false `high` (twin houses with shared marketing
photos).

Process slips and how they were handled:

* A mutation helper aborted on an unmatched pattern and **left a mutated source file in the tree**;
  restored from backup, verified byte-identical, and the helper was rewritten to validate all
  patterns first and always restore (5.2).
* I wrote a nonsense expression as the last assertion of one chain test; noticed on re-reading,
  traced the scenario by hand and replaced it with the correct expectations
  (`merged_into` of the absorbed properties and the group members).
* A write of `lib.rs` was refused because I had edited the file since reading it; a build then
  ran against the **old** wiring. Noticed because the command handlers compiled without being
  registered; the file was re-read and rewritten.
* Edit scripts with line-number guards were off by one three times; the guards stopped them before
  any change.
* Small command-argument conversions, two loader fixtures, and the contract tests were written
  together with their tests rather than strictly red-first (conversions were mutation-checked).
  Earlier in the session `SourceOutcome::validate` and the cache were also written with their tests
  and mutation-checked.
* Two documentation inaccuracies were caught and corrected before finishing: a statement that a
  labelled evaluation set holds "no personal data" (its terms are real words; the document now
  requires replacing them with opaque ids and says no export tool exists), and a status-report row
  that listed fetcher limits as mutation-checked in this pass (they were covered by their own
  red-first tests).
* Earlier-in-session issues (from the compaction summary): `Result::unwrap` ambiguity through
  `use super::*` (use a closure or `std::result::Result`); `record_price` called twice for an
  existing listing masked a `Recorded` outcome (now once); a `let Ok(..) = select! {..} else` syntax
  error (split into two statements); a redirect-policy borrow error (clone the host first); the
  JPEG decoder accepting truncated files (added the end-of-image check); a PNG bomb test needing an
  `IDAT` chunk to pass header parsing; placeholder hacks removed; `python3 -` hanging twice.
* A permission classifier once denied a command that referenced an API-key environment variable
  (not worked around) and once returned no verdict (retried successfully).

---

## 12. Known limitations and risks

* **The parser never extracts a property type.** `PropertyView.propertyType` is therefore always
  `null` for parsed listings, the "same type" signal and the metadata-only `possible` path cannot
  fire, and the apartment cap cannot apply to parsed listings. This errs toward keeping listings
  apart, but a type extractor (with adversarial tests such as "casa de praia", "apartamento" in
  prose) would make matching meaningfully better. It was not approved, so it was not added.
* **Matching has only been evaluated on a synthetic set.** Its single false `high` shows the rule
  is not safe for automatic merging; keep merging manual.
* Text similarity is a weak signal (ads built from one template look alike).
* Matching loads every listing into memory and re-compares all candidates on each pass.
* Cached image files are never pruned from disk.
* `list_properties` is N+1 per page (one view query set per returned property).
* The price-field hint is inert until a source is calibrated (confidence `Low`).
* Emit failures for Tauri events are ignored (`let _ =`); the saved runs are the source of truth.
* There is no logging or tracing yet.
* The frontend has no automated tests and was not run.
* `docs/phase-2-design.md` (the user's file) still says "no implementation has begun".
* Git reports `Cargo.lock` LF will become CRLF (line-ending setting); not changed.
* The bundle identifier names the app-data folder: changing it again after real data exists would
  move the data directory, so treat `com.polariscompany.loosecanon` as fixed once users have data.

---

## 13. What remains and the decisions needed

### 13.1 To complete Phase 2

* **Real source adapters**, registered in `sources::registered_adapters()`. Each one implements
  `SourceAdapter` with its own `SourceConfig`; `price_field_confidence` must start `Low`; add its
  tests (a local-server test is the established pattern); introduce an HTML-parsing dependency
  only if that adapter needs one; update the registry test together with the registry.
* **A labelled sample of real advertisements**, to (a) calibrate the price-field hint per source
  (precision by source and confidence band, with sample sizes, errors and uncertainty, using
  separate examples for tuning and for reporting; under 60 % certainty stays `Low`) and (b)
  evaluate matching with `matching::evaluation` (format and protocol in `docs/matching.md`;
  an export command for sanitised comparison inputs does not exist yet).
* Only then can automatic merging be reconsidered, with a narrowly specified multiple-signal rule
  that a **real** report supports. Not before.

### 13.2 Decisions for the owner

1. Confirm the merged-group status rule (D3).
2. Confirm the apartment cap at `possible` (D5).
3. Whether to add property-type extraction to the parser (section 12).
4. When (and how) to commit; a suggested split is below.
5. Explicit approval to start Phase 3. It has not been given.

### 13.3 Suggested commit split (only if the user asks for a commit)

1. Parser hint (parser, evidence, `price.rs`, hint tests). 2. Schema v2 and repositories.
3. Source layer. 4. Image layer. 5. Matching and merge persistence. 6. Service, commands,
frontend contract and diagnostics. 7. Documentation (`docs/matching.md`,
`docs/phase-2-alignment-review.md`, README, handoffs). Run the three Rust gates and the
frontend build before each.

---

## 14. Conventions and working agreements

* Tests first; observe the red run; mutation-check safety rules; fix weak tests the mutations
  reveal.
* Run `cargo fmt` **then** `cargo clippy ... -D warnings`; never blanket-`allow` a lint (two
  narrow, justified attributes exist: `needless_pass_by_value` on Tauri commands and one
  `#[expect]` on a pixel-math test fixture; a table-driven evaluation test also carries a justified
  `#[expect(clippy::too_many_lines)]`).
* Migrations are append-only: never edit a shipped one; add `0003_*.sql`.
* Adding an enum value means changing the Rust enum **and** the SQL `CHECK`; the agreement tests
  must stay green.
* Bump `PARSER_VERSION` when parser behaviour changes, `RULE_VERSION` when the matching rule or
  its thresholds change, and the hash `ALGORITHM` name if the perceptual hash changes (hashes of
  different algorithms are never compared).
* Stored JSON documents keep their `snake_case` shape; returned DTOs are `camelCase`; update
  `src/lib/api.ts` and `commands/contract_tests.rs` together.
* No personal contact details or credentials in fixtures, tests or the repository. Evidence never
  stores the words of an advertisement.
* Do not commit unless asked. Do not change the bundle identifier without the user. UI strings
  are pt-BR.
* Windows: prefer script files over shell one-liners for anything with quotes or backslashes
  (section 5.1).

---

## 15. Suggested next steps for the incoming agent

1. Run the gates in section 1 and confirm 545 tests pass; check `git status` (all Phase 2 work is
   uncommitted).
2. Read `docs/phase-2-alignment-review.md` and `docs/matching.md`.
3. Ask the owner for the decisions in 13.2; do not start Phase 3.
4. If asked to commit, use the split in 13.3.
5. If asked to continue Phase 2 engineering without new inputs, the useful, low-risk work is: an
   export command for sanitised matching inputs; a property-type extractor *if approved*; logging;
   an on-disk image cache pruning policy; a real manual run of the desktop app to exercise the
   diagnostics panel and events.

---

## 16. Glossary (terms added in Phase 2; pt-BR real-estate terms are in v1)

* **Merge group**: a root property plus everything soft-merged into it (`property_groups`).
* **Root / survivor / absorbed**: the group's top property; the root that stays after a merge; the
  root folded into it.
* **Soft merge**: `merged_into_property_id` set on the absorbed root; no listing moves.
* **Candidate**: a pair proposed for comparison; not a match.
* **Evidence**: signals, conflicts, unknowns, image pairs and notes stored with a proposal.
* **Established price**: a listing's headline price when it is a full price with confidence at or
  above Medium and the listing is active; the only price the full-price filter and the price range
  consider.
* **Outcome**: one source's result in a run: results, successful empty, failed or unreliable, plus
  a `complete` flag.
* **pHash / Hamming distance**: 64-bit perceptual hash and the number of differing bits; used only
  to propose candidates.
* **Review queue**: pairs with status `review_required` whose listings are not yet one property.

---

## Appendix A — Test inventory (545)

Counts per module (library tests; `cargo test --lib -- --list`). Phase 1 modules are inventoried
in v1 and were extended where noted.

| Module | Tests | What it covers |
|---|---|---|
| `commands` (`tests`) | 5 | status DTO, error JSON, distinct error kinds, slug validation |
| `commands::collection` | 6 | request conversion (exact money, camelCase), refusals (price order, zero limit, slugs, region count), equal bounds, history limits |
| `commands::properties` | 5 | default query, every filter reaches the query, page cap, malformed filters, status text |
| `commands::matches` | 5 | default listing, review-queue arguments, note trimming and limit, outcome JSON shapes |
| `commands::contract_tests` | 6 | exact JSON keys of every DTO from real values |
| `service::tests` | 8 | startup, no-source refusal, background run with images and matching, single run + cancel, cancel with nothing running, sources listing, camelCase, callback runs after the service is free |
| `sources::cancel` / `config` / `error` | 4 / 4 / 2 | token behaviour; config defaults and every bound; error kinds and retryability |
| `sources::fetch` | 23 | allow-list incl. redirects, concurrency, spacing, timeouts, retries and backoff, status mapping, size limits incl. compressed, cache, cancellation, image limits (own larger limit, never cached) |
| `sources::normalize` | 13 | text joining, hint use, extraction status, source fields, host checks |
| `sources::url_norm` | 8 | tracking removal, parameter order, distinct ads stay distinct, schemes, credentials, length |
| `sources::pipeline` | 31 | the four outcomes, partial/cancel/rate limit/panic, dedupe and caps, listing history and prices across runs, unavailable only on evidence, user status surviving runs, image queuing (5) |
| `sources` registry | 1 | the registry is empty |
| `images::phash` / `index` / `decode` / `cache` | 8 / 8 / 10 / 5 | golden hash, transformations, near-pair search vs brute force, bomb and truncation rejection, path safety |
| `images::pipeline` | 15 | hashed/cached/recorded, idempotent, each permanent failure, transient retry later, disallowed host, source stops on refusal or rate limit, unregistered sources, cancellation (up front and mid-download), batch bound, queue starvation, decoder panic, unwritable cache, near-duplicate hashes |
| `db::tests` | 37 | connection, pragmas, migrations (Phase 1 suite, extended) |
| `db::migration_0002_tests` | 9 | v1 -> v2 upgrade keeps rows, new tables, triggers, views, constraints |
| `db::listings` | 16 | upsert policies, price observation atomicity, unavailable on evidence |
| `db::properties` | 20 | canonical values, groups, status, filters, pagination |
| `db::runs` | 11 | run lifecycle, outcome validation, health queries, interrupted runs |
| `db::regions` | 16 | seed validation, idempotent seeding, resolver |
| `db::images` | 17 | registration, queue, outcomes, adapter filter, parameter binding |
| `db::matches` | 34 | enum/SQL agreement, storing, idempotency, decisions kept, merge/undo/reject, chains, cycles, status handling, atomic rollback, review status, listing and paging |
| `matching::compare` | 34 | every signal and conflict, thresholds at their boundaries, categories, symmetry, determinism, no text in evidence, apartment cap |
| `matching::candidates` | 11 | image and metadata candidates, placeholders, groups, oversized blocks, scale, determinism |
| `matching::load` | 9 | inputs from stored listings, price rules, contradictions, terms, images, groups |
| `matching::run` | 13 | proposals not merges, idempotence, updates, decided pairs, low handling, stored inputs, cancellation, validation |
| `matching::evaluation` | 12 | the synthetic set's exact report, scenario coverage, provenance caveats, rule of three, undefined rates, refusals, label independence, order independence, JSON round trip |
| `models::*` | 43 | area 4, characteristics 4, collection 5, confidence 2, evidence 2, image 1, listing 6, matching 2, money 4, price 11, region 2 |
| `parser::br_real_estate` / `number` / `text` | 85 / 5 / 6 | Phase 1 parser suite (66) plus 19 hint tests; number and text helpers |

Total: commands 27, service 8, sources 86, images 46, db 160, matching 79, models 43, parser 96 =
**545**.

## Appendix B — Examples of stored documents (snake_case, stable)

Match evidence (`property_matches.evidence_json`):

```json
{
  "rule_version": "match_rules/1",
  "signals": [{ "kind": "same_municipality", "detail": "goiania" }],
  "conflicts": [{ "kind": "price", "severity": "soft", "detail": "R$ 300.000,00 vs R$ 450.000,00" }],
  "unknown": ["bedrooms unknown in listing B"],
  "image_pairs": [{ "image_a": 101, "image_b": 201, "distance": 3 }],
  "notes": ["house type labels differ (house vs sobrado); treated as compatible"]
}
```

Compared inputs (`property_matches.metadata_signals_json`): `{ "a": {...}, "b": {...} }` where each
side holds `adapter`, `municipality`, `neighborhood`, `property_type`, `bedrooms`, `suites`,
`parking_spaces`, `lot_area_centi_m2`, `built_area_centi_m2`, `usable_area_centi_m2`,
`full_price_centavos`, `images` (count) and `description_terms` (count). No words, no ids.

Merge event snapshot (`property_merge_events.snapshot_json`): for a merge,
`{ "surviving": P, "absorbed": P, "after": { "effective_status": "...", "status_conflict": bool } }`;
for an undo, `after` is `{ "surviving": ..., "absorbed": ... }`, each a group-status object or
null. `P` is `{ "id", "user_status", "hidden_at", "merged_into", "group_members" }`.

A run's saved request (`collection_runs.request_json`, serialised `SearchRequest`):
`{ "region_slugs": [], "min_price": null, "max_price": null, "max_listings": null }`.

---

End of handoff v2.
