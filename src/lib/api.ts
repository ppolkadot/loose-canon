import { invoke, isTauri } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

// Typed contract with the Rust backend (`src-tauri/src/commands/`). Rust owns all data and logic;
// this file only describes what crosses the bridge.
//
// Conventions:
// * returned DTOs are camelCase; enum values are the snake_case text stored in the database;
// * money is an integer number of centavos, areas an integer number of hundredths of m²;
// * timestamps are ISO-8601 strings;
// * *stored documents* (match evidence, a run's saved request, canonical provenance, merge
//   snapshots) keep the snake_case shape they are stored in.

// ---------------------------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------------------------

/** Mirrors the `{ kind, message }` shape produced by `error::Error`'s Serialize impl. */
export interface CommandError {
  kind: string;
  message: string;
}

/** Error kinds the interface should recognise (others are shown with their message). */
export const ERROR_KIND = {
  noSourcesConfigured: "no_sources_configured",
  collectionAlreadyRunning: "collection_already_running",
  matchRefused: "match_refused",
  invalidConfig: "invalid_config",
} as const;

export function runningInsideTauri(): boolean {
  return isTauri();
}

export function errorMessage(err: unknown): string {
  if (typeof err === "string") return err;
  if (err && typeof err === "object" && "message" in err) return String((err as CommandError).message);
  return String(err);
}

export function errorKind(err: unknown): string | null {
  return err && typeof err === "object" && "kind" in err ? String((err as CommandError).kind) : null;
}

// ---------------------------------------------------------------------------------------------
// Shared value types
// ---------------------------------------------------------------------------------------------

export type Confidence = "low" | "medium" | "high";
export type PriceClassification =
  | "full_price"
  | "agio"
  | "down_payment"
  | "monthly_payment"
  | "outstanding_balance"
  | "unknown";
export type UserStatus = "new" | "favorite" | "hidden";
export type ListingStatus = "active" | "unavailable" | "unknown";
export type ExtractionStatus = "pending" | "ok" | "partial" | "failed";
export type PropertyType =
  | "house"
  | "semi_detached"
  | "sobrado"
  | "apartment"
  | "lot"
  | "chacara"
  | "other";
export type RunStatus = "running" | "completed" | "cancelled" | "failed";
/** Four different states: a failed source is never "no properties exist". */
export type SourceOutcomeKind = "results" | "successful_empty" | "failed" | "unreliable";
export type RegionType =
  | "state"
  | "municipality"
  | "neighborhood"
  | "district"
  | "condominium"
  | "corridor"
  | "other";

export interface Page<T> {
  items: T[];
  total: number;
  offset: number;
  limit: number;
}

/** Price of one listing, with what it is and how sure the classifier is. */
export interface PriceView {
  amount: number;
  classification: PriceClassification;
  confidence: Confidence;
}

// ---------------------------------------------------------------------------------------------
// App status
// ---------------------------------------------------------------------------------------------

/** Mirrors `commands::AppStatus`. */
export interface AppStatus {
  appVersion: string;
  schemaVersion: number;
  foreignKeysEnabled: boolean;
}

export function getAppStatus(): Promise<AppStatus> {
  return invoke<AppStatus>("get_app_status");
}

// ---------------------------------------------------------------------------------------------
// Collection
// ---------------------------------------------------------------------------------------------

/** What a collection is asked for. Everything is optional; `{}` means "everything the source offers". */
export interface CollectionRequest {
  regionSlugs?: string[];
  minPriceCentavos?: number | null;
  maxPriceCentavos?: number | null;
  maxListings?: number | null;
}

export interface SourceOutcome {
  adapterId: string;
  kind: SourceOutcomeKind;
  /** `false` for a partial search: its warnings must be shown and it is not a complete search. */
  complete: boolean;
  discovered: number;
  stored: number;
  unparseable: number;
  failed: number;
  skipped: number;
  warnings: string[];
  errorKind: string | null;
  errorDetail: string | null;
  startedAt: string;
  finishedAt: string;
}

/** A registered source and its latest run. `latestOutcome: null` means it never ran (not "failed"). */
export interface SourceInfo {
  adapterId: string;
  platform: string;
  latestOutcome: SourceOutcome | null;
}

export interface RunRecord {
  id: number;
  startedAt: string;
  finishedAt: string | null;
  status: RunStatus;
  /** The saved search request (a stored document, snake_case). */
  request: unknown;
  note: string | null;
}

export interface StoredOutcome {
  id: number;
  runId: number;
  outcome: SourceOutcome;
}

export interface RunSummary {
  run: RunRecord;
  outcomes: StoredOutcome[];
}

export type ProgressPhase = "discovering" | "fetching" | "finished";

/** Event `collection-progress`. */
export interface ProgressEvent {
  runId: number;
  adapterId: string;
  phase: ProgressPhase;
  discovered: number;
  processed: number;
  stored: number;
}

export interface ImageReport {
  attempted: number;
  hashed: number;
  failed: number;
  skipped: number;
  retryLater: number;
  unavailableSource: number;
  notAttempted: number;
  internalErrors: number;
  cancelled: boolean;
}

export interface MatchRunReport {
  ruleVersion: string;
  listings: number;
  candidatePairs: number;
  skippedDecided: number;
  compared: number;
  high: number;
  possible: number;
  lowStored: number;
  lowDiscarded: number;
  created: number;
  updated: number;
  unchanged: number;
  decisionsKept: number;
  oversizedBlocks: number;
  oversizedImageBuckets: number;
  commonHashes: number;
  cancelled: boolean;
}

/** Event `collection-finished`. `error` is set when the collection itself could not run or finish. */
export interface CollectionFinished {
  runId: number | null;
  status: RunStatus | null;
  outcomes: SourceOutcome[];
  error: string | null;
  /** Problems in the work after a successful collection (images, matching, ranking). */
  followUpErrors: string[];
  images: ImageReport | null;
  matching: MatchRunReport | null;
  /** The local scores refreshed after matching. */
  ranking: RankingReport | null;
}

/** What a refresh of the local scores did. */
export interface RankingReport {
  /** Properties saved with a score. */
  scored: number;
  /** Properties saved without one (nothing could be computed), with their explanation. */
  unscored: number;
  /** Properties that had a score but are no longer ranked. */
  cleared: number;
}

export const COLLECTION_PROGRESS_EVENT = "collection-progress";
export const COLLECTION_FINISHED_EVENT = "collection-finished";

export function listSources(): Promise<SourceInfo[]> {
  return invoke<SourceInfo[]>("list_sources");
}

/**
 * Starts a collection in the background. Rejects with `no_sources_configured` when no source is
 * registered (nothing is recorded: that is not "searched and found nothing") and with
 * `collection_already_running` when one is running.
 */
export function startCollection(args: CollectionRequest = {}): Promise<void> {
  return invoke<void>("start_collection", { args });
}

/** Resolves to whether a collection was running (and is now being cancelled). */
export function cancelCollection(): Promise<boolean> {
  return invoke<boolean>("cancel_collection");
}

export function isCollectionRunning(): Promise<boolean> {
  return invoke<boolean>("is_collection_running");
}

/** Newest first. */
export function listCollectionRuns(limit?: number): Promise<RunRecord[]> {
  return invoke<RunRecord[]>("list_collection_runs", { limit: limit ?? null });
}

export function getCollectionRun(runId: number): Promise<RunSummary | null> {
  return invoke<RunSummary | null>("get_collection_run", { runId });
}

/** Failed, unreliable or incomplete source outcomes, newest first. */
export function listSourceProblems(limit?: number): Promise<StoredOutcome[]> {
  return invoke<StoredOutcome[]>("list_source_problems", { limit: limit ?? null });
}

export function onCollectionProgress(handler: (event: ProgressEvent) => void): Promise<UnlistenFn> {
  return listen<ProgressEvent>(COLLECTION_PROGRESS_EVENT, (e) => handler(e.payload));
}

export function onCollectionFinished(handler: (event: CollectionFinished) => void): Promise<UnlistenFn> {
  return listen<CollectionFinished>(COLLECTION_FINISHED_EVENT, (e) => handler(e.payload));
}

// ---------------------------------------------------------------------------------------------
// Regions and saved results
// ---------------------------------------------------------------------------------------------

export interface RegionRow {
  id: number;
  slug: string;
  stateCode: string;
  municipality: string | null;
  name: string;
  regionType: RegionType;
  parentId: number | null;
  enabled: boolean;
  aliases: string[];
}

export function listRegions(onlyEnabled = true): Promise<RegionRow[]> {
  return invoke<RegionRow[]>("list_regions", { onlyEnabled });
}

export type PropertySort = "recent" | "score";

/** Filters and paging of the saved-results list. `{}` is every visible property, first page. */
export interface ListPropertiesArgs {
  /** `recent` (default): latest activity first. `score`: highest local score first, unscored last. */
  sort?: PropertySort;
  includeHidden?: boolean;
  favoritesOnly?: boolean;
  /** A region slug; matches that region and everything below it. */
  regionSlug?: string | null;
  minPriceCentavos?: number | null;
  maxPriceCentavos?: number | null;
  /** "Ocultar Ágio (Apenas Valor Total)": only properties with an established full price. */
  fullPriceOnly?: boolean;
  /** `0` or absent means the default page size; larger values are capped. */
  limit?: number;
  offset?: number;
}

export interface ListingView {
  listingId: number;
  sourcePlatform: string;
  adapterId: string;
  url: string;
  title: string | null;
  price: PriceView | null;
  status: ListingStatus;
  extractionStatus: ExtractionStatus;
  firstSeenAt: string;
  lastSeenAt: string;
  lastVerifiedAt: string | null;
  lastScrapedAt: string | null;
}

/** An established, current full price of one listing of the property. */
export interface ComparablePrice {
  listingId: number;
  adapterId: string;
  amount: number;
  confidence: Confidence;
}

export type ScoreComponentKind = "price_per_m2" | "cross_listing" | "fit" | "freshness";

export type UnavailableReason =
  | "no_comparable_price"
  | "area_unknown"
  | "too_few_comparables"
  | "not_verified"
  | "single_source"
  | "no_preferences"
  | "value_unknown";

export type ConfidenceReason =
  | "no_price"
  | "no_area"
  | "low_component_coverage"
  | "partial_component_coverage"
  | "few_comparables"
  | "broad_segment"
  | "property_type_unknown"
  | "conflicting_values"
  | "price_confidence_medium"
  | "stale_data"
  | "fit_values_unknown";

/** One part of the local score. A component that cannot be computed is unavailable, never zero. */
export interface ScoreComponent {
  kind: ScoreComponentKind;
  /** The configured weight, before renormalising over the available components. */
  weight: number;
  /** 0-100, or null when unavailable. */
  score: number | null;
  unavailable_reason: UnavailableReason | null;
  /** What the score rests on (values, comparables, segment). A stored document. */
  evidence: unknown;
}

/** The stored explanation of a local score (a stored document, snake_case). */
export interface ScoreDetails {
  ranking_version: string;
  comparable_price_centavos: number | null;
  /** Always the four components, available or not. */
  components: ScoreComponent[];
  /** Share (0-1) of the total weight that was available. */
  available_weight_share: number;
  confidence_reasons: ConfidenceReason[];
}

export interface PropertyView {
  id: number;
  status: UserStatus;
  /** Members of a merge group disagree about favorite/hidden: show it to the user. */
  statusConflict: boolean;
  municipality: string | null;
  neighborhood: string | null;
  propertyType: PropertyType | null;
  bedrooms: number | null;
  suites: number | null;
  parkingSpaces: number | null;
  /** Hundredths of a square metre; lot, built and usable area are never interchangeable. */
  lotArea: number | null;
  builtArea: number | null;
  usableArea: number | null;
  /** `none`, `review_required` or `reviewed`. */
  matchReviewStatus: string;
  /** Every listing of the merge group, newest activity first. */
  listings: ListingView[];
  comparablePrices: ComparablePrice[];
  /** Lowest and highest comparable price. Differences between sources are information, not a conflict. */
  priceSpread: [number, number] | null;
  /** Which listing supports each canonical value (a stored document, snake_case). */
  canonicalProvenance: unknown;
  /** "Pontuação local", 0-100 with one decimal; null when the property is not ranked or nothing could be scored. */
  localScore: number | null;
  /** How much to trust the data behind the score (separate from the score itself). */
  dataConfidence: Confidence | null;
  scoreDetails: ScoreDetails | null;
  rankingVersion: string | null;
  scoredAt: string | null;
}

/** What the user is looking for. A criterion that is left empty is not scored. */
export interface SearchPreferences {
  bedroomsMin: number | null;
  bedroomsMax: number | null;
  suitesMin: number | null;
  parkingMin: number | null;
}

/** Weights and thresholds of the local score (the documented defaults until changed). */
export interface RankingConfig {
  weightPricePerM2: number;
  weightCrossListing: number;
  weightFit: number;
  weightFreshness: number;
  /** Fewest comparables a price-per-m² comparison may rest on. */
  minComparables: number;
  /** Comparables needed before the comparison counts as well supported. */
  solidComparables: number;
  /** A price spread of this percent between sources scores 100 (linear below it). */
  crossListingCapPercent: number;
  /** A listing first seen this many days ago (or more) scores 0 for freshness. */
  freshnessMaxAgeDays: number;
  /** A listing last scraped longer ago than this lowers the data confidence. */
  staleScrapeDays: number;
}

export function getSearchPreferences(): Promise<SearchPreferences> {
  return invoke<SearchPreferences>("get_search_preferences");
}

/** Refused when the preferences contradict themselves; refreshes the scores when saved. */
export function saveSearchPreferences(preferences: SearchPreferences): Promise<RankingReport> {
  return invoke<RankingReport>("save_search_preferences", { preferences });
}

export function getRankingConfig(): Promise<RankingConfig> {
  return invoke<RankingConfig>("get_ranking_config");
}

/** Refused when the values make the ranking meaningless; refreshes every score when saved. */
export function saveRankingConfig(config: RankingConfig): Promise<RankingReport> {
  return invoke<RankingReport>("save_ranking_config", { config });
}

/** Recomputes every score now (collections and merge decisions already do it on their own). */
export function recomputeRanking(): Promise<RankingReport> {
  return invoke<RankingReport>("recompute_ranking");
}

export function listProperties(args: ListPropertiesArgs = {}): Promise<Page<PropertyView>> {
  return invoke<Page<PropertyView>>("list_properties", { args });
}

/** Applies the choice to the property's whole merge group. Resolves to the rows touched. */
export function setPropertyStatus(propertyId: number, status: UserStatus): Promise<number> {
  return invoke<number>("set_property_status", { propertyId, status });
}

// ---------------------------------------------------------------------------------------------
// Match candidates (duplicate review). Every merge is a user decision.
// ---------------------------------------------------------------------------------------------

export type MatchCategory = "high" | "possible" | "low";
export type DecisionStatus = "candidate" | "accepted" | "rejected" | "review_required";
export type DecisionSource = "auto" | "user";

/** One signal for or against two listings being the same property (stored document, snake_case). */
export interface MatchSignal {
  kind: string;
  detail: string;
}

export interface MatchConflict {
  kind: string;
  severity: "hard" | "soft";
  detail: string;
}

/** Everything the matcher looked at for a pair (a stored document, snake_case). */
export interface MatchEvidence {
  rule_version: string;
  signals: MatchSignal[];
  conflicts: MatchConflict[];
  /** What could not be compared: absence of evidence is not evidence of a difference. */
  unknown: string[];
  image_pairs: { image_a: number; image_b: number; distance: number }[];
  notes: string[];
}

export interface ListingBrief {
  listingId: number;
  propertyId: number;
  /** Root of the merge group the listing is in right now. */
  groupRoot: number;
  adapterId: string;
  url: string;
  title: string | null;
  price: PriceView | null;
}

export interface MergeInfo {
  survivingPropertyId: number;
  absorbedPropertyId: number;
}

export interface MatchView {
  id: number;
  category: MatchCategory;
  decisionStatus: DecisionStatus;
  decisionSource: DecisionSource | null;
  decisionNote: string | null;
  a: ListingBrief;
  b: ListingBrief;
  evidence: MatchEvidence;
  minImageDistance: number | null;
  createdAt: string;
  reviewedAt: string | null;
  revertedAt: string | null;
  /** The merge this decision holds (or held, when `revertedAt` is set). */
  merge: MergeInfo | null;
}

export interface MergeEventRecord {
  id: number;
  matchId: number;
  event: "merged" | "reverted";
  survivingPropertyId: number;
  absorbedPropertyId: number;
  actor: string;
  ruleVersion: string | null;
  note: string | null;
  occurredAt: string;
  /** What each side's user status was at that moment (a stored document, snake_case). */
  snapshot: unknown;
}

/** `{}` lists every stored pair; the review queue is `reviewQueueArgs()`. */
export interface ListMatchesArgs {
  status?: DecisionStatus | null;
  /** Only pairs the user can still act on (the two listings are not yet one property). */
  actionableOnly?: boolean;
  limit?: number;
  offset?: number;
}

export function reviewQueueArgs(offset = 0, limit = 50): ListMatchesArgs {
  return { status: "review_required", actionableOnly: true, limit, offset };
}

export type AcceptOutcome =
  | { kind: "merged"; surviving: number; absorbed: number }
  | { kind: "already_merged" }
  | { kind: "already_same_group" };
export type RejectOutcome = { kind: "rejected" } | { kind: "already_rejected" };
export type UndoOutcome =
  | { kind: "reverted"; surviving: number; absorbed: number }
  | { kind: "already_reverted" };

export function listMatches(args: ListMatchesArgs = {}): Promise<Page<MatchView>> {
  return invoke<Page<MatchView>>("list_matches", { args });
}

export function getMatch(matchId: number): Promise<MatchView | null> {
  return invoke<MatchView | null>("get_match", { matchId });
}

/** The audit trail of one pair: every merge and undo, oldest first. */
export function listMatchEvents(matchId: number): Promise<MergeEventRecord[]> {
  return invoke<MergeEventRecord[]>("list_match_events", { matchId });
}

/** Merges the two properties. Repeating it changes nothing. A note is limited to 500 characters. */
export function acceptMatch(matchId: number, note?: string): Promise<AcceptOutcome> {
  return invoke<AcceptOutcome>("accept_match", { matchId, note: note ?? null });
}

/** Keeps the two apart. A merged pair must be undone first. */
export function rejectMatch(matchId: number, note?: string): Promise<RejectOutcome> {
  return invoke<RejectOutcome>("reject_match", { matchId, note: note ?? null });
}

/** Reverses a merge; each side keeps the status it had. */
export function undoMerge(matchId: number, note?: string): Promise<UndoOutcome> {
  return invoke<UndoOutcome>("undo_merge", { matchId, note: note ?? null });
}

/** Looks for duplicate candidates now. It only proposes; it never merges. */
export function runMatchingPass(): Promise<MatchRunReport> {
  return invoke<MatchRunReport>("run_matching_pass");
}
