import type { UsageSummary, UsageSummaryByApp } from "../types/usage";

export type RankingMetric = "tokens" | "cost" | "requests" | "hitRate";
export function rankingValue(
  summary: UsageSummary,
  metric: RankingMetric,
): number | null {
  if (metric === "tokens") return summary.realTotalTokens;
  if (metric === "requests") return summary.totalRequests;
  if (metric === "cost") {
    const cost = Number(summary.totalCost);
    return Number.isFinite(cost) ? cost : null;
  }
  const denominator =
    summary.totalInputTokens +
    summary.totalCacheReadTokens +
    summary.totalCacheCreationTokens;
  return denominator > 0 ? summary.totalCacheReadTokens / denominator : null;
}
export function rankApps(
  items: UsageSummaryByApp[],
  metric: RankingMetric,
  app: string,
) {
  return items
    .filter((a) => !app || a.appType === app)
    .map((a) => ({ ...a, value: rankingValue(a.summary, metric) }))
    .sort(
      (a, b) =>
        (b.value ?? -Infinity) - (a.value ?? -Infinity) ||
        a.appType.localeCompare(b.appType),
    )
    .slice(0, 7);
}
