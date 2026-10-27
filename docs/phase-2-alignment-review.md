# Phase 2: alignment review and status report

Date: 2026-10-27. Reviewed document: `docs/phase-2-design.md` (left untouched, including its
"no implementation has begun" status line, which is now out of date).

## Verdict

**Phase 2 is INCOMPLETE and must not be called complete.** The plan's own completion criterion
needs *two permitted real source adapters*. There are **zero**: none of the four evaluated
candidates currently permits automated collection by a method this application can use
(`docs/sources/`). Following the plan's rule for that case, everything that does not depend on a
real source was built and tested, no mock was substituted for a real integration, and the gap is
reported here.

The plan was judged adequate and aligned with the original specification before implementation
began (it narrows rather than widens the spec: automatic merging off, no mock sources, explicit
completion criteria). Nothing below goes beyond it, except the items listed under "Decisions made
where the plan was silent", each of which the owner can reverse.

## Completion criteria, one by one

| Criterion (plan, "Validation and completion") | Status |
|---|---|
| Two permitted real adapters work reliably | **Not met: 0 of 2.** Dependency: written authorization / a permitted access method for a source, or named brokerages that permit it. |
| Per-source outcomes are accurate | Verified for the shared machinery with scripted adapters (4 outcomes + `complete`, partial/cancel/rate-limit/panic). **Not verified against any real source.** |
| History and user decisions survive repeated runs | Verified by tests: repeated runs, identical observations, unavailable ads, merge/undo cycles, statuses. |
| Matching and reversal are verified | Verified by tests and mutation checks (below). Matching was evaluated **only on a synthetic set**. |
| Price and match evaluations reported with limitations | Match: harness built, synthetic report in `docs/matching.md`. **Price calibration not done** (needs real labelled ads); the new price-field hint therefore stays at `Low`, i.e. never comparable. |

## What was built, against the plan

| Plan section | Implementation | Where |
|---|---|---|
| Source evaluation records | Dated records for 62imoveis, OLX, ZAP Imóveis and local brokerages, with request evidence and verbatim clauses. Decision: do not integrate three; brokerages need owner input. | `docs/sources/` |
| Adapter contract, bounded pipeline | `SourceAdapter` trait; `Fetcher` (host allow-list incl. redirects, per-source and global concurrency, request spacing, bounded retries for transient errors only, 403/429 never retried or bypassed, size caps on decoded bytes, bounded cache, cancellation everywhere); URL normalisation; shared normalisation; pipeline with four outcomes and a `complete` flag, DB work off the async workers, panic containment. | `src/sources/` |
| Price-field hint (user-approved) | Source-attributed structured hint, per-source calibrated confidence (default `Low`), financing labels and contradictions win, parser version bumped, adversarial tests including an ágio in a headline field. | `src/parser/` |
| Migrations, repositories | Additive migration v2; listings, properties, runs/outcomes, regions (seeded, idempotent), images, matches/merge audit. | `src/db/` |
| Region configuration | Validated seed for the three municipalities and example neighbourhoods; stable ids; resolver. | `data/regions.json`, `src/db/regions.rs` |
| Images | Bounded decode (JPEG/PNG/WebP, header dimension check, bomb rejection), own deterministic `dct64-v1` pHash with a golden test, multi-index near-pair search, cache with relative paths and traversal protection, image job with transient vs permanent outcomes. | `src/images/` |
| Matching | Candidate generation, comparison rule `match_rules/1`, stored evidence, review queue, atomic reversible merge with audit, evaluation harness. **No automatic merge.** | `src/matching/`, `src/db/matches.rs`, `docs/matching.md` |
| Commands, typed contracts, diagnostics | 18 typed commands, progress/finished events, `src/lib/api.ts`, a minimal diagnostics panel. Startup seeds regions and fails runs a dead process left running. With no source, starting a collection fails with `no_sources_configured` and records nothing. | `src/commands/`, `src/service/`, `src/` |

## Decisions made where the plan was silent (the owner may reverse any)

1. **Hint confidence defaults to `Low`** and is raised only by a per-source calibration backed by
   real labelled data (none exists).
2. **Merge target must be a root property**, enforced by triggers: no cycles, chains allowed. The
   property with the smaller id survives.
3. **Favorite beats hidden beats new for a merged group, with a conflict flag**; each member keeps
   its own stored status, so an undo restores each side. *Needs the owner's confirmation.*
4. **A pair already in one group records no decision** (`already_same_group`), so an undo of another
   merge can never leave it inconsistent.
5. **Apartments are capped at `possible`** (identical flats of one building share photos, address
   and floorplan). Found while designing the evaluation set.
6. **Metadata-only `low` results are not stored**; `low` results that came from photos are, as
   "considered and kept apart".
7. **`confidence_score` is left `NULL`**: no probability is invented for an uncalibrated rule.
8. **All thresholds are experimental configuration** with integer arithmetic only.
9. **Images are queued by the collection pipeline** (see "Defects found" below).
10. The source registry is an honestly empty function (`sources::registered_adapters`), guarded by
    a test that must be updated together with `docs/sources/` when a source is permitted.

## Defects and limitations found during the work (candid)

* **The collection pipeline never queued images.** Each stage was tested alone, but nothing
  registered a listing's image URLs, so the image and matching chain would have had no input in
  production. Caught by the end-to-end service test; fixed test-first at the pipeline level (5 tests).
* **The Phase 1 parser never extracts a property type** (hard-coded `Unknown`). Type-based matching
  evidence is therefore inert in practice. It errs towards keeping listings apart, but a type
  extractor would materially improve matching. It was not part of what was approved, so it was not
  added; it is listed as an owner decision.
* **The rule is not safe for automatic merging.** The synthetic set contains twin houses with
  shared marketing photos, which the rule rates `high` (1 false merge in 8 different-property
  pairs). This is the reason merging stays a user decision.
* A tooling slip: a mutation-testing helper once aborted leaving a mutated source file in the
  tree; it was restored from backup and verified byte-identical before anything else ran, and the
  helper now validates every pattern first and always restores.
* Small command-argument conversions were written together with their tests rather than strictly
  red-first (then mutation-checked). All larger logic was red-first.

## Verification evidence (commands actually run, final tree)

| Check | Result |
|---|---|
| `cargo fmt --check` | clean |
| `cargo clippy --all-targets --locked -- -D warnings` (pedantic) | clean |
| `cargo test --locked` | **545 passed, 0 failed** (Phase 1 had 149) |
| `npm run typecheck`, `npm run build` | pass |
| Rust/TypeScript contract | 6 tests serialise real DTOs and assert their exact JSON keys against `src/lib/api.ts` |
| Mutation checks | For the safety rules written in this phase (image job, matching rule, candidate generation, merge/undo/idempotency, matching run, evaluation accounting, command argument validation, the service's busy guard, image queuing) each rule was removed in turn and a test was shown to fail: 73 mutations in total. Survivors were investigated: one equivalent mutant (a redundant self-pair guard) was kept deliberately; the others exposed weak tests, which were strengthened. The earlier source-layer limits (fetcher, hint) were checked by their own red-first tests, not by this mutation pass. |
| Flakiness | Timing-sensitive image and source tests were run repeatedly (8 and 6 times) without failure. |

### Not verified

* **No real source was contacted** (no source is permitted), so nothing was tested against live
  traffic or real markup. Source behaviour is tested with a local TCP server and scripted adapters.
* **The desktop app was not launched** in this phase: the commands compile and are covered through
  their service and argument layers, but the Tauri window, events and diagnostics panel were not
  exercised by hand.
* The declared minimum Rust version (1.90) was not built; the toolchain used is 1.99.
* Performance was checked only up to a 4000-listing candidate scan.
* No Windows packaging (Phase 4).

## Decisions needed from the owner

1. **A source.** Written authorization from a portal, or named brokerage sites that permit
   automated access, or whether to build a user-assisted import (and whether it counts as one of
   the "two real sources"). This is the single dependency blocking completion.
2. **Reconcile "all of them"** (earlier answer) with the specification's "two first sources".
3. **Confirm the merged-group status rule** (decision 3 above).
4. **A labelled sample** of real, permitted advertisements, to calibrate the price hint and to
   evaluate matching. Someone must label pairs by hand; the harness and the format exist.
5. **Whether to add property-type extraction** to the parser (new finding).
6. ~~Bundle identifier~~ **Resolved:** the owner supplied `com.polariscompany.loosecanon`, now set in `src-tauri/tauri.conf.json`.

## Resuming

```powershell
npm install
npm run build
cd src-tauri
cargo fmt --check; cargo clippy --all-targets --locked -- -D warnings; cargo test --locked
```

Start with `docs/sources/README.md` (why there is no adapter), `docs/matching.md` (rule, merge
semantics, evaluation), then `src-tauri/src/service/mod.rs` (how a collection flows). A real
adapter implements `SourceAdapter`, is added to `sources::registered_adapters()` only after its
evaluation record permits it, and brings `scraper` (deliberately not yet a dependency) if it needs
HTML parsing.

**Next step requires the owner: stop here for an explicit decision on the points above and for
explicit Phase 3 approval. Phase 3 has not been started.**
