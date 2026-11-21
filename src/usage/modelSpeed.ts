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
};

const ratio = (tokens?: number, ms?: number) =>
  tokens && ms && ms > 0 ? (tokens * 1000) / ms : null;

/**
 * Output speed per model, fastest first. Exact speed (output tokens over
 * generation time, router-recorded requests) wins over the session-log
 * estimate, the same rule the stats table uses for its ≈ values. Models
 * without either are left out.
 */
export function modelSpeeds(models: ModelStats[], limit = 8): ModelSpeedRow[] {
  return models
    .flatMap((m) => {
      const exact = ratio(m.speedOutputTokens, m.speedGenerationMs);
      const estimate = ratio(m.estSpeedOutputTokens, m.estSpeedDurationMs);
      const speed = exact ?? estimate;
      if (speed === null || !Number.isFinite(speed)) return [];
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
        },
      ];
    })
    .sort((a, b) => b.speed - a.speed)
    .slice(0, limit);
}
