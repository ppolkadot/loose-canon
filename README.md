# loose-canon
hope will not save you now

Local-first Windows desktop app that discovers, consolidates, filters and ranks real-estate
listings for the Goiânia metropolitan area (Goiânia, Aparecida de Goiânia, Hidrolândia).
Stack: Tauri v2, Rust, SQLite (`rusqlite`, bundled), React 19, TypeScript.

> **Status: Phase 1 (foundation) only.** Domain types, SQLite schema/migrations and the
> Brazilian real-estate text parser exist and are tested. There is **no** source collection,
> deduplication, ranking, Gemini integration or real UI yet; those are Phases 2-4 and wait for
> explicit approval. The frontend is a placeholder that proves the typed command bridge.

## Layout

```
index.html, vite.config.ts, tsconfig.json, package.json   frontend scaffold (Vite + React 19 + TS)
src/                                                      placeholder UI + typed `invoke` wrapper
src-tauri/
  Cargo.toml, build.rs, tauri.conf.json, capabilities/    Tauri v2 shell
  src/
    main.rs, lib.rs        entry point; opens the DB in Tauri's app-data dir during `setup`
    error.rs               ModelError / ParseError / Error (serialises as { kind, message })
    commands/              thin typed Tauri commands (Phase 1: get_app_status)
    models/                domain types: Money, Area kinds, price classifications, evidence,
                           Fact<T>, ExtractedFacts, NormalizedListing
    parser/                br_real_estate.rs (+ number.rs, text.rs helpers)
    db/                    mod.rs (connection, pragmas, migrations), schema.sql, price_history.rs
```

## Working with it

```powershell
npm install
npm run build              # tauri embeds ../dist at compile time, so build the frontend first
cd src-tauri
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
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
  avenida`) go to `discarded` with a reason.
* **Properties and listings are separate.** A property has no price column; prices live on
  listings and in the append-only `listing_price_history`. Merges are soft (`merged_into_property_id`)
  so they can be undone without moving any listing.
* **Migrations** use `PRAGMA user_version` + a `schema_migrations` audit table; see `db/mod.rs`.
