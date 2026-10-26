# Loose Canon — Session Handoff (end of Phase 1)

Snapshot date: **2026-10-26**. Model used in the session: **Claude Sonnet 5.5**.
Audience: the next engineer or AI session that continues this project.
Everything here is either (a) verified by a command run during the session (evidence is in
section 14), or (b) explicitly labelled as an assumption, a proposal, or *not verified*.

> **Redaction note.** This file intentionally omits the user's account email address and
> machine-specific absolute paths / Windows user name. Paths are written relative to the
> repository root (`<repo>`). See section 3 for the full sharing review.

---

## 0. Status at a glance

| Item | State |
|---|---|
| Phase 1 — Foundation | **Done and verified** (section 14) |
| Phase 2 — Source collection & matching | **Not started. Awaiting an explicit "proceed to Phase 2" from the user.** The user has already answered the four open questions (section 2) but has not yet said to start. |
| Phase 3 — Ranking & optional Gemini | Not started |
| Phase 4 — React UI & Windows packaging | Not started |
| Git | Branch `main`, single commit `f0153e4 Initial commit`. **Nothing from this session is committed** (the user did not ask for a commit). |
| Tests | `cargo test`: **149 passed, 0 failed** (parser 77, db 37, models 32, commands 3) |
| Static checks | `cargo fmt --check` clean; `cargo clippy --all-targets -- -D warnings` clean (pedantic lints on); `npm run typecheck` and `npm run build` pass |
| Real app | Built (`tauri build --debug --no-bundle`), launched 3×; DB created in the Tauri app-data dir, migrated once, user data survived relaunches |
| Not verified | That the React page actually rendered inside the webview and that `get_app_status` returned there (only the window title / process liveness were observed). Command logic itself is unit-tested. |
| Not implemented yet but approved by the user | The **price-field hint** for adapters (decision 1) — see sections 2 and 17 |

---

## 1. How to resume (first 15 minutes)

1. Read this file top to bottom, then `README.md`, then `<repo>/src-tauri/src/db/schema.sql`
   and `<repo>/src-tauri/src/parser/br_real_estate.rs` (module docs explain the algorithm).
2. Confirm the toolchain (section 5) and build once:
   ```powershell
   npm install
   npm run build                    # REQUIRED before any cargo compile: Tauri embeds ../dist
   cd src-tauri
   cargo fmt --check
   cargo clippy --all-targets -- -D warnings
   cargo test                       # expect: 149 passed
   ```
3. Do **not** start Phase 2 until the user says so explicitly (the original instruction was
   "Stop after Phase 1 … until I explicitly approve proceeding").
4. When approved, begin with section 17 (Phase 2 plan).
5. Conventions to keep (section 19): test-first, no commits unless asked, commit/PR attribution
   lines, UI language pt-BR, never put secrets in the repo.

---

## 2. Decisions log (user answers, 2026-10-26)

At the end of Phase 1 the assistant asked four questions. The user answered:

| # | Question asked | User's answer (verbatim) | Interpretation / what it means for the code |
|---|---|---|---|
| 1 | Should adapters be able to pass a hint so a source's structured "asking price" field counts as a full price at Medium confidence? (Without it, every unlabelled-price listing is hidden by "Ocultar Ágio".) | "You can add the hint" | **Approved.** Not implemented yet. Design proposal in section 17.2. It lives in the parser API (`parse_listing_text` has no hint parameter today). |
| 2 | Is Medium confidence the right minimum for a price to count as comparable? | "Yes, low confidence (less than 60% of certain should not be classified as comparable)" | **Confirmed**: `MIN_COMPARABLE_CONFIDENCE = Confidence::Medium` stays (`src-tauri/src/models/price.rs`); `Low` is never comparable. The user additionally defines the numeric meaning: **anything below 60 % certainty is "Low" and must not be treated as comparable.** Consequence: the `Medium` band must really mean ≥ 60 % certain. The current Low/Medium/High assignments are *hand-set heuristics, not calibrated probabilities* — they must be calibrated on labelled real listings in Phase 2 (section 17.3). The 60 % floor is the user's number; the Medium/High boundary (proposed 85 %) is only a proposal. |
| 3 | Which two sources should be evaluated first (62imoveis, OLX, ZAP Imóveis, or a local brokerage)? | "all of them" | Interpreted as: **evaluate all four candidate sources** (62imoveis, OLX, ZAP Imóveis, local brokerages). **Tension to confirm at Phase 2 kickoff:** the original specification says "Start with two permitted sources" and "Do not expand to many sources before the initial adapters prove reliable." Recommended reconciliation: do the access-rule/feasibility review for all four first, then implement adapters incrementally (first the two that pass review and are technically simplest) — unless the user explicitly says to implement all four at once. Instagram/Facebook remain out of scope as HTML sources (permitted APIs/providers only). |
| 4 | Is the bundle identifier `br.com.a3consultoria.loosecanon` the right one? | "no, will be informed later" | The identifier in `src-tauri/tauri.conf.json` is a **placeholder** (derived from the company domain) and must be replaced when the user provides the real one. **Impact:** the identifier is also the name of the per-user app-data folder, so changing it moves where the SQLite file lives. Nothing is shipped yet, so it is safe to change now; do it before any packaging/distribution. Do not invent another value. |

---

## 3. Sharing review ("does the project contain files that can't be shared?")

Requested by the user at the end of the session. Method (all run against the working tree,
excluding git-ignored folders which are listed separately):

* keyword scan: `api key | secret | password | bearer | authorization: | private key | client secret | access/auth token`
* token-format scan: Google `AIza…`, OpenAI-style `sk-…`, GitHub `ghp_/gho_`, Slack `xox…`, AWS `AKIA…`, JWT shape, PEM `-----BEGIN`
* email-address scan
* machine/identity scan: `C:\Users`, `/Users/`, the local Windows user name, git user handle, company domain
* credential-like **file names**: `.env*`, `*.pem`, `*.key`, `*.pfx`, `*.p12`, `*.jks`, `*credential*`, `*secret*`, `*.sqlite*`, `*.db`, `id_rsa*`, `*.token`, `.npmrc`, `.netrc`
* hidden files at the repo root
* git: tracked files, number of remotes, whether the remote URL embeds credentials (`://user@…`; the URL itself was *not* printed)
* lockfile sources (npm `resolved` URLs, Cargo `source`)

| Check | Result |
|---|---|
| Secret keywords | 1 hit, benign: `src-tauri/src/db/tests.rs` uses the string `'/etc/passwd'` as a **fake absolute path** for a negative CHECK-constraint test |
| Token formats | none |
| Email addresses | none (the single regex hit was `icons/128x128@2x.png` in `tauri.conf.json`, a false positive) |
| Local user name / `C:\Users` paths in files | none |
| Credential-like files, `.env`, databases | none in the repo |
| Hidden files | only `.git/` and `.gitignore` |
| Git remote | 1 remote configured; **no embedded credentials** |
| Lockfiles | npm: only `registry.npmjs.org`; Cargo: 417 packages, all from the crates.io index |
| Ignored folders | `dist/`, `node_modules/`, `src-tauri/gen/`, `src-tauri/target/` — correctly git-ignored |

**Conclusion: no file must be withheld.** Soft flags to be aware of before sharing publicly:

1. `LICENSE` names the copyright holder by handle (this was already in the initial commit).
2. `src-tauri/tauri.conf.json` → `identifier` contains the company name (`a3consultoria`). It is
   a placeholder pending decision 4; replace it (or use a neutral placeholder) if the repo is
   published before the real identifier is known.
3. **Do not zip/share the ignored folders.** `src-tauri/target/` embeds absolute local paths and the
   Windows user name in debug info; `node_modules/` and `dist/` are just bulky build output.
4. The smoke-test databases created under the Tauri app-data folder are **outside the repo** and
   were deleted after testing (the folder no longer exists).
5. **All fixtures in tests are fabricated.** E.g. fake phone `(62) 99999-9999`, example URLs.
   When Phase 2 introduces *recorded real listings*, they can contain sellers' names, phone
   numbers, emails and addresses → **sanitize before committing** (see section 17.6).
6. There is no Gemini (or any) API key anywhere. The design rule stays: the optional key goes in
   Windows Credential Manager (e.g. `keyring` crate), never in the repo, never in the binary.

---

## 4. Session context and timeline

### 4.1 Setting

* Project: **Local-First Real Estate Discovery App** — Windows desktop app that discovers,
  consolidates, filters and ranks real-estate listings; initial region = Goiânia metropolitan
  area (Goiânia, Aparecida de Goiânia, Hidrolândia, Goiás, Brazil).
* The user pasted a very long implementation prompt (the full text is **Appendix B**). Key
  meta-rules in it: work incrementally; Phase 1 only; no pseudocode; no fabricated test results;
  report actual command results; stop after Phase 1 and wait for explicit approval.
* The session began with the user running `/model` to select Sonnet 5.5.
* The repo at session start contained only `LICENSE` (MIT) and a 2-line `README.md`
  (`# loose-canon` / `hope will not save you now`), one commit on `main`, clean status.
  The README's two lines were preserved; documentation was appended below them.

### 4.2 Tooling/harness context that influenced the work

* **Skills used:** `rust-router` (Rust routing; its default-project hints: edition 2024, clippy
  pedantic), `superpowers:test-driven-development` (red → green discipline; "no production code
  without a failing test first"), `hawkscan:hawkscan` (invoked because a session hook asked for a
  security scan after code changes — judged **not applicable**, see 15.9).
* A session hook states the HawkScan environment: **hawk runtime = false, `HAWK_API_KEY` = false.**
* Windows 11, PowerShell primary + Git Bash available. Each has its own quoting rules (see 19.4).
* Persistent memory directory exists for this project; a memory entry with the decisions in
  section 2 was written (section 19.6).

### 4.3 Chronological timeline

1. **Discovery.** Listed repo (only `LICENSE`, `README.md`); read toolchain versions; confirmed
   MSVC Build Tools 2022 and WebView2 are present; confirmed crates.io reachable.
   `cargo search tauri` ranks **3.0.0-alpha.5** first, so Tauri **v2** was pinned explicitly
   (latest 2.x = 2.12.5).
2. **Scaffold.** Frontend (Vite + React 19 + TypeScript) with a deliberately minimal placeholder
   page; Tauri v2 config; capabilities; icons generated from a generated placeholder PNG
   (white "LC" on blue `rgb(30,64,175)`, 1024 px, via System.Drawing → `npx tauri icon`;
   Android/iOS/`.icns`/Store logos were deleted as unnecessary for a Windows desktop app).
3. **Warm-up build** of the Tauri dependency tree in the background: **2 m 29 s** cold.
4. **Models** (`error.rs`, `models/*`). Wrote `assess_full_price` test-first: 4 tests red against a
   stub, then green. Removed a redundant `from_db` helper.
5. **Parser, test-first.** Wrote stubs + tests for `text.rs`, `number.rs`, `br_real_estate.rs`:
   **62 failed / 6 passed (vacuous)** → implemented helpers (12 green) → implemented main module.
   Compile error (closure monomorphic over area kinds) fixed with the generic `extracted` helper.
   First full run: 93/94 — failure exposed a **real bug**: `garagens?` does not match `garagem`.
   Then wrote *adversarial probing tests*; one (`bare_saldo_is_a_balance_never_a_price`) failed
   as predicted: a bare `saldo R$ 150 mil` was promoted to a full-price candidate. Fixed by adding a
   `BalanceGeneric` cue. Result: **104 parser+model tests green**.
6. **Database, test-first.** `schema.sql` placeholder + stubs + 37 tests → **34 failed / 3 passed** →
   implemented migrations/pragmas/`open`, real schema, `price_history.rs` → **37 green**. A big
   table-driven constraint test passed on first run, which was suspicious; the helper was
   tightened (`assert_check_rejected` requires a CHECK/NOT NULL failure, not any constraint) and one
   case that used a non-existent FK target was fixed; still green.
7. **`headline_price`** had been written *before* its tests (process slip). It was stubbed, tests
   written (4 red), reimplemented with a refined rule (conflicting full prices ⇒ no headline).
8. **Tauri wiring.** `commands::get_app_status`, `lib.rs` `setup` opens the DB in `app_data_dir()`.
   Test-first (1 red) → 150 tests green.
9. **Static analysis.** Clippy pedantic with `-D warnings` produced ~16 findings (listed in
   section 14.3), all fixed; `extract_areas` (111 lines) was split into helpers; dead code
   (`evidence_covering`) removed; `rustfmt` applied.
10. **Tests moved to sibling files** (`parser/br_real_estate/tests.rs`, `db/tests.rs`) because the
    parser file had grown to 2,145 lines; implementation is now 1,213 lines.
11. **Tauri features.** `default-features = false` was initially used and then **reverted** after
    finding the defaults include `common-controls-v6` (needed for native dialogs on Windows) and
    `dynamic-acl`. The Tauri CLI later normalised the line to `features = []` (equivalent).
12. **Real-app smoke test** (section 14.5): build, launch ×3, DB inspection, relaunch idempotency.
13. **README** extended; **HawkScan** consulted and judged not applicable; **final verification**
    rerun; Phase 1 report delivered with four questions.
14. **Follow-up (this request):** sensitive-files review (section 3), decisions recorded
    (section 2), this handoff written.

---

## 5. Environment facts

| Item | Value |
|---|---|
| OS / shells | Windows 11 Pro 10.0.26200; PowerShell 5.1 primary; Git Bash |
| Rust | `rustc 1.99.0 (b940084d7 2026-09-28)`, `cargo 1.99.0`, toolchain `stable-x86_64-pc-windows-msvc` |
| Node / npm | `v24.19.0` / `11.17.0` |
| MSVC | Visual Studio 2022 **Build Tools** present (vswhere finds the VC tools component) |
| WebView2 runtime | `153.0.4234.48` |
| Python | `python.exe` from the WindowsApps launcher works; used only for inspecting SQLite files (`python -I`) |
| Not installed / not available | `hawk` CLI (per the session hook), HawkScan credentials |

**Pinned versions**

Rust (`src-tauri/Cargo.toml`, edition **2024**, `rust-version = "1.90"` = Tauri 2.12 MSRV):

| Crate | Version | Notes |
|---|---|---|
| `tauri` | 2.12.5 | default features kept |
| `tauri-build` | 2.7.4 | build-dependency |
| `rusqlite` | 0.41.0 | `bundled` |
| `serde` | 1.0.229 | `derive` |
| `serde_json` | 1.0.154 | |
| `regex` | 1.13.1 | |
| `chrono` | 0.4.45 | `default-features = false`, features `std`, `clock`, `serde` |
| `thiserror` | 2.0.21 | |
| `tempfile` | 3.27.0 | dev-dependency |

npm (`package.json`; resolved versions in `package-lock.json`):
`react ^19.3.1`, `react-dom ^19.3.1`, `@tauri-apps/api ^2.12.4`, dev: `@tauri-apps/cli ^2.12.4`,
`@types/react ^19.3.0`, `@types/react-dom ^19.3.0`, `@vitejs/plugin-react ^6.1.2`,
`typescript ^7.0.2` (**TypeScript 7**, newest major), `vite ^8.3.5` (newest major).

**Deferred dependencies and the phase that should introduce them:**
Phase 2 `tokio`, `reqwest`, `scraper`, `url`, an image crate + pHash implementation;
Phase 3 `reqwest` (Gemini REST) and a credential-store crate (e.g. `keyring`);
Phase 4 `ort` (optional ONNX vision), model-download hashing, **Tailwind CSS and Lucide icons**
(both listed in the spec's stack but deliberately not installed in Phase 1).
Tauri already ships an async runtime, so `tokio` was not needed in Phase 1.

---

## 6. Repository state

* Branch `main`; last commit `f0153e4 Initial commit`; **no new commits**.
* `git status`: ` M README.md`; untracked: `.gitignore`, `index.html`, `package-lock.json`,
  `package.json`, `tsconfig.json`, `vite.config.ts`, `src/`, `src-tauri/`; and `HANDOFF.md`
  (this file). Ignored: `dist/`, `node_modules/`, `src-tauri/gen/`, `src-tauri/target/`.
* `.gitignore` covers: `node_modules/`, `dist/`, `*.tsbuildinfo`, `src-tauri/target/`,
  `src-tauri/gen/`, `*.sqlite3`, `*.sqlite3-wal`, `*.sqlite3-shm`, `.vscode/`, `.idea/`,
  `.DS_Store`, `Thumbs.db`. **`Cargo.lock` and `package-lock.json` are meant to be committed**
  (application, reproducible builds).
* One git remote is configured (URL not inspected; no embedded credentials).

---

## 7. File inventory (line counts measured)

```
<repo>/
  .gitignore
  HANDOFF.md                               this file
  README.md                                project intro, layout, commands, design notes
  LICENSE                                  MIT (unchanged)
  index.html                       12      Vite entry (lang="pt-BR")
  package.json                     25      scripts: dev, build (tsc --noEmit && vite build), typecheck, tauri
  package-lock.json              1539
  tsconfig.json                    18      strict TS, noUnused*, bundler resolution, jsx react-jsx
  vite.config.ts                   13      port 1420 strictPort (matches tauri devUrl), ignores src-tauri
  src/
    main.tsx                        9      React 19 root, StrictMode
    App.tsx                        48      placeholder: calls get_app_status, shows status or error (pt-BR)
    lib/api.ts                     22      AppStatus/CommandError types, getAppStatus(), runningInsideTauri()
  src-tauri/
    Cargo.toml                     43
    Cargo.lock
    build.rs                        3      tauri_build::build()
    tauri.conf.json                36      see section 12
    capabilities/default.json       7      window "main", permission core:default
    icons/                                 128x128.png, 128x128@2x.png, 32x32.png, 64x64.png, icon.ico, icon.png (PLACEHOLDERS)
    src/
      main.rs                       6      windows_subsystem="windows" in release; calls loose_canon_lib::run()
      lib.rs                       28      modules + run(): setup opens DB in app_data_dir, manages Database, registers commands
      error.rs                    106      ModelError, ParseError, Error (+Serialize as {kind,message}), Result
      commands/mod.rs              74      AppStatus, app_status(&Database), #[tauri::command] get_app_status (+3 tests)
      models/
        mod.rs                     63      `string_enum!` macro + module list
        money.rs                  118      Currency, Money (+tests)
        area.rs                   204      Area, LotArea/BuiltArea/UsableArea/UnspecifiedArea, AreaKind, ClassifiedArea (+tests)
        confidence.rs              31      Confidence (+tests)
        evidence.rs               124      TextSpan, Evidence, WarningKind, ParseWarning, DiscardReason, DiscardedMention
        price.rs                  374      PriceClassification, typed amounts, ClassifiedPrice, PriceMention, FullPriceStatus, assess_full_price, PriceProvenance (+tests)
        characteristics.rs        185      Extracted<T>, Fact<T>, PropertyType, LotShare, PropertyCharacteristics (+tests)
        listing.rs                230      ListingStatus, ExtractionStatus, UserStatus, ExtractedFacts, SourceRef, NormalizedListing::headline_price (+tests)
      parser/
        mod.rs                      8      re-exports parse_listing_text, PARSER_VERSION, parse_money
        text.rs                   168      FoldedText: lowercase/accent folding with byte-offset map to the original (+6 tests)
        number.rs                 378      BR number & money normalisation (+5 tests)
        br_real_estate.rs        1213      the extractor (prices, counts, areas, lot share)
        br_real_estate/tests.rs   930      66 tests
      db/
        mod.rs                    217      Database, configure, migrate, timestamps, MIGRATIONS
        schema.sql                291      migration 1
        price_history.rs          153      record_observation, list_observations
        tests.rs                 1021      37 tests
```

Gaps vs. the spec's suggested module list: `sources`, `matching`, `ranking`, `ai` were **not
created** on purpose ("do not create empty modules for features not yet implemented").

---

## 8. Architecture and design decisions (with the reasoning)

1. **Rust owns DB and logic; the frontend only calls typed commands.** `@tauri-apps/plugin-sql`
   is not used. Commands stay thin. Command errors serialise as `{kind, message}`.
2. **Money = integer centavos, area = integer hundredths of m².** Chosen so equality/ordering
   are exact; `Money` has no arithmetic operators on purpose; amounts parse with `i128`
   integer math and reject anything not exactly representable (`R$ 350.000,555`).
3. **Types prevent mixing financial concepts.** `FullPrice`, `Agio`, `DownPayment`,
   `MonthlyPayment`, `OutstandingBalance`, `UnclassifiedAmount` are distinct newtypes wrapped by
   `ClassifiedPrice`. Same for `LotArea` / `BuiltArea` / `UsableArea` / `UnspecifiedArea`.
4. **`Fact<T>` = `Unknown | Known | Conflicting`.** Unknown is never 0; disagreements are kept
   with evidence, never resolved silently. `Fact::from_candidates` merges equal values (keeping the
   most confident) and reports different values as `Conflicting`.
5. **Parser prefers uncertainty.** Every value has confidence, original-text evidence and
   warnings; non-facts are listed in `discarded` with a reason.
6. **Unlabelled amount ⇒ `Unknown`.** Exception (promotion rule): an unlabelled amount ≥ R$ 10.000
   that is strictly larger than every labelled financing component (ágio / down payment /
   instalment / balance with ≥ Medium confidence) becomes a **Low-confidence `FullPrice` candidate**
   with warning `assumed_full_price`. Low is never comparable.
7. **"Ocultar Ágio (Apenas Valor Total)" filter predicate** = `FullPriceStatus::passes_full_price_only_filter()`
   = exactly one distinct `FullPrice` amount with confidence ≥ `MIN_COMPARABLE_CONFIDENCE` (Medium).
   Conflicting full prices ⇒ fails. It is a pure function; it is **not yet wired** to a search command.
8. **`NormalizedListing::headline_price` precedence:** established full price → (conflicting ⇒ nothing)
   → most confident full-price *candidate* → most confident other amount (earliest wins ties).
   The result always carries its classification, so a non-full price cannot be mistaken for one.
9. **Properties vs listings.** `properties` has **no price column** (no "lowest price ever" master
   price). Prices live on `listing_links` (headline) and in append-only `listing_price_history`.
10. **Soft merge.** `properties.merged_into_property_id` (+`merged_at`) marks a merge; listings keep
    their original `property_id`; undo = clear those two columns + mark the `property_matches` row
    `rejected` with `reverted_at`. Nothing is ever moved or deleted.
11. **DB-level invariants** (so no code path can violate them): append-only price history
    (triggers), automatic merges only at `high` confidence category, hidden⇔`hidden_at`, merge
    pointer⇔timestamp, canonical ordering `listing_a_id < listing_b_id`, relative image paths only,
    embedding blob length = dim × 4, etc.
12. **Migrations via `PRAGMA user_version` + `schema_migrations` audit table**, each in its own
    transaction, forward-only, refuse newer DBs, refuse inconsistent bookkeeping, plain `CREATE`
    (never `IF NOT EXISTS`) so schema never silently merges with existing data.
13. **Single SQLite connection behind a `Mutex`** (`Database::with_conn`). Fine for Phase 1; keep
    transactions short in Phase 2; add a read pool only if measurements demand it.
14. **Timestamps** stored as UTC RFC 3339 with milliseconds (`2026-01-31T12:00:00.000Z`) so text
    ordering equals chronological ordering. `format_timestamp` / `parse_timestamp` in `db/mod.rs`.
15. **pHash storage convention:** 64-bit hash stored as the signed INTEGER with the same bit
    pattern (`i64::from_ne_bytes(u64.to_ne_bytes())`); Hamming distance is computed in Rust.
16. **Embedding storage:** one row per image: little-endian f32 BLOB + `embedding_model`
    (`name@version`) + `embedding_dim` + `embedding_dtype = 'f32le'`; embeddings from different
    models are never comparable.
17. **User status lives on `properties`** so hiding survives rediscovery. Rediscovery of the same ad
    is an upsert on `(adapter_id, normalized_url)` (tested).
18. **Frontend is a placeholder** proving the typed bridge. Tailwind/Lucide deferred to Phase 4.
19. **UI strings and parser warning messages are Brazilian Portuguese**; code and comments English.
20. **`unsafe_code = "forbid"`** and clippy `all` + `pedantic` (with `module_name_repetitions`,
    `missing_errors_doc`, `missing_panics_doc` allowed).

---

## 9. Domain model reference (`src-tauri/src/models`)

All enums are declared with the `string_enum!` macro: serde name = SQL text = `as_str()`;
`ALL` constant lists variants; `FromStr` rejects unknown text with `ModelError::UnknownVariant`.
A DB test proves the Rust enums and the SQL CHECK constraints accept exactly the same values.

| Type | Values / shape |
|---|---|
| `Currency` | `BRL` |
| `Confidence` (Ord: Low < Medium < High) | `low`, `medium`, `high` |
| `PriceClassification` | `full_price`, `agio`, `down_payment`, `monthly_payment`, `outstanding_balance`, `unknown` |
| `AreaKind` | `lot`, `built`, `usable`, `unspecified` |
| `PropertyType` | `house`, `semi_detached`, `sobrado`, `apartment`, `lot`, `chacara`, `other` |
| `LotShare` | `whole`, `half` |
| `ListingStatus` | `active`, `unavailable`, `unknown` |
| `ExtractionStatus` | `pending`, `ok`, `partial`, `failed` |
| `UserStatus` | `new`, `favorite`, `hidden` |
| `WarningKind` | `unlabelled_amount`, `assumed_full_price`, `conflicting_labels`, `non_sale_amount`, `dot_as_decimal_separator`, `suites_may_be_additional_to_bedrooms`, `count_implied_by_singular`, `unspecified_area_kind` |
| `DiscardReason` | `linear_measure_not_area`, `fraction_of_lot`, `fraction_not_bedrooms`, `bedroom_shorthand_without_context`, `ambiguous_bedroom_range`, `ambiguous_amount_range`, `unsupported_area_unit`, `plural_without_count`, `implausible_count`, `unparseable_number` |

Structs / wrappers:

* `Money(i64)` — non-negative centavos; `from_centavos`, `from_reais`, `checked_add`; `Display` → `R$ 350.000,00`;
  serde via `try_from = "i64"` (negative rejected on deserialise).
* `Area(u32)` — hundredths of m², `0 < area ≤ MAX_AREA_M2 (10,000,000 m²)`; `Display` → `180 m²`, `180,5 m²`, `180,55 m²`.
* Typed area wrappers `LotArea`, `BuiltArea`, `UsableArea`, `UnspecifiedArea`; `ClassifiedArea` (adjacently tagged `{"kind","area"}`).
* Typed price wrappers `FullPrice`, `Agio`, `DownPayment`, `MonthlyPayment`, `OutstandingBalance`, `UnclassifiedAmount`;
  `ClassifiedPrice` (adjacently tagged `{"classification","amount"}`), with `new`, `classification()`, `money()`.
* `PriceMention { price, confidence, evidence, warnings }` — `evidence` covers the cue *and* the amount (e.g. `ágio de R$ 80 mil`).
* `FullPriceStatus` = `Established{price, confidence}` | `Conflicting{candidates}` | `NotEstablished`.
  `assess_full_price(&[PriceMention])`: only `FullPrice` mentions with confidence ≥ Medium count; equal
  amounts merge (best confidence); different amounts ⇒ `Conflicting` (sorted ascending).
* `PriceProvenance { extractor, source_field: Option, evidence: Option }` — stored with each history row.
* `TextSpan { start, end }` (byte offsets into the **original** text), `Evidence { span, snippet }`
  (`Evidence::from_span` panics on out-of-range / non-boundary spans — the parser guarantees validity).
* `Extracted<T> { value, confidence, evidence, warnings }`; `Fact<T>` (serde `{"status","data"}`);
  `PropertyCharacteristics { property_type, bedrooms, suites, parking_spaces, lot_area, built_area, usable_area, unspecified_areas: Vec<Extracted<UnspecifiedArea>>, lot_share }`.
* `ExtractedFacts { parser_version, prices, characteristics, discarded }` — serialised into `listing_links.extracted_facts_json`.
* `SourceRef { platform, adapter_id, source_listing_id }`;
  `NormalizedListing { source, url, normalized_url, title, description, location_text, currency, facts, status, extraction_status, extraction_error, first_seen_at, last_seen_at, last_verified_at, last_scraped_at }`.
* Errors (`error.rs`): `ModelError` (`NegativeMoney`, `MoneyOverflow`, `InvalidArea`, `UnknownVariant`);
  `ParseError` (`Empty`, `UnrecognizedFormat`, `InvalidNumber`, `TooPrecise`, `ZeroAmount`, `OutOfRange`);
  `Error` (`Database`, `Io`, `Json`, `Model`, `Parse`, `SchemaTooNew`, `Migration`,
  `MigrationStateInconsistent`, `ForeignKeysUnavailable`, `InvalidStoredValue`,
  `OutOfOrderObservation`, `LockPoisoned`) with `kind()` strings for the frontend.

Not yet modelled in Rust (tables exist; add with Phase 2 code): `RegionType`, match decision/confidence
enums, image processing status, region/property/image/match structs and repositories.

---

## 10. Database reference

**File:** `loose_canon.sqlite3` in Tauri's per-user app-data dir (`app.path().app_data_dir()`;
folder name = bundle identifier). `db::database_file(dir)` joins the name. `Database::open` creates
parent dirs, opens, `configure`s, migrates.

**`configure(conn, file_backed)`:** `busy_timeout` 5 s; `PRAGMA foreign_keys = ON` **and verified**
(`Error::ForeignKeysUnavailable` otherwise); for file DBs `journal_mode = WAL` (result not treated as
an error if the filesystem refuses) and `synchronous = NORMAL`.

**Migrations:** `MIGRATIONS: &[Migration{version,name,sql}]` (v1 = `schema.sql`, name `initial_schema`),
`LATEST_SCHEMA_VERSION`. `migrate_with` per migration: transaction → `execute_batch` → insert
`schema_migrations` row → `PRAGMA user_version` → commit; any failure rolls that migration back
(`Error::Migration{version,name,source}`). Bookkeeping check (`schema_migrations` max version ==
`user_version`) before applying anything when `user_version > 0`. To change the schema **append** a
migration (`0002_*.sql`); never edit a shipped one; remember FK enforcement cannot be toggled inside
a transaction when rebuilding tables.

**`Database` API:** `open(&Path)`, `open_in_memory()`, `with_conn(|&mut Connection| …)`,
`schema_version()`, `foreign_keys_enabled()`. Mutex poisoning ⇒ `Error::LockPoisoned`.

**`db::price_history`:**
* `record_observation(conn, listing_id, &PriceObservation) -> RecordOutcome { Recorded{id} | Unchanged{latest_id} }`
  — `BEGIN IMMEDIATE`; compares to the latest row by `(observed_at DESC, id DESC)`; **unchanged = same amount
  AND same classification** (confidence/provenance/timestamp differences alone are not changes);
  A→B→A is three rows; an observation **older than the latest** ⇒ `Error::OutOfOrderObservation` (nothing
  written); unknown listing ⇒ FK error (nothing written).
* `list_observations(conn, listing_id)` — oldest first, strict parsing (`Error::InvalidStoredValue` on bad stored text).

### Schema v1 (source of truth: `src-tauri/src/db/schema.sql`, 291 lines)

Conventions: `STRICT` tables; money = INTEGER centavos; areas = INTEGER hundredths m²; scores/similarities REAL;
UTC ms timestamps (defaults via `strftime('%Y-%m-%dT%H:%M:%fZ','now')`); unknown = NULL; enums = TEXT + CHECK;
history-like FKs `ON DELETE RESTRICT`, derived image metadata `ON DELETE CASCADE`; nothing deletes data when an ad disappears.

**`schema_migrations`**: `version` PK (>0), `name` NOT NULL non-empty, `applied_at` NOT NULL.

**`regions`**: `id` PK AUTOINCREMENT; `slug` UNIQUE non-empty (stable seed key); `country_code` default `'BR'` (len 2);
`state_code` (len 2); `municipality` (NULL only for `state`); `name` non-empty; `region_type` ∈ {`state`,`municipality`,
`neighborhood`,`district`,`condominium`,`corridor`,`other`}; `parent_id` → regions (RESTRICT, ≠ id); `enabled` 0/1 default 1;
`metadata_json` (json_valid or NULL); `created_at`, `updated_at`. Indexes: `idx_regions_parent`,
`idx_regions_lookup (state_code, municipality, region_type) WHERE enabled = 1`.

**`properties`**: `id`; `region_id` → regions; `municipality`, `neighborhood` (text); `property_type` (enum or NULL);
`bedrooms`, `suites`, `parking_spaces` (0..100 or NULL); `lot_area_centi_m2`, `built_area_centi_m2`,
`usable_area_centi_m2` (>0 or NULL); `characteristics_json`, `canonical_provenance_json` (json_valid or NULL);
`local_score` REAL 0..100 or NULL; `local_score_json`; `data_confidence` (low/medium/high or NULL);
`user_status` default `'new'` ∈ {new,favorite,hidden}; `hidden_at`; `match_review_status` default `'none'` ∈ {none,review_required,reviewed};
`merged_into_property_id` → properties (RESTRICT) and `merged_at`; `created_at`, `updated_at`.
CHECKs: `(user_status='hidden') = (hidden_at IS NOT NULL)`; `(merged_into IS NULL) = (merged_at IS NULL)`; `merged_into <> id`.
Indexes: region, user_status, `(municipality, neighborhood)`, partial on `merged_into_property_id`. **No price column by design.**

**`listing_links`**: `id`; `property_id` → properties (nullable, RESTRICT); `source_platform`, `adapter_id` (non-empty);
`source_listing_id`; `url`, `normalized_url` (non-empty); `title`, `raw_description`, `location_text`;
`price_centavos` (≥0 or NULL); `price_classification` default `'unknown'` (six values); `price_confidence` (low/medium/high or NULL);
`currency` default `'BRL'` (CHECK in ('BRL')); `extracted_facts_json` (json_valid or NULL); `parser_version`;
`listing_status` default `'unknown'` ∈ {active,unavailable,unknown}; `extraction_status` default `'pending'` ∈ {pending,ok,partial,failed};
`extraction_error`; `first_seen_at`, `last_seen_at` (NOT NULL); `last_verified_at`, `last_scraped_at`; `created_at`, `updated_at`.
Constraints: `UNIQUE(adapter_id, normalized_url)`; `(price_centavos IS NULL) = (price_confidence IS NULL)`;
`price_centavos IS NOT NULL OR price_classification = 'unknown'`; `last_seen_at >= first_seen_at`.
Indexes: `property_id`; `(listing_status, last_seen_at)`; `(price_classification, price_centavos)`.

**`property_images`**: `id`; `listing_id` → listing_links NOT NULL (**CASCADE**); `position` ≥0 default 0; `source_url`;
`local_path` (relative only: non-empty, no `:`, not starting with `/` or `\`); `width`,`height` (>0 or NULL);
`phash` INTEGER + `phash_algorithm` (both or neither); `embedding` BLOB + `embedding_model` + `embedding_dim` (>0) +
`embedding_dtype` (`'f32le'` only) — all four together or none, and `length(embedding) = embedding_dim * 4`;
`processing_status` default `'pending'` ∈ {pending,hashed,embedded,failed,skipped}; `processing_error`; `created_at`; `processed_at`.
`UNIQUE(listing_id, source_url)`. Indexes: listing, partial on `phash`, partial on pending status.

**`property_matches`**: `id`; `listing_a_id`, `listing_b_id` → listing_links NOT NULL (RESTRICT); `phash_distance` 0..64 or NULL;
`visual_similarity` −1..1 or NULL with `visual_model` (both or neither); `metadata_signals_json`; `evidence_json` NOT NULL json_valid;
`confidence_category` ∈ {high,possible,low}; `confidence_score` 0..1 or NULL; `decision_status` default `'candidate'` ∈
{candidate,accepted,rejected,review_required}; `decision_source` ∈ {auto,user} or NULL; `decision_note`;
`merge_surviving_property_id`, `merge_absorbed_property_id` → properties (RESTRICT); `created_at`, `reviewed_at`, `reverted_at`.
CHECKs: `UNIQUE(a,b)` and `a < b`; decided (accepted/rejected) ⇔ `decision_source` and `reviewed_at` present;
`NOT (accepted AND auto) OR confidence_category = 'high'`; both merge ids or neither, and different;
a recorded merge is either live (`accepted`, not reverted) or reverted (`rejected` + `reverted_at`); `reverted_at` only if a merge was recorded.
Indexes: `listing_b_id`, `decision_status`.

**`listing_price_history`**: `id`; `listing_id` → listing_links NOT NULL (RESTRICT); `amount_centavos` ≥0; `price_classification` (six values);
`confidence` (or NULL); `observed_at`; `provenance_json` NOT NULL json_valid. Index `(listing_id, observed_at DESC, id DESC)`.
Triggers `listing_price_history_no_update` / `_no_delete` raise `'listing_price_history is append-only'`.

**`updated_at` triggers** on `regions`, `properties`, `listing_links` (`AFTER UPDATE … WHEN NEW.updated_at = OLD.updated_at`).

Total after migration 1: 7 tables, 5 triggers (verified on the DB created by the real app: `user_version=1`, WAL,
`integrity_check ok`, empty `foreign_key_check`).

---

## 11. Parser reference (`src-tauri/src/parser`)

**Public API**
* `parse_listing_text(&str) -> ExtractedFacts` — callers may join title and description with a newline (newline is a sentence boundary).
* `PARSER_VERSION = "br_real_estate/1"` — stored in provenance; **bump when behaviour changes**.
* `parse_money(&str) -> Result<Money, ParseError>` — standalone normaliser for strings *known* to be money
  (`R$ 350.000,00`, `R$350.000`, `350 mil`, `350 mil reais`, `350k`, `350K`, `R$ 1,2 milhão/milhões`, `1.200.000`, `R$ 2.500,50`, `350000`, `R$ 349,90`).
  It does **not** decide whether a string is a price.

**Pipeline.** `FoldedText` lowercases, strips Portuguese diacritics (also combining marks), maps `²`→`2`, `×`→`x`, dashes→`-`,
NBSP/whitespace→space (newline kept) one-to-one, and records for every folded byte the original char span, so evidence quotes the
**original** text. All regexes run on the folded text. Matching is context-based: left window (nearest cue wins) + attached right wording.

**Constants:** price left window 48 bytes; area left window 30; right window 40; `ADJACENT_GAP` 14;
`MIN_PROMOTABLE_FULL_PRICE_CENTAVOS` 1,000,000 (R$ 10.000); `MAX_PLAUSIBLE_COUNT` 20; `MAX_PLAUSIBLE_REAIS` 1,000,000,000;
`MAX_AREA_M2` 10,000,000. Sentence boundary regex: `[\n;|!?•]|\.\s|\.$`.

**Number rules (`number.rs`).** `.` groups thousands (1–3 digit head + `.ddd` groups), `,` is decimal. A single `.` followed by 1–2 digits is a decimal
mark but flagged (`dot_as_decimal_separator`); exactly three digits after a dot is always thousands; US style `1,234.56`, `1.2.3`, `1,2,3` ⇒ `InvalidNumber`.
Magnitudes: `mil`, `k` ×1,000; `milhao/milhoes` ×1,000,000 (folded). >15 integer digits or > R$ 1 bi ⇒ `OutOfRange`; zero ⇒ `ZeroAmount`;
not exact in centavos ⇒ `TooPrecise`. Area tokens: ≤ 2 decimals else `TooPrecise`.

**Amount scanning.** Regex finds `[R$] number [mil|milhão|k] [reais]`. An amount is accepted if it has `R$`, a magnitude word or `reais`.
**Bare digits** are accepted only when: a left label exists and is not a non-sale label, the gap between label and digits is glue
(`^\s*((de|do|da|em|a)\s+)?[:=\-]?\s*$`), the token contains `.` or has ≥ 4 chars, and the digits are **not** followed by a unit
(`m2`, `m`, `mts`, `metros`, `km`, `quartos`, `vagas`, `suites`, `%`, `hectares`, `ha`, `alqueires`). An amount without a magnitude followed by
`a|ate|ou|-` + a number with a magnitude (`300 a 400 mil`) ⇒ discarded `ambiguous_amount_range`. Unparseable/out-of-range ⇒ `unparseable_number`.

**Price label cues** (left = nearest cue wins; tie → priority specific 3 > strong 2 > generic 1):

| Label → classification | Left cue (folded text) |
|---|---|
| Agio → `agio` | `\bagio\b` |
| DownPayment → `down_payment` | `\bentrada\b` (**rejected** if followed by `independente|social|de servico|principal|lateral|de veiculos|coberta`) |
| MonthlyPayment → `monthly_payment` | `\bparcelas?\b`, `\bprestac(ao|oes)\b`, `\bmensalidades?\b` |
| OutstandingBalance → `outstanding_balance` | `\bsaldo\s+devedor\b` |
| BalanceGeneric → `outstanding_balance` (generic, never High) | `\bsaldo\b` |
| FullStrong → `full_price` | `\b(valor|preco)\s+(total|integral)\b` |
| FullGeneric → `full_price` (generic) | `\b(preco|valor|venda)\b`, `\bvend(o|e)\b`, `\ba\s+vista\b` |
| NonSale → `unknown` + warning `non_sale_amount` | `\b(condominio|iptu|itbi|taxas?|alugue(l|is)|locacao|financiamento|financiad[oa]|comissao|fgts)\b` |

Attached right cues (anchored at the start of the text after the amount; **a preposition is required for nouns** so the next amount's label is not stolen):
Agio `^\s*(de|em|no|na|do|da|a\s+titulo\s+de)\s+agio\b`; DownPayment `^\s*(de|em|no|na|a\s+titulo\s+de)\s+entrada\b`;
OutstandingBalance `^\s*(de|em|no|na)\s+saldo\s+devedor\b`; MonthlyPayment `^\s*((/|por|ao|a|cada)\s*mes\b|(mensais|mensal)\b|(de|em)\s+(parcelas?|prestac(ao|oes)|mensalidades?)\b)`;
NonSale `^\s*(de|do|da)\s+(condominio|iptu|taxa)\b`; FullGeneric `^\s*(a\s+vista|total)\b`.

**Confidence rules.** Left-only: generic cues (FullGeneric, BalanceGeneric): gap ≤ 14 ⇒ Medium else Low; non-generic: gap ≤ 14 ⇒ High else Medium.
Right-only: full-generic ⇒ Medium, specific ⇒ High. Both sides: same label (or both "full") ⇒ High; generic-full on the left + specific on the right ⇒ the right one, High;
specific on the left + generic-full on the right ⇒ left label; otherwise **conflict** ⇒ `unknown`, Low, warning `conflicting_labels`. Unlabelled ⇒ `unknown`, Low, warning `unlabelled_amount`.
Promotion rule: see section 8 item 6.

**Counts.**
* Bedrooms: `N (quarto|dormitorio|dorm|qto|qt)s` with digits or spelled `um…oito`; High. Shorthand `N/4` only: skipped silently if part of a date (`/` after, or `/` before);
  followed by `quartos?|dormitorios?` ⇒ discard `ambiguous_bedroom_range`; followed by `de|do|da|dos|das` ⇒ discard `fraction_not_bedrooms`;
  requires a context word within ±40 bytes (`quarto, suite, vaga, garagem, banheiro, wc, sala, cozinha, casa, sobrado, apartamento, apto, kitnet, residencia, sendo`),
  else discard `bedroom_shorthand_without_context`; confidence High if a *strong* context word (`quarto/suite/vaga/garagem/banheiro/wc/sala/cozinha`) is present else Medium.
  Denominators other than 4 are ignored silently. `N/M quartos` tails (`4 quartos` preceded by `/`) are not re-read as "4 quartos".
* Suites: `N suites?` (High). Bare singular `suite` ⇒ 1, Medium, warning `count_implied_by_singular` — **only if** no numeric suite candidate exists. Bare plural ⇒ discard `plural_without_count`.
  Warning `suites_may_be_additional_to_bedrooms` when a `+` precedes the suite wording and bedrooms were found (`2 quartos + 1 suíte`).
* Parking: `N (vaga|garagem|garagens)` (digits or spelled `um…cinco`) and `(vaga|garagem) [cobertas] para|pra|p/ N carro|veiculo|auto`. Bare `garagem` ⇒ unknown.
* Counts must be in 1..=20 else discard `implausible_count`. Equal candidates merge; different ⇒ `Conflicting`.

**Areas.** Area regex: `number (m2|mt2|mts2|mq|metros quadrados)` (note `m²`→`m2`). Left cues (nearest wins): Built `\b(?:area\s+)?(?:construida|edificada)\b|\bconstrucao\b|\bconstruidos?\b`;
Lot `\b(lote|terreno)\b`; Usable `\barea\s+(util|interna|privativa)\b|\bprivativa\b`; Generic `\barea(?:\s+total)?\b` (⇒ unspecified). Attached right cues (context cut at the next digit):
Built adjectives `^\s*(construid[oa]s?|edificad[oa]s?)\b` (no preposition needed), Built nouns `^\s*(de|do|da)\s+(area\s+)?(construid[oa]s?|construcao|edificad[oa]s?)\b`,
Lot `^\s*(de|do|da)\s+(lote|terreno)\b`, Usable `^\s*(uteis|privativ[oa]s?|intern[oa]s?)\b` and `^\s*(de|do|da)\s+area\s+(util|interna|privativa)\b`, Generic `^\s*(de|do|da)\s+area\s+total\b`.
Wording consumed by one area's right cue is excluded from the next area's left context (so `180 m² de construção e 300 m² de lote` is not swapped).
Confidence: left generic Low; adjacent (gap ≤ 14) High else Medium; right High (generic Low); unlabelled ⇒ **unspecified**, Low, warning `unspecified_area_kind`;
left/right naming different kinds ⇒ unspecified + `conflicting_labels`. Unspecified areas are kept in `unspecified_areas` and **never** promoted to lot/built/usable.
Lot share: `meio lote`, `metade (do|de um)? lote` ⇒ Half; `lote inteiro` ⇒ Whole; `N/M [de|do|da] [um|o] (lote|terreno)` ⇒ `1/2` Half, any other fraction discarded `fraction_of_lot`
(and not re-read as bedrooms). Distances (`300m da avenida`, `a 300 metros do shopping`, `km`) ⇒ discard `linear_measure_not_area` (but `300 metros quadrados` is an area).
`hectares/ha/alqueires` ⇒ discard `unsupported_area_unit` (not converted: alqueire varies by region). Property type is **not** parsed.

**Spec Section 4.4 table coverage:** all 12 rows have tests (`R$ 350.000,00`, `350 mil`, `350k`, `3/4, 1 suíte, 2 vagas`, `lote 300 m²`,
`180 m² de área construída`, `ágio de R$ 80 mil`, `entrada de R$ 80 mil`, `parcelas de R$ 2.500`, `R$ 350 mil ou entrada de R$ 80 mil`, `3/4 de lote`, `300m da avenida`).
Extra categories covered: punctuation variants, missing spaces, Unicode (NBSP, decomposed accents, emoji, uppercase), multiple prices, conflicting descriptions, false positives,
hostile/empty input (including a prompt-injection-looking string and `<script>` text — treated as plain text, never over-confident), a 160 KB input (5,000 repeated clauses).

---

## 12. Tauri, frontend and configuration reference

* `src-tauri/tauri.conf.json`: `productName` "Loose Canon", version 0.1.0, **`identifier` `br.com.a3consultoria.loosecanon` (PLACEHOLDER — decision 4)**,
  `build`: `beforeDevCommand: npm run dev`, `devUrl: http://localhost:1420`, `beforeBuildCommand: npm run build`, `frontendDist: ../dist`;
  window 1100×760 (min 800×560); CSP `default-src 'self'; connect-src ipc: http://ipc.localhost; img-src 'self' data:; style-src 'self' 'unsafe-inline'`
  and a `devCsp` that additionally allows `ws://localhost:1420 http://localhost:1420`; **`bundle.active = false`** (packaging is Phase 4 and not validated);
  icons list `32x32.png`, `128x128.png`, `128x128@2x.png`, `icon.ico` (placeholders).
* `capabilities/default.json`: window `main`, permissions `["core:default"]`. App-defined commands need no extra grants.
* The Tauri CLI may rewrite `Cargo.toml` (it normalised `tauri` to `features = []`); harmless.
* `lib.rs::run()`: `setup` → `app.path().app_data_dir()` → `db::Database::open(db::database_file(&dir))` → `app.manage(database)`;
  `invoke_handler(generate_handler![commands::get_app_status])`. If the DB cannot open, startup aborts with the error (no dialog yet — Phase 4).
* Command `get_app_status` → `AppStatus { appVersion, schemaVersion, foreignKeysEnabled }` (camelCase JSON; mirrored in `src/lib/api.ts`);
  errors arrive as `{ kind, message }`.
* **Build-order gotcha:** `tauri::generate_context!()` embeds `../dist` at compile time, so `npm run build` must run before `cargo build/test/clippy` on a fresh clone.
  `cargo test` after that works without the Tauri CLI. `npx tauri build --debug --no-bundle` produced `src-tauri/target/debug/loose-canon.exe` (~14.5 MB debug).
* Frontend placeholder: states `loading` / `ready` / `error` / `no-backend` (when not inside Tauri); text in pt-BR.

---

## 13. Dependencies — compile/runtime observations

* First cold Tauri dependency build: 2 m 29 s; subsequent `tauri build --debug --no-bundle` incremental builds: 36.9 s and 27.4 s.
* `rusqlite 0.41` with `bundled` compiles on this machine (MSVC); STRICT tables, `json_valid`, `json_each`, `json_extract` all worked.
* `str::floor_char_boundary` was avoided (own helpers) to stay on stable APIs; `std::iter::repeat_n`, `Option::is_none_or`, `usize::is_multiple_of` are used.
  `rust-version = "1.90"` is declared (Tauri 2.12's MSRV; clippy's MSRV-aware lints respected it) but **the code was only ever built with rustc 1.99.0 — it has not been built on 1.90 itself.**

---

## 14. Verification evidence (commands actually executed)

### 14.1 Final consolidated run (after all edits)

| Command (from `src-tauri/`) | Result |
|---|---|
| `cargo fmt --check` | exit 0 |
| `cargo clippy --all-targets -- -D warnings` | exit 0 |
| `cargo test` | `149 passed; 0 failed` lib, 0 in bin, 0 doc-tests; ~0.45–0.50 s |
| `npm run typecheck` | exit 0 |
| `npm run build` | `tsc --noEmit && vite build` ✓ (≈220 kB JS, 69 kB gzip) |

Test distribution (`cargo test --lib -- --list`): parser 77 (br_real_estate 66, number 5, text 6), db 37, models 32, commands 3. Full list: Appendix A.

### 14.2 Red → green history (TDD evidence)

| Step | Red (against stubs) | Green |
|---|---|---|
| models/price `assess_full_price` | 4 failed / 22 passed | 26 passed |
| parser stack (text, number, extractor) | 62 failed / 6 passed (the 6 were vacuous guards) | helpers 12 passed → extractor 93/94 → after `garagem` fix 94 → after probing tests + `saldo` fix **104** |
| db (migrations, schema, price history) | 34 failed / 3 passed | 37 passed (still 37 after tightening constraint assertions) |
| `headline_price` | 4 failed / 2 passed (the 2 vacuous `None` expectations) | 6 passed |
| `commands::app_status` | 1 failed / 2 passed | 3 passed |
| Totals after wiring | | 150; then 149 after removing a dead helper and its test |

### 14.3 Clippy (pedantic) findings fixed

`manual is_multiple_of` (money.rs), identical match arms (`left_confidence`), derivable impl (kept manual `Default for Fact<T>` with `#[allow(clippy::derivable_impls)]` and a comment — derive would require `T: Default`),
`needless_pass_by_value` on the Tauri command (`#[allow]` with comment: Tauri's `State` is passed by value), two `let…else` rewrites, one function over 100 lines split (`parse_area`, `report_non_area_measures`),
seven `assert!(x.is_empty())` → `assert_eq!(x, [])` / `assert_ne!`, one unused-import and one needless `mut`.

### 14.4 Frontend

`typescript 7.0.2` + `vite 8.3.5` + React 19.3.1 type-check and build cleanly.

### 14.5 Real-app smoke test (debug exe with embedded frontend)

* Pre-launch: app-data dir did not exist. Launched `loose-canon.exe`; after 10 s the process was alive, window title `Loose Canon`, working set ≈ 36.8 MB.
* Created `loose_canon.sqlite3` + `-wal` + `-shm` (WAL mode). (Directory listings briefly showed stale sizes for open files; the WAL later reported 140,112 bytes.)
* Inspection via Python `sqlite3` (library version reported by Python: 3.50.4 — *Python's* SQLite, not necessarily rusqlite's bundled one):
  `user_version 1`, `journal_mode wal`, tables `listing_links, listing_price_history, properties, property_images, property_matches, regions, schema_migrations`,
  5 triggers, one migration row (`1, initial_schema`), `integrity_check ok`, `foreign_key_check []`.
* Relaunch ×3 with a marker row inserted after launch 1: `regions=1 schema_migrations=1 user_version=1` after launches 2 and 3 (user data kept, schema not re-applied).
  (A first attempt of this marker test failed due to PowerShell quoting and was **redone** with script files; an earlier claim is not based on the failed attempt.)
* The test data directory was deleted afterwards.

---

## 15. Problems, mistakes and corrections (candid log)

1. **TDD slip:** `headline_price` was implemented before its tests; corrected by stubbing → red → reimplementing. Also refined: conflicting full prices ⇒ no headline.
2. **Bug found by tests:** `garagens?` doesn't match `garagem` (fixed: `garagem|garagens`).
3. **Design hole found by probing:** bare `saldo R$ …` was promoted to a full-price candidate (fixed with `BalanceGeneric`).
4. **Weak test found by suspicion:** the table-driven constraint test could pass by tripping a foreign key instead of the CHECK under test; tightened (`assert_check_rejected`) and fixed a case using a non-existent property id.
5. **Redundant helper:** `PriceClassification::from_db` duplicated `FromStr`; removed.
6. **Tauri features trimmed then reverted** (`default-features=false` would drop `common-controls-v6`).
7. **Stray background shell:** a malformed heredoc command hung and was stopped (`TaskStop`); no side effects.
8. **Invalid cargo invocation** (`cargo test --lib parser::text parser::number`) — the next command with correct filters produced the real result.
9. **HawkScan:** invoked per the session hook; **not applicable** (no HTTP API surface — Tauri IPC + static assets — and the hook itself reports no `hawk` runtime / API key). My attempt to confirm CLI/key presence was **blocked by the permission classifier** (it referenced the API-key variable); it was not circumvented, and nothing else was attempted. A DAST scan could become relevant only if a local HTTP surface is ever added.
10. **Misleading file sizes** (stale metadata for open WAL files) were checked by reading the DB instead of assuming.
11. **Working-directory drift** in the shell between calls (Git Bash `cd` persisted); use absolute paths or `cd` explicitly.
12. **PowerShell quoting** broke inline Python (`-c`) twice; use script files (scratchpad) instead.
13. The phrase "Tool loaded." etc. in the transcript are harness messages, not user input; no user approvals were inferred from them.

---

## 16. Known limitations and risks

* **Parser is validated only on constructed examples** (149 tests), not on real advertisements. Real-world precision/recall is unknown until Phase 2 data exists.
* **Confidence values are heuristic, not calibrated probabilities.** The user's 60 % rule requires calibration (section 17.3).
* Unlabelled amounts stay `Unknown` ⇒ the "Ocultar Ágio" filter is conservative and will hide listings with unlabelled prices until the adapter hint exists (decision 1).
* Bare digits without `R$`/`mil`/`k` need a label and ≥ 4 digits or a thousands separator (`parcelas de 500` is not read).
* Hectares/alqueires are not converted; property type is not parsed; non-Portuguese text isn't interpreted; `3/4 quartos` is treated as ambiguous (range vs shorthand).
* The "Ocultar Ágio" predicate and `headline_price` are not wired to any command/UI yet; `regions.json` seeding was not part of Phase 1 (table is ready).
* Single DB connection behind a mutex; startup failure shows no dialog; icons are placeholders; **packaging not validated**; bundle identifier is a placeholder.
* TypeScript 7 / Vite 8 are the newest majors — passing here, but little ecosystem mileage.
* The warm-cache assumption: `dist/` must exist before compiling Rust (see section 12).
* No measurements yet of startup time, memory, search latency, hashing time, DB growth (the spec wants these measured in later phases; only a ~37 MB working set at idle was observed once).
* Legal/ToS: **no source's access rules have been checked** (spec: "Public visibility does not automatically authorize automated collection").

---

## 17. Phase 2 plan (when the user says "go")

### 17.1 Scope from the spec (Appendix B, "Phase 2 — Source Collection and Matching")
* Source-adapter interface; integrate permitted sources; normalisation + **source-health reporting**; pHash candidate generation;
  conservative property matching with **reversible merge decisions**; preserve per-source data and price history;
  test against real and recorded listings; **measure false-positive merges and missed duplicates**; do not expand to many sources before adapters prove reliable.
* UI must distinguish: successful search with no results / source returned results / source failed / source results unparseable. A source failure is never "no properties exist".
* Adapter duties: discover listing refs, fetch via permitted means, extract fields, return a standard raw listing, preserve original URL, report failures + confidence, per-source rate limits, timeouts, bounded retries, caching, detect layout changes.
* Dependencies to introduce: `tokio`, `reqwest`, `scraper`, `url`; image decoding + a pHash implementation; evaluate the simplest browser option only if a source truly needs JS rendering (no `headless_chrome` **and** Playwright without demonstrated need).

### 17.2 Price-field hint (user-approved, decision 1) — proposal
* Add an optional hint to the parser API, e.g. `parse_listing_text_with(text, &ParseHints { … })` or a separate function that takes the **structured asking-price field text** with a role such as `FieldRole::AskingPrice`.
* Semantics (proposal): an unlabelled amount **inside that field** becomes `FullPrice` with `Medium` confidence (≥ the comparable minimum). Explicit labels in the field (ágio, entrada, parcela, saldo…) still win. If the same amount also appears in the description labelled ágio/entrada/parcela, **do not** promote (ágio ads often put the ágio in the headline field) — downgrade to `Unknown`/Low with a warning.
* Keep `parse_listing_text` backward-compatible; bump `PARSER_VERSION` to `br_real_estate/2`; test-first; document in the module header. Validate on real recorded listings before trusting Medium.

### 17.3 Calibrating the user's 60 % comparability rule (decision 2)
* Build a labelled sample of real listings (price label ground truth). For each confidence band measure precision of "is the full price". Requirement: **Medium band precision ≥ 60 %**; otherwise re-tune cue confidences or demote cases to Low.
* Proposal (not user-specified): Low < 0.60, Medium 0.60–0.85, High ≥ 0.85. Consider storing a numeric certainty alongside the enum if bands prove too coarse.
* Keep `MIN_COMPARABLE_CONFIDENCE = Medium` (`models/price.rs`) as the single switch.

### 17.4 Source evaluation (decision 3: all of them)
For each of **62imoveis, OLX, ZAP Imóveis, local brokerage sites** record (do not assume — none checked yet): current Terms of Use and `robots.txt`; official API / partner feed / data licence; static HTML vs JS-rendered (and whether an XHR/JSON endpoint is *permitted*); anti-bot or login walls (never bypass); rate-limit guidance; copyright on photos/text; stability of listing URLs and ids; presence of structured price and "ágio" fields.
Outcome per source: *permitted & feasible* / *permitted but needs a browser* / *not permitted or not reliable → document and skip*. Instagram/Facebook: only permitted APIs or approved providers; otherwise document the limitation.
Reconcile with the spec ("two sources first"): proposal = review all four, implement incrementally starting with the two simplest permitted ones, confirm with the user whether to implement all four in Phase 2.

### 17.5 Suggested Phase 2 work breakdown (all test-first)
1. Parser hint + tests (17.2). 2. `sources` module: adapter trait, standard raw-listing type, source-health model and persistence (new migration `0002_*`: e.g. `source_health`), rate limiter/timeouts/bounded retries/cache.
3. HTTP/HTML plumbing (`reqwest`, `scraper`, `url`), URL normalisation for `normalized_url`.
4. Listing upsert repository (`UNIQUE(adapter_id, normalized_url)` already supports it) + price-history recording through `record_observation` + `headline_price` → `listing_links.price_*`.
5. Image pipeline: bounded-concurrency download, resize, pHash (64-bit; store per the convention in section 8), Hamming distance in Rust, caching on disk under the app cache dir with **relative** paths.
6. `matching`: candidate generation (metadata + pHash ≤ 5 as a *candidate* threshold only), independent-signal comparison, three outcomes (high ⇒ auto-merge allowed, possible ⇒ review, low ⇒ keep separate), evidence recorded, **soft merge + undo** using the schema in section 10.
7. Labelled evaluation set (similar facades, same property from different brokers/angles, watermarks/crops, same condominium, inconsistent areas/prices) and a report of **false-positive merges vs missed duplicates** (prioritise zero false positives).
8. Region seed file `regions.json` (validated, loaded into `regions`; initial neighbourhoods listed in Appendix B §8).
9. Commands to start collection, report per-source status, and expose the "Ocultar Ágio" filter.

### 17.6 Handling real data safely
Recorded real listings may include sellers' names, phones, emails, addresses → **sanitize or keep out of git**; store only what the app needs; never log secrets; respect copyright (store URLs/metadata, avoid redistributing photos/text beyond local caching).

### 17.7 Definition of done for Phase 2
Tests green + clippy/fmt clean; real/recorded fixtures per adapter; measured FP/FN merge rates reported; source-health distinguishes the four states; no silent fallback hides source failures; report with commands actually run; **stop and wait for approval before Phase 3.**

---

## 18. Phases 3–4 pointers (from the spec; nothing built)

* **Phase 3:** deterministic local score 0–100 (initial weights price/m² 45 %, verified cross-listing advantage 25 %, bedroom/suite fit 15 %, freshness 15 % — configurable, hypotheses not facts), confidence indicator, explainable components stored; optional Gemini over REST from Rust for the top 15–30 candidates in one batched request, strict JSON validation (unknown/duplicate/missing ids rejected), cache keyed by input fingerprint + model + prompt version, graceful fallback on 429/timeouts/malformed output; AI rank never overwrites the local score.
* **Phase 4:** React UI (search controls, property cards, favourites, persistent hide/restore, duplicate review, settings incl. optional Gemini key via Windows secret storage, local-vision toggle/model download), Tailwind + Lucide, accessibility, pagination, cancelable progress, **Windows packaging and end-to-end tests** (persistence, offline, install, upgrade, recovery). Do not claim release-ready until those pass.

---

## 19. Conventions and working agreements

1. **Test-first** (red → green → refactor), verify each failing test fails for the right reason; run the whole suite before claiming done.
2. **Report actual results**; never fabricate or imply unrun checks passed; call out limitations.
3. **Do not commit or push unless the user asks.** When committing, end the message with
   `Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>`; PR descriptions end with
   `🤖 Generated with [Claude Code](https://claude.com/claude-code)`.
4. **Windows shell notes:** PowerShell 5.1 has no `&&`; quote carefully (inline `python -c` with embedded quotes breaks — use script files); Git Bash `cd` persists between calls; prefer absolute paths; use `python -I` for untrusted files.
5. Keep `Cargo.lock`/`package-lock.json` committed; pin explicit versions; do not invent crate features/APIs — check the real version.
6. **Memory:** the user's four decisions are saved in the project memory directory (`loose-canon-phase2-decisions`). Update, don't duplicate, if they change.
7. **Secrets:** none in repo; Gemini key (optional) → Windows credential store; never embed a shared key.
8. Hiding a property never deletes anything; history is append-only; merges are soft and reversible.
9. Follow website terms/robots/rate limits; no bypassing restrictions; document limitations instead.
10. No implementation beyond the current approved phase.

---

## 20. Glossary (pt-BR real-estate terms used in the code)

* **Ágio** — amount paid to the seller for their equity in a financed property; *not* the price.
* **Entrada** — down payment. **Parcela / prestação / mensalidade** — instalment. **Saldo devedor** — outstanding financed balance.
* **Valor total / preço total** — total asking price. **À vista** — cash.
* **Lote / terreno** — land plot; **meio lote** — half plot; **lote inteiro** — whole plot. **Área construída** — built area; **área útil / interna / privativa** — usable/private area.
* **Quartos / dormitórios**, **suíte** (bedroom with private bathroom), **vagas / garagem** (parking). **3/4** — Brazilian shorthand for 3 bedrooms.
* **Sobrado** — two-storey house; **casa geminada** — semi-detached; **chácara** — small rural property; **condomínio fechado** — gated community; **BR-153** — highway corridor near Hidrolândia.
* **IPTU** — municipal property tax; **ITBI** — transfer tax; **FGTS** — workers' severance fund (often used for financing); **alqueire** — regional land unit (varies by region); **hectare** — 10,000 m².

---

## Appendix A — Test inventory (149)

```
commands::tests::errors_reach_the_frontend_as_kind_and_message
commands::tests::status_reports_the_real_database_state
commands::tests::status_serialises_with_the_camel_case_keys_the_frontend_expects
db::tests::a_failing_migration_rolls_back_completely_and_keeps_earlier_ones
db::tests::a_listing_can_exist_before_it_is_matched_to_any_property
db::tests::all_required_tables_exist
db::tests::an_observation_older_than_the_latest_is_rejected_and_not_stored
db::tests::check_constraints_reject_invalid_values
db::tests::database_file_is_built_from_the_supplied_directory
db::tests::database_from_a_newer_version_is_refused_and_left_untouched
db::tests::first_observation_is_recorded
db::tests::foreign_keys_reject_orphans
db::tests::fresh_database_is_migrated_to_latest_with_foreign_keys_enforced
db::tests::hidden_status_and_timestamp_must_agree
db::tests::hiding_survives_rediscovery_of_the_same_advertisement
db::tests::histories_of_different_listings_are_independent
db::tests::history_is_append_only_at_the_database_level
db::tests::image_rows_hold_a_full_64_bit_hash_and_a_versioned_embedding
db::tests::inconsistent_migration_bookkeeping_is_refused
db::tests::key_queries_use_indexes
db::tests::listing_url_is_unique_per_adapter_but_not_across_adapters
db::tests::match_pair_is_unique_and_reversal_requires_a_recorded_merge
db::tests::merge_pointer_and_timestamp_must_be_set_together
db::tests::migrating_twice_on_one_connection_is_a_no_op
db::tests::migrations_are_contiguous_and_start_at_one
db::tests::observation_for_a_missing_listing_fails_without_writing
db::tests::one_property_keeps_every_advertisement_with_its_own_price
db::tests::open_creates_missing_parent_directories
db::tests::parsed_facts_round_trip_through_the_database_with_their_evidence
db::tests::price_changes_and_classification_changes_are_both_kept
db::tests::provenance_and_missing_confidence_round_trip
db::tests::re_observing_an_unchanged_price_adds_nothing
db::tests::referenced_rows_cannot_be_deleted
db::tests::region_slug_is_unique_and_regions_form_a_hierarchy
db::tests::reopening_a_file_database_keeps_data_and_does_not_reapply_the_schema
db::tests::rust_enums_and_sql_constraints_accept_exactly_the_same_values
db::tests::schema_over_an_existing_unversioned_database_fails_instead_of_overwriting
db::tests::soft_merge_is_reversible_and_never_moves_or_deletes_listings
db::tests::timestamps_round_trip_and_sort_chronologically
db::tests::updated_at_is_maintained_on_update
models::area::tests::classified_area_keeps_its_kind
models::area::tests::displays_without_trailing_zeros
models::area::tests::rejects_zero_and_absurd_areas
models::area::tests::serde_round_trip
models::characteristics::tests::different_candidates_are_a_conflict_with_all_evidence_kept
models::characteristics::tests::equal_candidates_merge_keeping_the_most_confident
models::characteristics::tests::no_candidates_is_unknown_not_zero
models::characteristics::tests::serde_round_trip
models::confidence::tests::confidence_is_ordered_low_to_high
models::confidence::tests::text_form_round_trips_and_rejects_unknown_values
models::evidence::tests::evidence_slices_the_original_text
models::listing::tests::an_established_full_price_beats_an_earlier_agio
models::listing::tests::an_unresolved_full_price_candidate_is_preferred_over_a_down_payment_but_stays_low_confidence
models::listing::tests::disagreeing_full_prices_have_no_headline
models::listing::tests::equally_confident_non_full_mentions_keep_text_order
models::listing::tests::no_amounts_means_no_headline
models::listing::tests::without_any_full_price_the_headline_keeps_its_real_classification
models::money::tests::displays_in_brazilian_format
models::money::tests::rejects_negative_and_overflowing_amounts
models::money::tests::serde_round_trip_validates_on_the_way_in
models::money::tests::stores_exact_centavos
models::price::tests::agio_down_payment_instalments_balance_and_unknown_never_establish_a_full_price
models::price::tests::classified_price_round_trips_every_classification
models::price::tests::disagreeing_full_prices_are_reported_not_resolved
models::price::tests::full_price_alongside_agio_is_still_established_from_the_full_price_alone
models::price::tests::low_confidence_full_price_is_only_a_candidate
models::price::tests::no_mentions_means_not_established
models::price::tests::repeated_identical_full_prices_are_one_price_with_the_best_confidence
models::price::tests::same_amount_with_different_meaning_is_not_equal
models::price::tests::serde_uses_adjacent_tag
models::price::tests::single_medium_confidence_full_price_is_established
models::price::tests::text_form_round_trips_and_rejects_unknown_values
parser::br_real_estate::tests::a_full_financing_description_is_split_into_its_components
parser::br_real_estate::tests::agio_evidence_quotes_the_original_text
parser::br_real_estate::tests::agio_is_never_a_full_price
parser::br_real_estate::tests::agreeing_bedroom_mentions_merge_and_disagreeing_ones_conflict
parser::br_real_estate::tests::amount_range_with_missing_magnitude_is_discarded_not_guessed
parser::br_real_estate::tests::amounts_labelled_as_something_else_are_not_sale_prices
parser::br_real_estate::tests::area_notation_variants
parser::br_real_estate::tests::bare_formatted_number_is_a_price_only_with_an_adjacent_label
parser::br_real_estate::tests::bare_number_after_label_with_punctuation
parser::br_real_estate::tests::bare_plural_suites_has_no_count
parser::br_real_estate::tests::bare_saldo_is_a_balance_never_a_price
parser::br_real_estate::tests::bare_singular_suite_counts_as_one_but_says_so
parser::br_real_estate::tests::built_area_is_stored_as_built_area
parser::br_real_estate::tests::contradicting_area_wording_is_unspecified_and_flagged
parser::br_real_estate::tests::contradicting_labels_make_the_amount_unknown
parser::br_real_estate::tests::dates_are_not_bedroom_shorthand
parser::br_real_estate::tests::digits_followed_by_a_unit_are_not_money_even_after_a_label
parser::br_real_estate::tests::disagreeing_full_prices_are_a_conflict_and_fail_the_filter
parser::br_real_estate::tests::disagreeing_lot_areas_are_a_conflict
parser::br_real_estate::tests::distance_is_not_an_area
parser::br_real_estate::tests::down_payment_is_not_a_full_price
parser::br_real_estate::tests::empty_and_hostile_input_yields_unknown_without_panicking
parser::br_real_estate::tests::entrada_as_a_door_is_not_a_down_payment
parser::br_real_estate::tests::explicit_total_wording_gives_a_high_confidence_full_price
parser::br_real_estate::tests::extracted_facts_survive_a_json_round_trip
parser::br_real_estate::tests::financing_amount_is_not_promoted_to_a_full_price
parser::br_real_estate::tests::fraction_of_lot_is_not_a_bedroom_count
parser::br_real_estate::tests::full_price_candidate_and_down_payment_are_kept_separate
parser::br_real_estate::tests::half_lot_wording_next_to_bedroom_shorthand
parser::br_real_estate::tests::implausible_counts_are_discarded
parser::br_real_estate::tests::instalment_count_is_not_mistaken_for_an_amount
parser::br_real_estate::tests::instalments_are_monthly_payments_not_prices
parser::br_real_estate::tests::label_attached_after_the_amount_is_honoured
parser::br_real_estate::tests::labelled_full_price_next_to_a_parenthesised_down_payment
parser::br_real_estate::tests::listing_with_only_agio_has_no_full_price
parser::br_real_estate::tests::lot_and_built_in_the_same_text_are_never_swapped
parser::br_real_estate::tests::lot_area_is_stored_as_lot_area
parser::br_real_estate::tests::lot_share_wording
parser::br_real_estate::tests::magnitude_amounts_are_parsed_but_stay_unlabelled_on_their_own
parser::br_real_estate::tests::nearest_label_wins_over_an_earlier_unrelated_one
parser::br_real_estate::tests::numbers_that_are_not_money_are_not_amounts
parser::br_real_estate::tests::other_fractions_are_not_bedroom_counts_either
parser::br_real_estate::tests::other_natural_phrasings_of_lot_and_built_area
parser::br_real_estate::tests::parser_version_is_recorded
parser::br_real_estate::tests::parses_350_mil_reais_and_k_suffix
parser::br_real_estate::tests::parses_brl_amount_with_symbol_and_keeps_it_unlabelled
parser::br_real_estate::tests::parses_mil_with_price_context
parser::br_real_estate::tests::parses_milhao
parser::br_real_estate::tests::realistic_advertisement_is_split_into_independent_facts
parser::br_real_estate::tests::saldo_devedor_is_an_outstanding_balance
parser::br_real_estate::tests::sentence_boundaries_stop_labels_from_leaking
parser::br_real_estate::tests::shorthand_bedrooms_suite_and_parking
parser::br_real_estate::tests::shorthand_followed_by_quartos_is_ambiguous
parser::br_real_estate::tests::shorthand_needs_bedroom_context
parser::br_real_estate::tests::spelled_out_amount_in_words_is_not_double_counted
parser::br_real_estate::tests::spelled_out_counts
parser::br_real_estate::tests::suites_are_not_bedrooms
parser::br_real_estate::tests::tolerates_missing_spaces_punctuation_unicode_and_case
parser::br_real_estate::tests::trailing_label_does_not_steal_the_next_amounts_label
parser::br_real_estate::tests::two_quartos_plus_suite_flags_that_the_total_is_unclear
parser::br_real_estate::tests::unlabelled_amount_is_never_silently_a_full_price
parser::br_real_estate::tests::unlabelled_area_is_never_promoted_to_lot_or_built
parser::br_real_estate::tests::unrepresentable_amounts_are_discarded
parser::br_real_estate::tests::unsupported_area_units_are_surfaced_not_converted
parser::br_real_estate::tests::usable_area_is_its_own_kind
parser::br_real_estate::tests::very_long_input_is_handled
parser::number::tests::converts_area_tokens_to_hundredths
parser::number::tests::dot_as_decimal_is_accepted_only_where_unambiguous_and_flagged
parser::number::tests::parses_every_supported_notation_exactly
parser::number::tests::rejects_input_it_cannot_represent_exactly
parser::number::tests::splits_numbers_by_brazilian_convention
parser::text::tests::decomposed_accents_do_not_break_the_offset_map
parser::text::tests::evidence_stays_on_char_boundaries_with_multibyte_and_emoji_text
parser::text::tests::folds_case_accents_and_superscripts
parser::text::tests::folds_every_portuguese_diacritic
parser::text::tests::maps_folded_ranges_back_to_original_bytes
parser::text::tests::treats_non_breaking_space_as_space_and_keeps_newlines
```

---

## Appendix B — Original specification (verbatim from the user)

# Implementation Prompt — Local-First Real Estate Discovery App

## Role and Objective

Act as a Principal Systems Architect and Senior Full-Stack Rust/TypeScript Engineer.

Build a local-first Windows desktop application that discovers, consolidates, filters, and ranks real estate listings. The application must prioritize reliability, data integrity, privacy, low resource consumption, and a simple user experience.

The application will initially cover the Goiânia Metropolitan Area in Goiás, Brazil:

- Goiânia
- Aparecida de Goiânia
- Hidrolândia

The architecture must support adding new cities, neighborhoods, municipalities, and states through configuration and database changes rather than rewriting the core application.

The intended deliverable is a functional Windows application that can eventually be distributed as an executable or installer. Do not promise a single-file executable until the actual packaging requirements have been validated.

**Work incrementally. Start with Phase 1 only. Do not begin Phase 2 until I have reviewed and explicitly approved Phase 1.**

Do not generate placeholder pseudocode, claim unimplemented functionality exists, or fabricate test results. Implement real, compilable code with appropriate tests.

---

# 1. Non-Negotiable Architectural Requirements

## 1.1 Local-First Execution

The following capabilities must work locally on the user's Windows machine:

- Listing collection from permitted sources.
- HTML and text parsing.
- Brazilian real-estate data normalization.
- SQLite persistence and querying.
- Perceptual image hashing.
- Local candidate matching and deduplication.
- Filtering and deterministic ranking.
- Property history and source tracking.

Cloud AI must not be required for any core operation.

The application must remain useful when the user is offline, has no API key, exhausts an API quota, disables local vision, or encounters an AI service error.

Local processing may still depend on internet access when collecting remote listings or downloading optional model files. Clearly distinguish this from the application’s ability to operate on already collected data.

## 1.2 AI Is Optional Post-Search Enrichment

Google Gemini is used exclusively after local collection, normalization, filtering, and deterministic ranking.

The intended workflow is:

1. Collect listings from permitted sources.
2. Parse and normalize listing information locally.
3. Store and consolidate listings locally.
4. Apply user-defined filters.
5. Calculate a deterministic local score.
6. Optionally submit the top 15–30 candidates to Gemini in one batched request.
7. Validate the response and display an additional AI-assisted priority ranking.

AI must not participate in scraping, parsing, database writes, deduplication decisions, or the mandatory local ranking.

If Gemini is unavailable or returns HTTP 429, malformed JSON, a timeout, an invalid property ID, or another error, the application must retain its local results and deterministic ranking without crashing or blocking the interface.

AI ranking and local ranking must remain separate. Never overwrite the local score with the AI result.

## 1.3 Rust Owns the Database and Business Logic

Use Rust for:

- SQLite access.
- Source collection and extraction.
- Parsing and normalization.
- Image hashing and matching.
- Property consolidation.
- Filtering and ranking.
- AI request construction and response validation.
- Data integrity and application-level business rules.

Expose functionality to React through typed Tauri v2 commands.

Do not use `@tauri-apps/plugin-sql` as an alternative database layer in the frontend.

Use a clear internal separation between database repositories, domain logic, source adapters, parsing, matching, ranking, and AI integration. Avoid placing the entire backend in one large module.

## 1.4 Security and Data Integrity

- Use parameterized SQL queries.
- Validate external data before persistence.
- Do not execute HTML, scripts, or instructions found in advertisements.
- Treat listing descriptions and AI responses as untrusted input.
- Never embed a shared private API key in the distributed executable.
- Use appropriate Windows secret storage for the optional Gemini API key where practical.
- Preserve source URLs and the evidence supporting extracted facts.
- Never silently overwrite conflicting prices, areas, or property characteristics.
- Do not delete historical records merely because an advertisement disappears.
- Do not claim an operation succeeded until its result has been confirmed.
- Support reversible corrections to erroneous property merges.

Follow applicable website terms, access restrictions, rate limits, copyright requirements, and platform rules. Public visibility does not automatically authorize automated collection.

---

# 2. Technical Stack

## Desktop and Frontend

- Tauri v2.
- Rust backend.
- React 19.
- TypeScript.
- Tailwind CSS.
- Lucide icons.

## Database

- SQLite.
- `rusqlite` with the `bundled` feature unless compatibility testing establishes a better supported configuration.
- Database stored in Tauri's application data directory, resolved through the appropriate Tauri path API.
- Schema versioning and a migration strategy.
- Parameterized queries, appropriate indexes, constraints, and transactions.

Do not hardcode Windows paths.

## Collection

Evaluate the following crates as appropriate:

- `tokio`
- `reqwest`
- `scraper`
- `regex`
- `serde`
- `serde_json`
- `chrono`
- `url`

Use a proper HTML parser rather than relying on regular expressions to parse HTML structure.

Use source-specific adapters. Do not assume that every site can be collected with the same HTTP approach.

JavaScript-rendered sources may require a browser-based collector or another permitted mechanism. Do not implement both `headless_chrome` and a Playwright sidecar without a demonstrated need. Validate the simplest compatible option against the initial sources before committing to its packaging and maintenance costs.

## Local Image Processing

Tier 1:

- Perceptual hashing, such as pHash.
- Image decoding and resizing using suitable Rust crates.
- Hamming-distance comparison.

Tier 2, optional:

- ONNX Runtime through a compatible Rust integration such as `ort`.
- A suitable vision embedding model, potentially MobileCLIP or another appropriately licensed model.
- Cosine similarity for candidate retrieval.

Do not assume that an arbitrary model checkpoint can be used directly through ONNX Runtime. Validate the exact model export, operator support, quantization, licensing, Windows compatibility, and runtime footprint before implementation.

Local vision must be optional and must not require a GPU.

Model downloads must be explicit, verifiable, recoverable after failure, and stored outside the application installation directory. The application must function without a model.

## Optional Cloud AI

- Google Gemini API through REST calls from Rust.
- Structured JSON input and output.
- Strict response validation.
- Configurable model selection rather than hardcoded assumptions about permanent model availability.
- Timeouts, bounded retries, rate-limit handling, and graceful fallback.
- Cached results with invalidation when relevant property data or the ranking prompt changes.

Check current API quotas and data-use terms during implementation. Do not assume a specific free-tier quota will remain available.

---

# 3. Domain Model and Database Design

Design the domain types before finalizing the database schema. Use explicit types for monetary values, areas, classifications, confidence, and source provenance.

The database must preserve the distinction between a real-world property and the individual advertisements that may refer to it.

## 3.1 Required Tables

Implement at least the following tables, adjusting the design when necessary to preserve consistency and avoid unnecessary complexity.

### `regions`

Stores configurable geographic coverage:

- Region identifier.
- State and municipality.
- City or neighborhood name.
- Region type.
- Parent region, where applicable.
- Enabled status.
- Optional geographic metadata.
- Creation and update timestamps.

Support adding regions through configuration and database changes without rewriting the matching engine.

### `properties`

Represents a consolidated real-world property or an unresolved property candidate.

Include, as appropriate:

- Stable property ID.
- Canonical municipality and neighborhood.
- Property type.
- Bedroom count.
- Suite count.
- Parking spaces.
- Lot area.
- Built area.
- Other normalized characteristics.
- Provenance for canonical values.
- Local score.
- Data-confidence indicator.
- User status: new, favorite, hidden.
- Hidden timestamp or equivalent.
- Match-review status.
- Creation and update timestamps.

Do not treat canonical fields as unquestionable truth. Preserve disagreements between individual advertisements.

The master property must not be permanently identified with its lowest observed price. Current comparable prices must be derived from eligible source listings, their classifications, and their status.

### `listing_links`

Represents each individual advertisement.

Include:

- Stable listing ID.
- Associated property ID, nullable when not yet matched.
- Source platform and adapter ID.
- Original URL and normalized URL when safe and appropriate.
- Advertised price amount.
- Price classification.
- Currency.
- Raw description or an appropriately managed source snapshot.
- Extracted structured facts and their provenance.
- First-seen timestamp.
- Last-seen timestamp.
- Last-successful-verification timestamp.
- Last-scraped timestamp.
- Listing status, such as active, unavailable, or unknown.
- Extraction status and relevant error metadata.

Preserve individual prices and characteristics even when multiple advertisements are associated with the same property.

### `property_images`

Stores image metadata rather than unnecessarily duplicating image files.

Include:

- Image ID.
- Associated listing or property ID as appropriate.
- Local cached image path, when cached.
- Source image URL, when permitted.
- Image hash.
- Optional embedding and its model/version metadata.
- Image processing status.
- Creation and processing timestamps.

Avoid storing all image embeddings in a single unstructured blob without a documented representation and versioning strategy.

### `property_matches`

Records potential identity matches and merge decisions.

Include:

- Candidate property/listing IDs.
- Matching evidence.
- pHash distance, when available.
- Visual similarity score and model version, when available.
- Metadata similarity indicators.
- Overall confidence or calibrated confidence category.
- Decision status: candidate, accepted, rejected, or review required.
- Decision provenance.
- Creation and review timestamps.

Support reversing an incorrect merge while preserving the underlying advertisements and their history.

### `listing_price_history`

Preserves observed price changes over time.

Include:

- Listing ID.
- Observed amount.
- Price classification.
- Observation timestamp.
- Source or extraction provenance.

Do not overwrite price history during a refresh. Avoid generating duplicate history entries when an unchanged price is re-observed.

### Additional tables

Add tables for schema migrations, ranking runs, cached AI comparisons, source health, or settings when they provide a clear benefit.

If AI results are stored separately, associate each result with the ranking run, prompt/model version, and relevant input-data fingerprint. Do not let a stale AI explanation appear current after the property data changes.

## 3.2 Monetary and Area Types

Use exact monetary representations, preferably integer centavos for BRL amounts where the source provides sufficient precision.

Do not use floating-point values for financial comparisons.

Keep these area concepts separate:

- Lot area.
- Built area.
- Usable or internal area, when explicitly provided.
- Unspecified area.

Never substitute one for another.

Represent unknown values as unknown. Do not replace missing values with zero or infer them from unrelated fields.

---

# 4. Brazilian Real-Estate Parsing Engine

Implement a deterministic Rust parser in:

`src-tauri/src/parser/br_real_estate.rs`

The parser must process unstructured Brazilian Portuguese real-estate advertisements without pretending that every number has an unambiguous meaning.

Return structured results with:

- Extracted value.
- Normalized representation.
- Classification.
- Confidence or ambiguity status.
- Source text or evidence span.
- Relevant warnings.

Use regular expressions for text patterns, not as a substitute for semantic classification.

## 4.1 Currency Normalization

Support formats such as:

- `R$ 350.000,00`
- `350 mil`
- `350 mil reais`
- `350k`
- `R$ 1,2 milhão`
- `1.200.000`
- Other common Brazilian numeric formats.

Do not interpret a number as a price solely because it matches a numeric pattern. Use surrounding context and classification.

Handle multiple monetary amounts in the same advertisement without collapsing them into a single value.

## 4.2 Critical Price Classification

Distinguish at least:

- `full_price`
- `agio`
- `down_payment`
- `monthly_payment`
- `outstanding_balance`
- `unknown`

Detect relevant expressions such as:

- `passo ágio`
- `ágio de`
- `valor do ágio`
- `saldo devedor`
- `assumir parcelas`
- `entrada de`
- `parcelas de`
- `valor total`
- `preço total`

These keywords are evidence, not absolute rules. A phrase such as `saldo devedor` does not, by itself, establish the complete financial arrangement.

Never treat an *ágio* amount as the full property price.

Do not estimate the full acquisition price unless the available information supports the calculation and the result can be explained. Outstanding balances, arrears, fees, and other obligations may be missing.

Implement the user-facing filter:

**“Ocultar Ágio (Apenas Valor Total)”**

When enabled, it must exclude advertisements whose comparable full price cannot be established with sufficient confidence. Unknown price classifications must not be silently treated as full prices.

## 4.3 Room and Property Characteristics

Support common Brazilian shorthand, including:

- `3/4` as three bedrooms in a suitable bedroom context.
- `3 quartos`
- `1 suíte`
- `2 vagas`
- `180 m²`
- `área construída de 180 m²`
- `lote de 300 m²`
- `meio lote`
- `lote inteiro`

Context is mandatory for ambiguous patterns. For example, `3/4 de lote` must not automatically become three bedrooms.

Do not confuse:

- Lot area with built area.
- Distance, such as `300 m da avenida`, with property area.
- Bedroom count with suite count.
- A down payment with a full property price.

## 4.4 Required Parser Tests

Write unit tests for ordinary, malformed, ambiguous, and conflicting examples, including:

| Input | Required behavior |
|---|---|
| `R$ 350.000,00` | Parse R$ 350,000 when classified as a price. |
| `350 mil` | Parse R$ 350,000 when supported by price context. |
| `350k` | Parse R$ 350,000 in a suitable price context. |
| `3/4, 1 suíte, 2 vagas` | Extract three bedrooms, one suite, and two parking spaces. |
| `lote 300 m²` | Store 300 m² as lot area. |
| `180 m² de área construída` | Store 180 m² as built area. |
| `ágio de R$ 80 mil` | Classify R$ 80,000 as an *ágio* amount, not a full price. |
| `entrada de R$ 80 mil` | Classify as a down payment, not a full price. |
| `parcelas de R$ 2.500` | Treat as a possible installment amount, not the property price. |
| `R$ 350 mil ou entrada de R$ 80 mil` | Preserve the full-price candidate and the down-payment candidate separately. |
| `3/4 de lote` | Do not interpret the fraction as three bedrooms without supporting context. |
| `300m da avenida` | Do not interpret the distance as property area. |

Add tests for punctuation variations, missing spaces, Unicode, multiple prices, conflicting descriptions, and false-positive patterns.

The parser must prefer uncertainty over an unsupported guess.

---

# 5. Modular Source Collection

Define a source-adapter interface so each supported platform can be implemented and tested independently.

Each adapter should:

- Discover listing references.
- Fetch listing content through permitted means.
- Extract source-specific fields.
- Return a standardized raw listing.
- Preserve the original listing URL.
- Report extraction failures and confidence.
- Respect per-source rate limits.
- Use timeouts, bounded retries, and caching where appropriate.
- Detect major layout changes or extraction failures.

Do not assume that every source supports the same discovery or extraction mechanism.

Start with two permitted sources. Candidate sources may include 62imoveis, OLX, ZAP Imóveis, or local brokerages, subject to verification of their current access rules and technical feasibility.

Do not treat Instagram or Facebook as ordinary HTML sources. Use only permitted APIs, approved providers, or other access methods that comply with the applicable platform rules. If a source cannot be accessed reliably and permissibly, document the limitation instead of bypassing its restrictions.

The UI must distinguish between:

- A successful search with no results.
- A source that returned results.
- A source that failed.
- A source whose results could not be parsed reliably.

A source failure must never be represented as proof that no suitable properties exist.

---

# 6. Conservative Property Deduplication and Entity Resolution

The goal is to identify advertisements for the same real-world property without accidentally merging different houses.

## 6.1 Matching Pipeline

Use a staged approach:

1. Normalize source-specific fields.
2. Generate potential match candidates using metadata and image hashes.
3. Compare independent signals.
4. Assign a confidence category.
5. Automatically consolidate only when the evidence is sufficiently strong.
6. Flag uncertain matches for review or keep them separate.

### Tier 1: Perceptual Hashing

Use pHash or another suitable perceptual hash for identical and near-identical image discovery.

A Hamming distance threshold such as `<= 5` may be used as an initial candidate-generation threshold, but it must not be treated as proof that two advertisements refer to the same property.

Different properties can have similar photographs. The same property can also have very different photographs.

### Tier 2: Metadata and Visual Similarity

Potential evidence includes:

- City and neighborhood.
- Lot area and built area.
- Bedroom and suite counts.
- Property type.
- Comparable full-price amounts.
- Address or location evidence, where lawfully available.
- Textual similarity.
- Image pHash results.
- Optional local vision embedding similarity.

A visual embedding cosine similarity of `>= 0.90` may be an experimental candidate threshold only. It must be calibrated against labeled examples before being used to justify automatic merging.

Do not merge based only on similar price, area, neighborhood, or appearance.

### Matching Outcomes

Use three outcomes:

- **High confidence:** Automatic merge is permitted only when multiple independent signals strongly support identity.
- **Possible match:** Keep separate and flag for review or further verification.
- **Low confidence:** Keep separate.

Record the evidence supporting every automated merge. Do not silently discard conflicting values.

Support undoing a merge and recovering the underlying property/listing relationships.

### Matching Validation

Create a small manually labeled evaluation set that includes:

- Different properties with similar facades.
- The same property advertised by different brokers.
- The same property shown from different angles.
- Watermarked, cropped, resized, or compressed photographs.
- Similar houses in the same condominium.
- Listings with inconsistent areas or prices.

Measure false-positive merges separately from missed duplicates. Prioritize avoiding false-positive merges, because an incorrect merge can corrupt price comparisons and rankings.

---

# 7. Two-Tier Ranking Engine

## 7.1 Tier 1: Deterministic Local Score

Calculate a local score from 0 to 100 in Rust.

The score must be based on the user's search preferences and evidence from collected listings. It must always operate without Gemini.

The initial proposed weights are:

- Price per comparable m²: 45%.
- Verified cross-listing price advantage: 25%.
- Bedroom/suite fit: 15%.
- Listing freshness: 15%.

Treat these as initial hypotheses, not validated facts. Keep weights configurable and document the normalization method.

### Important scoring rules

**Price per m²**

- Compare similar property types and market segments.
- Use built area and lot area separately.
- Do not compare values when the area definition is unknown or incompatible.
- Account for the number and quality of comparable observations.
- Do not imply that a lower price per m² automatically makes a property a good purchase.

**Cross-platform price advantage**

- Consider only sufficiently verified matches.
- Compare equivalent price classifications and transaction terms.
- Exclude *ágio*, down payments, installments, and unknown-price advertisements from ordinary full-price comparisons.
- Do not call a price difference a verified discount when the underlying listings are not comparable.

**Bedroom and suite fit**

- Score against the user's requirements or preferences.
- Do not assume that a higher bedroom count universally means a better property.

**Freshness**

- Distinguish the date an advertisement was first seen from the date it was last scraped.
- A recently scraped listing is not necessarily newly posted.

### Confidence and Explainability

Display a separate data-confidence indicator.

If important data is missing, contradictory, or based on too few comparables, explain that the score is less reliable. Do not manufacture precision.

Use a neutral label such as **“Pontuação local”** or **“Compatibilidade com seus critérios”**. Do not call it a definitive investment score or guarantee that a property is a bargain.

Store enough score components and evidence to explain why a result received its score.

## 7.2 Tier 2: Optional Gemini Comparison

Allow the user to request AI-assisted prioritization of the top 15–30 locally filtered candidates.

Send compact structured data, including:

- `property_id`
- Comparable asking price and price classification.
- Built area and lot area separately.
- Bedrooms, suites, and parking spaces.
- Neighborhood and property type.
- Number of source listings.
- Source price spread, when comparable.
- Verified facts and their evidence.
- Unknown or conflicting facts.
- Local score and confidence.
- Relevant excerpts from listing descriptions.

Do not send unnecessary personal or sensitive data.

Ask Gemini to identify qualitative signals, including:

- Explicit seller urgency.
- Price reductions supported by listing evidence.
- Described finishing and amenities.
- Potential concerns, such as occupancy, auction status, or informal contractual arrangements.

Distinguish explicit statements from inference. For example, the phrase `motivo mudança` may indicate a stated reason for selling, but does not prove the seller is under pressure or that the property is a bargain.

Do not claim the property has solar panels, planned cabinetry, or other features unless the supplied evidence supports that claim.

Require structured JSON containing:

- Valid `property_id`.
- AI priority rank.
- Concise Portuguese explanation.
- Evidence-backed positive signals.
- Potential concerns.
- Uncertainties.

Validate the complete response before displaying it. Reject unknown IDs, duplicates, missing candidates, malformed structures, or unsupported output as appropriate. Preserve the deterministic ranking if validation fails.

Cache results using a fingerprint of relevant input data, model, and prompt version. Invalidate the cache when meaningful property data changes.

AI results must be clearly marked as supplementary and must never silently replace the local score.

---

# 8. Geographic Configuration

Create a seed configuration such as `regions.json`, which is validated and loaded into SQLite.

Initial coverage:

### Goiânia

Include relevant residential neighborhoods, such as:

- Setor Bueno.
- Setor Marista.
- Jardim Goiás.
- Parque Amazônia.

### Aparecida de Goiânia

Include relevant residential neighborhoods, such as:

- Vila Brasília.
- Garavelo.
- Cidade Livre.
- Buriti Sereno.

Account for single-family homes, semi-detached homes, and listings described as *meio lote* or *lote inteiro*.

### Hidrolândia

Support:

- Gated communities.
- *Chácaras*.
- Weekend properties.
- Relevant areas along the BR-153 corridor.

Do not hardcode these as the only valid locations. They are initial configuration entries, not the full geographic model.

---

# 9. User Interface Requirements

Build a simple, approachable interface that hides technical complexity without hiding important uncertainty.

## 9.1 Search Controls

Provide:

- Region and neighborhood selection.
- Minimum and maximum price.
- A visible toggle: **“Ocultar Ágio (Apenas Valor Total)”**.
- A primary button: **“Buscar Ofertas”**.
- An optional button: **“Priorizar Melhores Ofertas (IA)”**.
- Clear progress and completion status.
- A visible indication when a source fails or returns unreliable data.

Use sensible defaults. Keep advanced settings out of the main workflow.

## 9.2 Property Cards

Each card should display, when available:

- Best representative photo.
- Comparable full asking price.
- Lot area and built area separately.
- Bedrooms, suites, and parking spaces.
- Neighborhood and property type.
- Local score and confidence.
- AI priority and explanation, when available.
- Relevant uncertainties or conflicting data.
- Direct links to individual advertisements and their respective prices.

Example link presentation:

`OLX — R$ 330.000`  
`62imoveis — R$ 345.000`  
`Instagram — R$ 350.000`

Use these values only as illustrative examples. Real results must come from collected listings.

Clearly distinguish verified full prices from *ágio*, down payments, installment amounts, and unclassified prices.

## 9.3 User Actions

Support:

- Favorite.
- Hide property.
- Restore a hidden property.
- Open original listing links.
- Review potential duplicate matches when necessary.

Hiding a property must persist across future scraping runs and rediscovery. It must not delete the underlying listing or history.

## 9.4 Settings

Provide an uncomplicated settings screen for:

- Optional Gemini API key configuration.
- Enabling or disabling local vision.
- Downloading or retrying an optional model.
- Viewing source health and recent collection errors.
- Managing cached data where appropriate.

Do not expose unnecessary technical settings to the normal user.

## 9.5 Accessibility and Performance

- Support keyboard navigation.
- Use clear labels, readable contrast, and accessible controls.
- Avoid requiring the user to understand scraping, embeddings, or database concepts.
- Keep collection and image processing off the main UI thread.
- Use pagination or incremental loading for large result sets.
- Limit concurrent network and image-processing operations.
- Make progress reporting accurate and cancelable where feasible.

---

# 10. Implementation Plan and Approval Gates

Implement the application in four phases. Each phase must be tested and reported before proceeding.

## Phase 1 — Foundation (START HERE)

Do only the work listed below.

### 1.1 Project Structure

Provide the complete directory tree for the Tauri v2 + React + TypeScript project.

Organize the Rust backend into clear modules, such as:

- `db`
- `models`
- `parser`
- `sources`
- `matching`
- `ranking`
- `ai`
- `commands`

Do not create empty modules for features that are not yet implemented unless a minimal module is required for compilation.

### 1.2 Cargo Dependencies

Provide `src-tauri/Cargo.toml` with the dependencies needed for Phase 1.

Evaluate appropriate versions and features for:

- Tauri v2.
- `rusqlite` with `bundled`.
- `serde`.
- `serde_json`.
- `tokio`.
- `reqwest`, if needed for the Phase 1 architecture.
- `scraper`, if needed for the Phase 1 architecture.
- `regex`.
- Suitable image and hashing crates, when needed.
- `chrono`.
- `url`, if needed.

Avoid adding every planned dependency immediately. Include dependencies needed for Phase 1 and document when deferred dependencies will be introduced.

Use compatible, explicit versions and preserve a lockfile. Do not invent crate features or assume that a particular API exists without checking the actual dependency version.

### 1.3 Domain Types

Implement real Rust types for:

- Normalized listing data.
- Property characteristics.
- Exact monetary values.
- Price classifications.
- Lot and built areas.
- Extraction evidence.
- Parsing confidence or ambiguity.
- Relevant error types.

These types must prevent accidental mixing of *ágio*, full price, down payments, and installment values.

### 1.4 SQLite Schema and Initialization

Create:

- `src-tauri/src/db/schema.sql`
- `src-tauri/src/db/mod.rs`

Implement schema initialization and a documented migration/versioning strategy.

Include the core tables described above, at minimum:

- `regions`
- `properties`
- `listing_links`
- `property_images`
- `property_matches`
- `listing_price_history`

Add appropriate foreign keys, indexes, uniqueness constraints, timestamps, and status fields.

Requirements:

- Initialize the database in Tauri's application data directory.
- Enable and verify SQLite foreign-key enforcement.
- Avoid hardcoded machine-specific paths.
- Use transactions where appropriate.
- Make initialization safe to repeat.
- Do not erase user data when the schema already exists.
- Support testing with a temporary database.
- Add database tests for initialization, persistence, constraints, relationships, and price-history behavior.

### 1.5 Brazilian Real-Estate Parser

Create:

`src-tauri/src/parser/br_real_estate.rs`

Implement the actual deterministic parser, including:

- Brazilian currency normalization.
- Full-price versus *ágio* classification.
- Down-payment and installment detection.
- Outstanding-balance extraction.
- Bedroom, suite, and parking-space parsing.
- Separate lot-area and built-area extraction.
- Ambiguity detection.
- Evidence retention.

Write comprehensive unit tests, including all examples in Section 4.4.

Do not force ambiguous data into a numeric field merely to produce a result.

### 1.6 Phase 1 Verification

Run the relevant available checks, including:

- Rust formatting.
- Compilation.
- Unit tests.
- Database tests.
- Clippy or equivalent static checks where practical.
- Frontend checks if the initial scaffold includes frontend code.

Report the actual results, including failures and limitations. Do not report checks as passing if they were not executed.

### Phase 1 Deliverables

At the end of Phase 1, provide:

1. Complete project directory tree.
2. Implemented files and their responsibilities.
3. Full database schema and initialization behavior.
4. Domain type definitions.
5. Parser implementation and test coverage.
6. Commands actually executed and their results.
7. Known limitations and unresolved decisions.
8. A concise explanation of what is ready for Phase 2.

**Stop after Phase 1. Do not implement the scraper engine, deduplication pipeline, ranking engine, Gemini integration, or finished React UI until I explicitly approve proceeding.**

## Phase 2 — Source Collection and Matching

After approval:

- Implement the source adapter interface.
- Integrate two permitted initial sources.
- Build normalization and source-health reporting.
- Implement pHash candidate generation.
- Add conservative property matching and reversible merge decisions.
- Preserve individual source data and price history.
- Test against real and recorded listing examples.
- Measure false-positive merges and missed duplicates.

Do not expand to many sources before the initial adapters prove reliable.

## Phase 3 — Ranking and Optional Gemini

After approval:

- Implement the deterministic local ranking engine.
- Implement price comparability and confidence calculations.
- Add structured Gemini requests and response validation.
- Add caching and quota/error handling.
- Verify that the local ranking works when AI is unavailable.
- Test against a manually reviewed sample of properties.

## Phase 4 — React UI and Windows Packaging

After approval:

- Implement the search interface and property cards.
- Add favorites, persistent hiding, and duplicate review.
- Add optional AI settings and local model management.
- Connect the UI through typed Tauri commands.
- Package a Windows build.
- Test data persistence, offline behavior, installation, upgrades, and recovery.

Do not claim the application is release-ready until the relevant packaging and end-to-end tests have actually passed.

---

# 11. Performance and Resource Requirements

The application should be practical on a low- to mid-range Windows PC without a dedicated GPU.

Use:

- Bounded concurrency.
- Background collection and processing.
- Incremental database writes.
- Cached HTTP responses where appropriate.
- Cached image hashes and embeddings.
- Lazy loading of optional models.
- Limited image dimensions and representative images per listing.
- Efficient SQL queries and indexes.
- Paginated result display.

Do not launch a large browser farm or generate embeddings for every image immediately.

Measure, rather than guess:

- Startup time.
- Idle and peak memory usage.
- Search latency.
- Image-hash processing time.
- Optional embedding-generation time.
- Database growth.
- AI request latency and failure behavior.

The absence of a GPU or optional model must not prevent ordinary searches.

---

# 12. Engineering Standards

- No placeholder pseudocode presented as completed functionality.
- No fabricated test results.
- No silent data overwrites.
- No unsafe automatic merges based on a single similarity threshold.
- No hardcoded secrets.
- No unnecessary dependencies.
- No unnecessary browser automation.
- No silent fallback that hides source failures.
- No claims of verified property features without evidence.
- No mixing of incompatible financial classifications.
- No destructive schema initialization.
- No unbounded retries or concurrency.
- No implementation beyond the current approved phase.

Keep code modular, readable, and testable. Document assumptions where the source data or external API behavior is uncertain.

If a requirement conflicts with the available APIs, package compatibility, source access rules, or current dependency behavior, explain the issue and propose the smallest practical adjustment instead of inventing a solution.

## Final Instruction

Begin with **Phase 1 only**.

Inspect the current project state before making changes if a repository already exists. Preserve existing work and conventions where appropriate. Implement the foundation, execute the tests, and present the evidence of completion.

Then stop and wait for my explicit confirmation before starting Phase 2.

---

*End of handoff.*
