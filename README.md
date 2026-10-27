# loose-canon
hope will not save you now

Local-first Windows desktop app that discovers, consolidates, filters and ranks real-estate
listings for the Goiânia metropolitan area (Goiânia, Aparecida de Goiânia, Hidrolândia).
Stack: Tauri v2, Rust, SQLite (`rusqlite`, bundled), React 19, TypeScript.

> **Status: Phase 2 (source collection and matching) is incomplete.** The shared collection
> pipeline, image processing, conservative duplicate matching with reversible merges, and the
> typed command layer are built and tested. **No real source adapter exists**: none of the
> evaluated sources (62imoveis, OLX, ZAP Imóveis, local brokerages) currently permits automated
> collection, so starting a collection fails with `no_sources_configured` instead of pretending to
> search. Phase 2 stays open until two permitted sources work; see
> [`docs/phase-2-alignment-review.md`](docs/phase-2-alignment-review.md) for the status, the
> evidence and the decisions needed from the owner. There is no ranking, Gemini integration or
> finished UI yet; those are Phases 3-4 and wait for explicit approval. The frontend is a
> diagnostics panel that exercises the typed commands.

## Documents

* [`docs/phase-2-design.md`](docs/phase-2-design.md): the approved Phase 2 plan.
* [`docs/phase-2-alignment-review.md`](docs/phase-2-alignment-review.md): what was built, what is
  missing and why, verification evidence, open decisions. **Start here.**
* [`docs/sources/`](docs/sources/README.md): dated evaluation of each candidate source.
* [`docs/matching.md`](docs/matching.md): the matching rule, merge/undo semantics, evaluation.
* [`HANDOFF.md`](HANDOFF.md): detailed end-of-Phase-1 handoff (domain model, schema, parser).

## Layout

```
index.html, vite.config.ts, tsconfig.json, package.json   frontend scaffold (Vite + React 19 + TS)
src/                                                      diagnostics UI; lib/api.ts = typed contract
src-tauri/
  Cargo.toml, build.rs, tauri.conf.json, capabilities/    Tauri v2 shell
  data/regions.json                                       seed for the supported regions
  src/
    main.rs, lib.rs        entry point; builds the AppService and runs startup in `setup`
    error.rs               ModelError / ParseError / Error (serialises as { kind, message })
    service/               application service: startup, one collection at a time, cancel,
                           collection -> images -> matching (free of Tauri, so it is testable)
    commands/              thin typed Tauri commands + the Rust side of the TS contract tests
    models/                domain types: Money, Area kinds, price classes, evidence, Fact<T>,
                           collection outcomes, matching evidence
    parser/                br_real_estate.rs (+ price-field hint; number.rs, text.rs helpers)
    sources/               adapter trait, bounded Fetcher, URL/normalisation, pipeline, registry
                           (the registry is empty on purpose: see docs/sources/)
    images/                bounded decode, deterministic pHash, near-pair index, cache, image job
    matching/              candidates, comparison rule, run, evaluation harness
    db/                    connection + migrations, schema.sql, migrations/0002_*.sql, and the
                           repositories (listings, properties, runs, regions, images, matches)
docs/                      plan, status report, source evaluations, matching reference
```

## Working with it

```powershell
npm install
npm run build              # tauri embeds ../dist at compile time, so build the frontend first
cd src-tauri
cargo fmt --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --locked
cd ..
npx tauri dev              # run the app
npx tauri build --debug --no-bundle   # build the exe without an installer
```

## Design decisions worth knowing

* **Money is integer centavos; areas are integer hundredths of m².** No floats for either.
* **Types prevent mixing amounts.** `FullPrice`, `Agio`, `DownPayment`, `MonthlyPayment` and
  `OutstandingBalance` are distinct newtypes; likewise `LotArea` / `BuiltArea` / `UsableArea`.
* **Unknown stays unknown.** `Fact<T>` is `Unknown | Known | Conflicting`: missing data is never
  zero and disagreements are never silently resolved.
* **The parser prefers uncertainty.** Every value carries confidence, the exact original text it
  came from, and warnings. Things that look like facts but are not (`3/4 de lote`, `300m da
  avenida`) go to `discarded` with a reason. Confidence below 60 % is never comparable.
* **A failed source is never "no results".** Each source outcome is one of results, successful
  empty, failed or unreliable, with a `complete` flag; a partial search says so.
* **Public is not authorised.** Collection follows each site's terms; a source is added to the
  registry only after its dated evaluation record permits it. Refusals (403) and rate limits (429)
  are never retried or bypassed.
* **Properties and listings are separate.** A property has no price column; prices live on
  listings and in the append-only `listing_price_history`. Merges are soft
  (`merged_into_property_id`), audited, atomic and reversible, and no listing ever moves.
* **Nothing merges on its own.** A perceptual hash only proposes a pair; every merge is a user
  decision. The matching rule is uncalibrated and its synthetic evaluation already shows a false
  merge, which is why.
* **Migrations** use `PRAGMA user_version` + a `schema_migrations` audit table; see `db/mod.rs`.
