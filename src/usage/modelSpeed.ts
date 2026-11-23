import type { ModelStats } from "../types/usage";

export type ModelSpeedRow = {
  model: string;
  /** Output tokens per second. */
  speed: number;
  /** True when only session-log estimates exist (includes first-token wait). */
  estimated: boolean;
  /** Median first-token time in milliseconds, exact first, else estimated. */
  firstTokenMs: number | null;
  firstTokenEstimated: boolean;
  requests: number;
  /** Output tokens behind the speed figure (exact or estimated). */
  basisTokens: number;
  /** Too little output measured for the speed to be reliable. */
  lowSample: boolean;
};

export type ModelSpeedSort = "usage" | "speed";

/**
 * Below this many measured output tokens the speed is shown but grayed: a few
 * requests can swing it widely (about 10–50 typical replies).
 */
export const LOW_SAMPLE_TOKENS = 5_000;

const ratio = (tokens?: number, ms?: number) =>
  tokens && ms && ms > 0 ? (tokens * 1000) / ms : null;

/**
 * Output speed per model. Exact speed (output tokens over generation time,
 * router-recorded requests) wins over the session-log estimate, the same rule
 * the stats table uses for its ≈ values. Models without either are left out.
 * `usage` sorts by request count (the models you rely on first), `speed` by
 * speed; reliable rows come before low-sample ones either way.
 */
export function modelSpeeds(
  models: ModelStats[],
  sort: ModelSpeedSort = "usage",
  limit = Infinity,
): ModelSpeedRow[] {
  return models
    .flatMap((m) => {
      const exact = ratio(m.speedOutputTokens, m.speedGenerationMs);
      const estimate = ratio(m.estSpeedOutputTokens, m.estSpeedDurationMs);
      const speed = exact ?? estimate;
      if (speed === null || !Number.isFinite(speed)) return [];
      const basisTokens =
        (exact !== null ? m.speedOutputTokens : m.estSpeedOutputTokens) ?? 0;
      const firstExact = m.firstTokenMedianMs ?? null;
      const firstTokenMs = firstExact ?? m.estFirstTokenMedianMs ?? null;
      return [
        {
          model: m.model,
          speed,
          estimated: exact === null,
          firstTokenMs,
          firstTokenEstimated: firstExact === null && firstTokenMs !== null,
          requests: m.requestCount,
          basisTokens,
          lowSample: basisTokens < LOW_SAMPLE_TOKENS,
        },
      ];
    })
    .sort(
      (a, b) =>
        Number(a.lowSample) - Number(b.lowSample) ||
        (sort === "usage" ? b.requests - a.requests : 0) ||
        b.speed - a.speed,
    )
    .slice(0, limit);
}
