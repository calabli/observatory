import type { ModelStats } from "../types/usage";

function localModelSpeed(m: ModelStats): {
  speed: number | null;
  estimated: boolean;
  firstTokenMs: number | null;
} {
  const exact =
    m.speedOutputTokens && m.speedGenerationMs && m.speedGenerationMs > 0
      ? (m.speedOutputTokens * 1000) / m.speedGenerationMs
      : null;
  const est =
    m.estSpeedOutputTokens && m.estSpeedDurationMs && m.estSpeedDurationMs > 0
      ? (m.estSpeedOutputTokens * 1000) / m.estSpeedDurationMs
      : null;
  const speed = exact ?? est;
  const firstTokenMs = m.firstTokenMedianMs ?? m.estFirstTokenMedianMs ?? null;
  return {
    speed: speed != null && Number.isFinite(speed) ? speed : null,
    estimated: exact == null && est != null,
    firstTokenMs,
  };
}

export interface BenchmarkModel {
  id: string;
  name: string;
  slug: string;
  releaseDate?: string | null;
  creatorName: string;
  creatorId?: string | null;
  intelligenceIndex?: number | null;
  codingIndex?: number | null;
  agenticIndex?: number | null;
  engineeringIndex?: number | null;
  financeIndex?: number | null;
  strategyIndex?: number | null;
  legalIndex?: number | null;
  healthcareIndex?: number | null;
  economicsIndex?: number | null;
  evalTotalCost?: number | null;
  evalCostPerTask?: number | null;
  priceInput1m?: number | null;
  priceOutput1m?: number | null;
  priceCacheHit1m?: number | null;
  priceCacheWrite1m?: number | null;
  medianTps?: number | null;
  meanTps?: number | null;
  medianTtftS?: number | null;
  meanTtftS?: number | null;
  medianE2eS?: number | null;
}

export interface BenchmarkSnapshot {
  provider: string;
  tier: string;
  intelligenceIndexVersion?: number | null;
  fetchedAt: number;
  totalPages: number;
  models: BenchmarkModel[];
}

export interface BenchmarksState {
  keyConfigured: boolean;
  keyHint?: string | null;
  tier: string;
  snapshot?: BenchmarkSnapshot | null;
}

export type BenchmarkDimensionKey =
  | "intelligenceIndex"
  | "engineeringIndex"
  | "codingIndex"
  | "agenticIndex"
  | "financeIndex"
  | "strategyIndex"
  | "legalIndex"
  | "healthcareIndex"
  | "economicsIndex";

export interface BenchmarkDimensionDef {
  key: BenchmarkDimensionKey;
  shortLabel: string;
  label: string;
}

export const BENCHMARK_DIMENSIONS: BenchmarkDimensionDef[] = [
  { key: "intelligenceIndex", shortLabel: "综合智力", label: "综合智力指数" },
  { key: "engineeringIndex", shortLabel: "工程", label: "工程能力指数" },
  { key: "codingIndex", shortLabel: "编程", label: "编程指数" },
  { key: "agenticIndex", shortLabel: "Agent", label: "智能体指数" },
  { key: "financeIndex", shortLabel: "金融财会", label: "金融与财会" },
  { key: "strategyIndex", shortLabel: "战略运营", label: "战略与运营" },
  { key: "legalIndex", shortLabel: "法律", label: "法律专业" },
  { key: "healthcareIndex", shortLabel: "医疗健康", label: "医疗与健康" },
  { key: "economicsIndex", shortLabel: "经济学", label: "经济学" },
];

export interface LocalModelMatch {
  requests: number;
  totalTokens: number;
  totalCostUsd: number;
  localSpeedTps: number | null;
  localSpeedEstimated: boolean;
  localTtftS: number | null;
  matchedLocalNames: string[];
}

export type ModelTierKey =
  | "max"
  | "xhigh"
  | "high"
  | "medium"
  | "low"
  | "minimal"
  | "adaptive"
  | "reasoning"
  | "non-reasoning"
  | "standard";

export interface ModelTierDef {
  key: ModelTierKey;
  label: string;
  order: number;
}

export const MODEL_TIER_DEFS: ModelTierDef[] = [
  { key: "max", label: "Max", order: 1 },
  { key: "xhigh", label: "Xhigh", order: 2 },
  { key: "high", label: "High", order: 3 },
  { key: "medium", label: "Medium", order: 4 },
  { key: "low", label: "Low", order: 5 },
  { key: "minimal", label: "Minimal", order: 6 },
  { key: "adaptive", label: "Adaptive", order: 7 },
  { key: "reasoning", label: "Reasoning", order: 8 },
  { key: "non-reasoning", label: "Non-reasoning", order: 9 },
  { key: "standard", label: "标准", order: 10 },
];

const TIER_ORDER_MAP = new Map(MODEL_TIER_DEFS.map((d) => [d.key, d.order]));
const TIER_LABEL_MAP = new Map(MODEL_TIER_DEFS.map((d) => [d.key, d.label]));

export interface ParsedModelVariant {
  familyKey: string;
  familyName: string;
  tierKey: ModelTierKey;
  /** Compact badge label such as "Max", "Low", "Non-reasoning · High" */
  tierLabel: string;
  /** Optional extra note such as "Default Fallback" or "Daybreak Blue" */
  extraNote: string | null;
  tierOrder: number;
}

function matchSingleTierToken(token: string): ModelTierKey | null {
  const t = token.trim().toLowerCase();
  if (t === "max") return "max";
  if (t === "xhigh" || t === "extra high" || t === "extra-high") return "xhigh";
  if (t === "high") return "high";
  if (t === "medium" || t === "med") return "medium";
  if (t === "low") return "low";
  if (t === "minimal" || t === "min") return "minimal";
  if (t === "adaptive") return "adaptive";
  if (t === "reasoning") return "reasoning";
  if (t === "non-reasoning" || t === "nonreasoning" || t === "non reasoning")
    return "non-reasoning";
  return null;
}

function isIgnorableAnnotation(token: string): boolean {
  const t = token.trim().toLowerCase();
  return (
    t === "default fallback" ||
    t.endsWith("fallback") ||
    t.startsWith("based on ")
  );
}

/**
 * Parse a benchmark model name into its base family name and reasoning/effort tier.
 * Examples:
 * - "Claude Fable 5.2 (Max, Default Fallback)" -> family "Claude Fable 5.2", tier "max" ("Max")
 * - "Claude Opus 4.6 (Non-reasoning, High)" -> family "Claude Opus 4.6", tier "non-reasoning" ("Non-reasoning · High")
 * - "GPT-6.2 Astra (Low)" -> family "GPT-6.2 Astra", tier "low" ("Low")
 * - "GPT-4o (Dec '24)" -> family "GPT-4o (Dec '24)", tier "standard" ("标准")
 */
export function parseModelVariant(
  model: Pick<BenchmarkModel, "name" | "creatorName">,
): ParsedModelVariant {
  const parenMatch = model.name.match(/^(.*?)\s*\(([^)]+)\)\s*$/);
  if (!parenMatch) {
    const familyName = model.name.trim();
    return {
      familyKey: `${model.creatorName.toLowerCase()}::${familyName.toLowerCase()}`,
      familyName,
      tierKey: "standard",
      tierLabel: "标准",
      extraNote: null,
      tierOrder: TIER_ORDER_MAP.get("standard") ?? 10,
    };
  }

  const basePart = parenMatch[1].trim();
  const rawTokens = parenMatch[2]
    .split(",")
    .map((s) => s.trim())
    .filter(Boolean);

  const matchedTiers: ModelTierKey[] = [];
  const otherTokens: string[] = [];
  const notes: string[] = [];

  for (const tok of rawTokens) {
    const tier = matchSingleTierToken(tok);
    if (tier) {
      matchedTiers.push(tier);
    } else if (isIgnorableAnnotation(tok)) {
      notes.push(tok);
    } else {
      otherTokens.push(tok);
    }
  }

  if (matchedTiers.length === 0) {
    // Parenthetical is a version/date tag like "(Dec '24)" or "(Preview)", not an effort tier
    const familyName = model.name.trim();
    return {
      familyKey: `${model.creatorName.toLowerCase()}::${familyName.toLowerCase()}`,
      familyName,
      tierKey: "standard",
      tierLabel: "标准",
      extraNote: null,
      tierOrder: TIER_ORDER_MAP.get("standard") ?? 10,
    };
  }

  // If "non-reasoning" is combined with an effort level (e.g. "Non-reasoning, High"),
  // keep "non-reasoning" as primary tierKey so it doesn't collide with reasoning "High".
  const primaryTier: ModelTierKey = matchedTiers.includes("non-reasoning")
    ? "non-reasoning"
    : matchedTiers[0];
  const labelParts = matchedTiers.map((k) => TIER_LABEL_MAP.get(k) ?? k);
  if (otherTokens.length > 0) {
    labelParts.push(...otherTokens);
  }
  const tierLabel = labelParts.join(" · ");
  const extraNote = notes.length > 0 ? notes.join(", ") : null;

  return {
    familyKey: `${model.creatorName.toLowerCase()}::${basePart.toLowerCase()}`,
    familyName: basePart,
    tierKey: primaryTier,
    tierLabel,
    extraNote,
    tierOrder:
      (TIER_ORDER_MAP.get(primaryTier) ?? 10) +
      (otherTokens.length > 0 ? 0.2 : 0) +
      (matchedTiers.length > 1 ? 0.1 : 0),
  };
}

export function shortModelLabel(
  model: Pick<BenchmarkModel, "name" | "creatorName">,
): string {
  const parsed = parseModelVariant(model);
  if (parsed.tierKey === "standard") {
    return parsed.familyName.length > 24
      ? parsed.familyName.slice(0, 23) + "…"
      : parsed.familyName;
  }
  const suffix = ` (${parsed.tierLabel})`;
  const maxBase = Math.max(10, 26 - suffix.length);
  const base =
    parsed.familyName.length > maxBase
      ? parsed.familyName.slice(0, maxBase - 1) + "…"
      : parsed.familyName;
  return `${base}${suffix}`;
}

export interface ModelFamilyVariant {
  model: BenchmarkModel;
  parsed: ParsedModelVariant;
  score: number | null;
}

export interface ModelFamilyGroup {
  familyKey: string;
  familyName: string;
  creatorName: string;
  releaseDate: string | null;
  /** All effort variants for this model, sorted Max -> Xhigh -> High -> Medium -> Low -> Non-reasoning */
  variants: ModelFamilyVariant[];
  /** Representative variant (highest score on active dim, or highest tier if none scored) */
  primary: ModelFamilyVariant;
  bestScore: number | null;
  bestIntelligence: number | null;
  bestEngineering: number | null;
}

export function groupModelFamilies(
  allModels: BenchmarkModel[],
  dimension: BenchmarkDimensionKey,
): ModelFamilyGroup[] {
  const buckets = new Map<string, ModelFamilyVariant[]>();
  for (const model of allModels) {
    const parsed = parseModelVariant(model);
    const list = buckets.get(parsed.familyKey) ?? [];
    list.push({
      model,
      parsed,
      score: getDimensionScore(model, dimension),
    });
    buckets.set(parsed.familyKey, list);
  }

  const groups: ModelFamilyGroup[] = [];
  for (const [familyKey, variants] of buckets.entries()) {
    variants.sort(
      (a, b) =>
        a.parsed.tierOrder - b.parsed.tierOrder ||
        (b.score ?? -1) - (a.score ?? -1) ||
        a.model.name.localeCompare(b.model.name),
    );
    // Representative: highest score on active dimension, else highest intelligence, else first tier
    const primary = [...variants].sort((a, b) => {
      if (a.score != null && b.score != null && a.score !== b.score) {
        return b.score - a.score;
      }
      if (a.score != null) return -1;
      if (b.score != null) return 1;
      const ai = a.model.intelligenceIndex ?? -1;
      const bi = b.model.intelligenceIndex ?? -1;
      if (ai !== bi) return bi - ai;
      return a.parsed.tierOrder - b.parsed.tierOrder;
    })[0];

    const bestScore = variants.reduce<number | null>(
      (acc, v) =>
        v.score != null && (acc == null || v.score > acc) ? v.score : acc,
      null,
    );
    const bestIntelligence = variants.reduce<number | null>(
      (acc, v) =>
        v.model.intelligenceIndex != null &&
        (acc == null || v.model.intelligenceIndex > acc)
          ? v.model.intelligenceIndex
          : acc,
      null,
    );
    const bestEngineering = variants.reduce<number | null>(
      (acc, v) =>
        v.model.engineeringIndex != null &&
        (acc == null || v.model.engineeringIndex > acc)
          ? v.model.engineeringIndex
          : acc,
      null,
    );
    const releaseDate =
      variants.find((v) => v.model.releaseDate)?.model.releaseDate ?? null;

    groups.push({
      familyKey,
      familyName: primary.parsed.familyName,
      creatorName: primary.model.creatorName,
      releaseDate,
      variants,
      primary,
      bestScore,
      bestIntelligence,
      bestEngineering,
    });
  }

  return groups;
}

export function getDimensionScore(
  model: BenchmarkModel,
  dim: BenchmarkDimensionKey,
): number | null {
  const value = model[dim];
  return typeof value === "number" && Number.isFinite(value) ? value : null;
}

/** Standard 3:1 input/output blended token price ($ per 1M tokens). */
export function blendedPrice1m(model: BenchmarkModel): number | null {
  const inp = model.priceInput1m;
  const out = model.priceOutput1m;
  const hasIn = typeof inp === "number" && Number.isFinite(inp) && inp >= 0;
  const hasOut = typeof out === "number" && Number.isFinite(out) && out >= 0;
  if (hasIn && hasOut) return (3 * inp + out) / 4;
  if (hasIn) return inp;
  if (hasOut) return out;
  return null;
}

export function effectiveTps(model: BenchmarkModel): number | null {
  const v = model.meanTps ?? model.medianTps;
  return typeof v === "number" && Number.isFinite(v) && v > 0 ? v : null;
}

export function effectiveTtftS(model: BenchmarkModel): number | null {
  const v = model.meanTtftS ?? model.medianTtftS;
  return typeof v === "number" && Number.isFinite(v) && v >= 0 ? v : null;
}

export interface ScatterPoint {
  id: string;
  x: number;
  y: number;
}

/**
 * Computes non-dominated Pareto frontier points:
 * - `price`: lower X (cheaper) and higher Y (smarter) wins.
 * - `speed`: higher X (faster) and higher Y (smarter) wins.
 */
export function computeParetoFrontier<T extends ScatterPoint>(
  points: T[],
  mode: "price" | "speed",
): T[] {
  const valid = points.filter(
    (p) => Number.isFinite(p.x) && Number.isFinite(p.y) && p.x > 0,
  );
  if (valid.length === 0) return [];
  if (mode === "price") {
    const sorted = [...valid].sort((a, b) => a.x - b.x || b.y - a.y);
    const frontier: T[] = [];
    let bestY = -Infinity;
    for (const pt of sorted) {
      if (pt.y > bestY) {
        frontier.push(pt);
        bestY = pt.y;
      }
    }
    return frontier;
  }
  const sorted = [...valid].sort((a, b) => b.x - a.x || b.y - a.y);
  const frontier: T[] = [];
  let bestY = -Infinity;
  for (const pt of sorted) {
    if (pt.y > bestY) {
      frontier.push(pt);
      bestY = pt.y;
    }
  }
  return frontier.reverse();
}

const EFFORT_WORDS = new Set([
  "low",
  "medium",
  "high",
  "xhigh",
  "max",
  "minimal",
  "adaptive",
  "reasoning",
  "non",
  "nonreasoning",
  "thinking",
  "default",
  "fallback",
  "preview",
  "latest",
  "instruct",
  "chat",
  "turbo",
]);

/** Normalize a model identifier or slug into canonical hyphenated lowercase form. */
export function normalizeModelSlug(raw: string): string {
  let s = raw.trim().toLowerCase();
  // Drop provider prefix like "anthropic/", "openai/", "google/"
  const slashIdx = s.lastIndexOf("/");
  if (slashIdx >= 0 && slashIdx < s.length - 1) {
    s = s.slice(slashIdx + 1);
  }
  // Drop context/routing suffixes like "[1m]", ":free", "@..."
  s = s
    .replace(/\[[^\]]*\]/g, "")
    .replace(/:[a-z0-9_-]+$/g, "")
    .replace(/@[a-z0-9._-]+$/g, "");
  // Drop parenthetical annotations like "(Max, Default Fallback)"
  s = s.replace(/\([^)]*\)/g, " ");
  // Strip trailing date stamps: -20250929, -2025-08-07, -0528
  s = s
    .replace(/[-_.]20\d{2}[-_.]?\d{2}[-_.]?\d{2}\b/g, "")
    .replace(/[-_.]20\d{2}[-_.]?\d{2}\b/g, "");
  // Convert punctuation/spaces to hyphens
  s = s
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "")
    .replace(/-{2,}/g, "-");
  return s;
}

/** Strip trailing effort/mode tokens (e.g. "-high", "-max", "-medium") to get base model slug. */
export function baseModelSlug(slug: string): string {
  const parts = slug.split("-").filter(Boolean);
  while (parts.length > 1 && EFFORT_WORDS.has(parts[parts.length - 1])) {
    parts.pop();
  }
  return parts.join("-");
}

/**
 * Match local usage `ModelStats` rows to the best corresponding `BenchmarkModel`
 * so users can compare benchmark scores/speeds against their real local measurements.
 */
export function matchLocalUsage(
  benchmarks: BenchmarkModel[],
  localModels: ModelStats[],
): Map<string, LocalModelMatch> {
  const result = new Map<string, LocalModelMatch>();
  if (benchmarks.length === 0 || localModels.length === 0) return result;

  // Build candidate index for benchmarks
  const exactSlug = new Map<string, BenchmarkModel[]>();
  const baseSlug = new Map<string, BenchmarkModel[]>();
  for (const bm of benchmarks) {
    const normSlug = normalizeModelSlug(bm.slug);
    const normName = normalizeModelSlug(bm.name);
    for (const key of new Set([normSlug, normName].filter(Boolean))) {
      const list = exactSlug.get(key) ?? [];
      list.push(bm);
      exactSlug.set(key, list);

      const base = baseModelSlug(key);
      if (base) {
        const bList = baseSlug.get(base) ?? [];
        bList.push(bm);
        baseSlug.set(base, bList);
      }
    }
  }

  const pickBest = (candidates: BenchmarkModel[]): BenchmarkModel => {
    return [...candidates].sort((a, b) => {
      // Prefer models with an intelligence index score, then higher score, then non-low variant
      const aHas = a.intelligenceIndex != null ? 1 : 0;
      const bHas = b.intelligenceIndex != null ? 1 : 0;
      if (bHas !== aHas) return bHas - aHas;
      const aScore = a.intelligenceIndex ?? -1;
      const bScore = b.intelligenceIndex ?? -1;
      if (bScore !== aScore) return bScore - aScore;
      return a.slug.length - b.slug.length;
    })[0];
  };

  for (const local of localModels) {
    if (!local.model || local.requestCount <= 0) continue;
    const norm = normalizeModelSlug(local.model);
    if (!norm) continue;
    const base = baseModelSlug(norm);

    let matched: BenchmarkModel | undefined;
    const exactHit = exactSlug.get(norm);
    if (exactHit && exactHit.length > 0) {
      matched = pickBest(exactHit);
    } else if (base && baseSlug.has(base)) {
      matched = pickBest(baseSlug.get(base)!);
    } else if (base && base.length >= 6) {
      // Prefix fallback for minor variant suffixes (e.g. "claude-sonnet-4-5" vs "claude-sonnet-4-5-thinking")
      const prefixHits = benchmarks.filter((bm) => {
        const bmBase = baseModelSlug(normalizeModelSlug(bm.slug));
        return (
          bmBase === base ||
          bmBase.startsWith(base + "-") ||
          base.startsWith(bmBase + "-")
        );
      });
      if (prefixHits.length > 0) {
        matched = pickBest(prefixHits);
      }
    }

    if (!matched) continue;

    const cost = Number.parseFloat(local.totalCost) || 0;
    const speedInfo = localModelSpeed(local);
    const prev = result.get(matched.id);
    if (!prev) {
      result.set(matched.id, {
        requests: local.requestCount,
        totalTokens: local.totalTokens,
        totalCostUsd: cost,
        localSpeedTps: speedInfo.speed,
        localSpeedEstimated: speedInfo.estimated,
        localTtftS:
          speedInfo.firstTokenMs != null ? speedInfo.firstTokenMs / 1000 : null,
        matchedLocalNames: [local.model],
      });
    } else {
      const totalReq = prev.requests + local.requestCount;
      // Weighted average speed if both present
      let nextSpeed = prev.localSpeedTps;
      let nextEst = prev.localSpeedEstimated;
      if (speedInfo.speed != null) {
        if (nextSpeed == null) {
          nextSpeed = speedInfo.speed;
          nextEst = speedInfo.estimated;
        } else {
          nextSpeed =
            (nextSpeed * prev.requests + speedInfo.speed * local.requestCount) /
            totalReq;
          nextEst = nextEst && speedInfo.estimated;
        }
      }
      const nextTtft =
        prev.localTtftS ??
        (speedInfo.firstTokenMs != null ? speedInfo.firstTokenMs / 1000 : null);
      result.set(matched.id, {
        requests: totalReq,
        totalTokens: prev.totalTokens + local.totalTokens,
        totalCostUsd: prev.totalCostUsd + cost,
        localSpeedTps: nextSpeed,
        localSpeedEstimated: nextEst,
        localTtftS: nextTtft,
        matchedLocalNames: [...prev.matchedLocalNames, local.model],
      });
    }
  }

  return result;
}

const CREATOR_PALETTE: Array<[RegExp, string]> = [
  [/anthropic/i, "#d97757"],
  [/openai/i, "#10a37f"],
  [/google/i, "#3b82f6"],
  [/deepseek/i, "#6366f1"],
  [/xai|x\.ai/i, "#8b5cf6"],
  [/meta/i, "#0ea5e9"],
  [/alibaba|qwen/i, "#f59e0b"],
  [/z\s*ai|zhipu|glm/i, "#06b6d4"],
  [/moonshot|kimi/i, "#ec4899"],
  [/minimax/i, "#f43f5e"],
  [/mistral/i, "#ea580c"],
  [/amazon|nova/i, "#84cc16"],
  [/cohere/i, "#14b8a6"],
  [/nvidia/i, "#22c55e"],
];

export function creatorColor(name: string): string {
  for (const [re, color] of CREATOR_PALETTE) {
    if (re.test(name)) return color;
  }
  return "#94a3b8";
}

export function formatPrice1m(v: number | null | undefined): string {
  if (v == null || !Number.isFinite(v)) return "—";
  if (v === 0) return "$0";
  if (v < 0.1) return `$${v.toFixed(3)}`;
  if (v < 10) return `$${v.toFixed(2)}`;
  return `$${v.toFixed(1)}`;
}

export function formatScore(v: number | null | undefined): string {
  if (v == null || !Number.isFinite(v)) return "—";
  return v.toFixed(1);
}

export function formatTps(v: number | null | undefined): string {
  if (v == null || !Number.isFinite(v) || v <= 0) return "—";
  return `${Math.round(v)} tok/s`;
}

export function formatSeconds(v: number | null | undefined): string {
  if (v == null || !Number.isFinite(v)) return "—";
  if (v < 10) return `${v.toFixed(2)}s`;
  return `${v.toFixed(1)}s`;
}
