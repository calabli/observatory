import type { ModelStats, ProviderStats } from "../types/usage";
export type StatsSortKey =
  | "requestCount"
  | "totalTokens"
  | "totalCost"
  | "avgCost"
  | "firstToken"
  | "successRate";
type Row = ModelStats | ProviderStats;
export function statsSortValue(row: Row, key: StatsSortKey): number | null {
  let value: number | null;
  if (key === "avgCost")
    value =
      "avgCostPerRequest" in row
        ? Number(row.avgCostPerRequest)
        : Number(row.totalCost) / Math.max(1, row.requestCount);
  else if (key === "firstToken")
    value = row.firstTokenMedianMs ?? row.estFirstTokenMedianMs ?? null;
  else value = Number(row[key]);
  return value === null || !Number.isFinite(value) ? null : value;
}
export function sortStats<T extends Row>(
  rows: T[],
  key: StatsSortKey,
  direction: "asc" | "desc",
): T[] {
  return rows
    .map((row, index) => ({ row, index, value: statsSortValue(row, key) }))
    .sort((a, b) => {
      if (a.value === null) return b.value === null ? a.index - b.index : 1;
      if (b.value === null) return -1;
      return (
        (a.value - b.value) * (direction === "asc" ? 1 : -1) ||
        a.index - b.index
      );
    })
    .map((item) => item.row);
}
