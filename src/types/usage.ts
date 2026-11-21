// Type definitions for usage statistics

export interface RequestLog {
  requestId: string;
  providerId: string;
  providerName?: string;
  appType: string;
  model: string;
  requestModel?: string;
  /** Model name actually used for pricing at write time; may differ from `model` under route takeover + request pricing modes */
  pricingModel?: string;
  costMultiplier: string;
  inputTokens: number;
  outputTokens: number;
  cacheReadTokens: number;
  cacheCreationTokens: number;
  inputCostUsd: string;
  outputCostUsd: string;
  cacheReadCostUsd: string;
  cacheCreationCostUsd: string;
  totalCostUsd: string;
  isStreaming: boolean;
  latencyMs: number;
  /** The router service records the exact first token; session log imports record an estimate derived from timestamps (see dataSource) */
  firstTokenMs?: number;
  durationMs?: number;
  statusCode: number;
  errorMessage?: string;
  createdAt: number;
  dataSource?: string;
  /** The model has no public price (internal or included in a subscription), so its cost is zero by design and not a missing price */
  notBillable?: boolean;
}

export interface PaginatedLogs {
  data: RequestLog[];
  total: number;
  page: number;
  pageSize: number;
}

export interface ModelPricing {
  modelId: string;
  displayName: string;
  inputCostPerMillion: string;
  outputCostPerMillion: string;
  cacheReadCostPerMillion: string;
  cacheCreationCostPerMillion: string;
}

export interface ModelsDevSyncConfig {
  autoSyncEnabled: boolean;
  includeCommonModels: boolean;
  selectedModelKeys: string[];
  excludedCommonModelKeys: string[];
  lastSyncAt: number | null;
  lastSyncError: string | null;
}

export interface ModelsDevSyncState {
  config: ModelsDevSyncConfig;
  configPath: string;
}

export interface UsageSummary {
  totalRequests: number;
  totalCost: string;
  totalInputTokens: number;
  totalOutputTokens: number;
  totalCacheCreationTokens: number;
  totalCacheReadTokens: number;
  successRate: number;
  /** input + output + cache_creation + cache_read, all cache-normalized */
  realTotalTokens: number;
  /** cache_read / (input + cache_creation + cache_read), range 0–1 */
  cacheHitRate: number;
}

export interface UsageSummaryByApp {
  appType: string;
  summary: UsageSummary;
}

export interface DailyStats {
  date: string;
  requestCount: number;
  totalCost: string;
  totalTokens: number;
  totalInputTokens: number;
  totalOutputTokens: number;
  totalCacheCreationTokens: number;
  totalCacheReadTokens: number;
}

export interface ProviderStats {
  providerId: string;
  appType: string;
  providerName: string;
  requestCount: number;
  /** Real token consumption (fresh input + output + cache write + cache hit), same metric as the stat cards */
  totalTokens: number;
  totalCost: string;
  successRate: number;
  avgLatencyMs: number;
  /** Speed numerator: sum of output tokens over detail requests that have a first token and ≥ 100 output tokens (daily aggregates excluded) */
  speedOutputTokens?: number;
  /** Speed denominator: sum of (elapsed − first token) over the same requests, in milliseconds */
  speedGenerationMs?: number;
  /** Estimated speed numerator: sum of output tokens over session-log imports that have an estimated elapsed time and ≥ 200 output tokens */
  estSpeedOutputTokens?: number;
  /** Estimated speed denominator: sum of the estimated elapsed time (including first-token wait) over the same requests, in milliseconds */
  estSpeedDurationMs?: number;
  /** Median first-token time (ms): successful streaming detail requests routed through the router service; null when unavailable */
  firstTokenMedianMs?: number | null;
  /** Estimated median first-token time (ms): session-log imports whose first token was derived from timestamps; only shown when no exact value exists, marked with ≈ */
  estFirstTokenMedianMs?: number | null;
}

export interface ModelStats {
  model: string;
  requestCount: number;
  /** Real token consumption (fresh input + output + cache write + cache hit), same metric as the stat cards */
  totalTokens: number;
  totalCost: string;
  avgCostPerRequest: string;
  successRate: number;
  /** Speed numerator/denominator, same metric as the four speed fields of ProviderStats */
  speedOutputTokens?: number;
  speedGenerationMs?: number;
  estSpeedOutputTokens?: number;
  estSpeedDurationMs?: number;
  /** Median first-token time, same metric as the two corresponding fields of ProviderStats */
  firstTokenMedianMs?: number | null;
  estFirstTokenMedianMs?: number | null;
}

export interface LogFilters {
  appType?: string;
  providerName?: string;
  model?: string;
  statusCode?: number;
  startDate?: number;
  endDate?: number;
}

/**
 * App types whose proxy uses an OpenAI-style protocol. Two consequences:
 *
 * 1. `inputTokens` already includes the cached portion (must subtract
 *    `cacheReadTokens` to get fresh-input semantics — see
 *    [getFreshInputTokens]).
 * 2. The protocol does not report cache _creation_ separately, only cache
 *    _reads_. So `cacheCreationTokens` is always 0 for these app types and
 *    the UI should label it as N/A rather than 0.
 *
 * Mirror of the Rust `CACHE_INCLUSIVE_APP_TYPES` whitelist.
 */
export const CACHE_INCLUSIVE_APP_TYPES: ReadonlySet<string> = new Set([
  "codex",
  "gemini",
  "grokbuild",
]);

/** Subset of request-log fields needed to derive cache-normalized input. */
export interface CacheNormalizableLog {
  appType: string;
  inputTokens: number;
  cacheReadTokens: number;
}

/**
 * For a single request log, return the input token count with cache reads
 * removed. Anthropic-style providers already report `inputTokens` without
 * cache, so they pass through unchanged.
 */
export function getFreshInputTokens(log: CacheNormalizableLog): number {
  if (
    CACHE_INCLUSIVE_APP_TYPES.has(log.appType) &&
    log.inputTokens >= log.cacheReadTokens
  ) {
    return log.inputTokens - log.cacheReadTokens;
  }
  return log.inputTokens;
}

export const NON_NEGATIVE_DECIMAL_REGEX = /^\d+(?:\.\d+)?$/;

export function isNonNegativeDecimalString(value: string): boolean {
  const trimmed = value.trim();
  if (!NON_NEGATIVE_DECIMAL_REGEX.test(trimmed)) return false;
  return Number.isFinite(Number(trimmed));
}

type UsageCostLog = Pick<
  RequestLog,
  | "inputTokens"
  | "outputTokens"
  | "cacheReadTokens"
  | "cacheCreationTokens"
  | "totalCostUsd"
  | "statusCode"
> &
  Partial<Pick<RequestLog, "costMultiplier" | "notBillable">>;

export function hasUsageTokens(log: UsageCostLog): boolean {
  return (
    log.inputTokens > 0 ||
    log.outputTokens > 0 ||
    log.cacheReadTokens > 0 ||
    log.cacheCreationTokens > 0
  );
}

/**
 * The model has no public price, so the cost is zero by design. This is a
 * fact about the model, not a missing price that the user should fix.
 */
export function isNotBillableUsage(log: UsageCostLog): boolean {
  return log.notBillable === true;
}

/**
 * The request was billable and produced tokens, but no price was found for
 * it. Unlike a model without a public price, this one is worth reporting:
 * the price list is missing an entry.
 */
export function isUnpricedUsage(log: UsageCostLog): boolean {
  if (isNotBillableUsage(log)) return false;
  const totalCost = Number.parseFloat(log.totalCostUsd);
  const multiplier =
    log.costMultiplier == null
      ? undefined
      : Number.parseFloat(log.costMultiplier);
  return (
    log.statusCode >= 200 &&
    log.statusCode < 300 &&
    hasUsageTokens(log) &&
    Number.isFinite(totalCost) &&
    (!Number.isFinite(multiplier) || multiplier !== 0) &&
    totalCost === 0
  );
}
