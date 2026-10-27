# Duplicate matching (Phase 2)

Status: implemented and tested; **evaluated only on a synthetic set**. Automatic merging does not
exist. This document describes what the code does today (`src-tauri/src/matching/`,
`src-tauri/src/db/matches.rs`). The design it implements is `docs/phase-2-design.md`.

## Principles

* A wrong merge hides a different property behind another one; a missed duplicate only leaves two
  ads side by side. Every choice favours keeping listings apart.
* A perceptual-hash match only **proposes** a pair. Similar price, neighbourhood, area or image
  alone never establishes identity.
* Missing and contradictory evidence is stored, never dropped.
* The rule has not been evaluated against real advertisements, so **every merge is a user
  decision**. There is no code path that merges on its own.
* All thresholds are experimental configuration (`MatchConfig`), integer arithmetic only (no
  floating point), versioned as `match_rules/1`. `confidence_score` is left `NULL` on purpose: the
  categories are not probabilities and no probability is invented.

## Pipeline

```
collection stores a listing ──> its image URLs are queued (property_images, status pending)
image job                   ──> download (bounded) → decode → 64-bit pHash → cache file → hashed
matching pass               ──> load listings → candidate pairs → compare → store proposals
user                        ──> review queue → accept (merge) / reject / undo
```

A finished collection runs the image job and then a matching pass automatically
(`service::AppService`); a pass can also be started on demand (`run_matching_pass`).

## Candidate generation (`matching/candidates.rs`)

Bounded, so a large library never costs a full pairwise scan:

* **Images:** multi-index hashing finds pairs of images within `image_distance` (default 5) bits.
  A hash seen on `common_hash_listings` (default 4) or more different listings (a "no photo"
  placeholder, a logo) is ignored and counted.
* **Metadata:** listings in the same municipality **and** neighbourhood (both known). Blocks larger
  than `max_block_size` (default 200) are skipped and counted, never expanded.
* Never proposed: a listing with itself, two listings already in one merge group, a pair the user
  already decided.

## Comparison (`matching/compare.rs`)

For a pair the rule collects *signals* (agreement), *conflicts* (disagreement), what is *unknown*,
the paired images, and notes. Details are short human-readable strings; the words of the ads are
never stored (only counts such as "6 of 10 distinct terms in common").

| Evidence | Signal | Conflict |
|---|---|---|
| Municipality | same | different: **hard** |
| Neighbourhood | same (only with a known, equal municipality) | different: soft |
| Property type | same | different: **hard** (house / semi-detached / sobrado are compatible; `other` is ignored) |
| Lot / built / usable area | same kind within 2 % | same kind more than 10 % apart: **hard** |
| Bedrooms / suites / parking | at least two known and all equal | any known pair differing: **hard** |
| Established full price | within 5 % | more than 30 % apart: soft |
| Description text | at least 8 distinct terms each and ≥ 60 % Jaccard | none |
| Images | one-to-one pairs within the distance | none |

Area kinds are never mixed: a lot area is only compared with a lot area. When the two sides state
different kinds, nothing is compared and a note says so. A listing that contradicts itself
(for example two different bedroom counts) is reported as such, not as merely "unknown".

### Categories

1. Different municipality → **low**.
2. Any hard conflict → **possible** only with strong images (at least 2 image pairs), else **low**.
3. Any soft conflict → **possible** with at least one image pair, else **low**.
4. **high** needs all of: ≥ 2 image pairs, same municipality **and** neighbourhood, a matching area
   or rooms signal, no conflict, and **neither listing an apartment** (identical flats of one
   building share photos, address and floorplan, so even all of that cannot tell two units apart).
5. Otherwise **possible** with at least one image pair.
6. Without images, **possible** only when all of these hold: same neighbourhood, same property
   type that is not an apartment, a matching area, a matching rooms signal and similar text.
7. Everything else → **low**.

`high` still goes to the user (`review_required`); only `low` is stored as `candidate` (considered
and kept apart). `low` results from metadata alone are not stored at all (there would be far too
many); `low` results that came from photos are stored so the audit shows they were considered.

## Merge, reject, undo (`db/matches.rs`)

All operations run in one `IMMEDIATE` transaction and are idempotent (repeating changes nothing
and writes no second audit event).

* **Accept** merges the two merge groups: the property with the **smaller id** (older root)
  survives, whichever listing happens to be `a`. The absorbed root gets
  `merged_into_property_id`; no listing moves. A target must be a root (database triggers), so
  cycles are impossible while chains are allowed. A pair already in one group is left untouched
  (`already_same_group`) and no decision is invented.
* **Undo** clears the absorbed property's pointer, records the match as `rejected` with
  `reverted_at` (so it is not proposed again) and keeps the merge ids as a record of what was
  undone. It refuses, changing nothing, if the stored pointers no longer agree with the record.
  Chains can be undone in any order.
* **Reject** keeps the pair apart; a live merge must be undone first. A user may still accept a
  rejected pair later.
* **Audit:** `property_merge_events` is append-only (triggers) and holds, for every merge and
  undo, the rule version, the note and a snapshot of both sides' user status.
* **User choices:** each property keeps its own stored status through a merge and its undo. The
  group shows the strongest (favorite > hidden > new) and flags a favorite/hidden disagreement.
  A choice made on the group applies to every member, so it is not undone by an undo.
  *(This favorite/hidden rule is an assumption awaiting the owner's confirmation.)*
* **Canonical values** (bedrooms, areas…) are recomputed for the merged group; ads that disagree
  leave the value unknown, with the disagreement recorded. They are restored by an undo.
* **Review status** of a property is `review_required` while a pair touching its group awaits the
  user, `reviewed` after any decision, otherwise `none`.

## Evaluation (`matching/evaluation.rs`)

`evaluate(set, config)` runs the **same** candidate generation and comparison as the application
over a labelled set and reports, separately and with denominators:

* **false merges**: different properties rated `high` (what an automatic merge would get wrong);
* **false review items**: different properties put in front of the user (`possible` or `high`);
* **missed duplicates**: same-property pairs rated `low` *or never proposed* by the generator;
* same-property pairs not rated `high`;
* the breakdown per scenario, every pair's outcome, and a list of limitations.

Every report states its provenance. A `synthetic` set "says nothing about performance on real
advertisements"; a `real` set is bounded by its size and how it was drawn. With fewer than 30
pairs of a kind only counts are given, and with zero false merges the report states the bound the
sample actually allows (the "rule of three": about 3/n at 95 % confidence).

### The synthetic regression set (what exists today)

Fourteen hand-written pairs, including the rule's known weak spots on purpose
(`matching/evaluation/tests.rs`). Result with the default configuration:

| | high | possible | low | never proposed |
|---|---|---|---|---|
| same property (6) | 2 | 2 | 1 | 1 |
| different properties (8) | **1** | 3 | 3 | 1 |

* False merges: **1 of 8** (twin houses of one developer with the same marketing photos on both
  ads). **The rule is therefore not safe for automatic merging, and this is why it stays off.**
* False review items: 4 of 8. Missed duplicates: 2 of 6 (heavily cropped photos; no photos and
  differing neighbourhood labels). Same-property pairs not rated high: 4 of 6.

These numbers describe constructed cases only.

### Building a real labelled set

A real evaluation needs advertisements collected where permitted, which depends on a source being
authorised (`docs/sources/`). When one is:

1. Collect, then export the comparison inputs of the listings involved (`MatchInput`: place, type,
   rooms, areas, established price, distinctive terms, image hashes). **No export command exists
   yet**; writing one is part of this step. The `tokens` of a listing are real words from the ad
   (digits are already excluded, but a name can remain), so before a set is kept or shared they
   must be replaced by opaque ids, consistently across the whole set (that preserves exactly the
   overlap the rule measures). The set must hold no contact details, no personal data and no
   URLs, and nothing from it belongs in the repository unless it has been sanitised this way.
2. Label each pair by hand as the same property or not, with a scenario: `same_cross_source`,
   `same_different_angle`, `similar_properties`, `condominium`, `image_transformation`,
   `conflicting_metadata`, `other`. Label from the originals, not from the matcher's output.
3. Include hard negatives (neighbouring houses of one developer, flats of one building) and
   listings that only populate the library (a shared placeholder photo).
4. Store it as JSON (`LabeledSet`: `description`, `provenance: "real"`, `listings`, `pairs`;
   `group_root` may be omitted) and run `evaluate`. Report the whole `EvaluationReport`.
5. Tune thresholds only against a **separate** examples set, never the one that is reported.

Automatic merging may be reconsidered only with a narrowly specified multiple-signal rule that
such a report supports, and after merge chains and undo have been verified (they are tested; see
`db/matches/tests.rs`).

## Known limitations

* **Property type is never extracted.** The Phase 1 parser always reports `Unknown`, so type-based
  evidence is inert in practice: the "same type" signal and the metadata-only `possible` path
  cannot fire, and the apartment cap cannot apply to parsed listings. This errs towards keeping
  listings apart, but a type extractor (with adversarial tests) would make matching meaningfully
  better. It is not part of what the owner approved, so it was not added.
* The only evaluation is synthetic; real performance is unknown.
* Identical houses with the same marketing photos are rated `high` (see above).
* Text similarity is a weak signal: ads built from the same template look alike.
* Matching loads every listing into memory; tested with 4000 listings, not with very large
  libraries.
