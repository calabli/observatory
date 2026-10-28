/** CC Switch's legacy totalTokens omits cache; the headline/trend includes all four counters. */
export function trendTokens(stat: {
  totalInputTokens: number;
  totalOutputTokens: number;
  totalCacheReadTokens: number;
  totalCacheCreationTokens: number;
}): number {
  return (
    stat.totalInputTokens +
    stat.totalOutputTokens +
    stat.totalCacheReadTokens +
    stat.totalCacheCreationTokens
  );
}

export function trendHitRate(stat: {
  totalInputTokens: number;
  totalCacheReadTokens: number;
  totalCacheCreationTokens: number;
}): number | null {
  const cacheable =
    stat.totalInputTokens +
    stat.totalCacheReadTokens +
    stat.totalCacheCreationTokens;
  return cacheable > 0 ? stat.totalCacheReadTokens / cacheable : null;
}

/** The Rust query emits RFC3339 timestamps for both hourly and daily buckets. */
export function isHourlyTrend(data: readonly { date: string }[]): boolean {
  if (!data.length) return false;
  if (/^\d{2}:\d{2}$/.test(data[0].date)) return true;
  if (data.length < 2) return false;
  const step = Date.parse(data[1].date) - Date.parse(data[0].date);
  return Number.isFinite(step) && step > 0 && step < 24 * 60 * 60 * 1000;
}

export function trendLabel(value: string, hourly: boolean): string {
  if (/^\d{2}:\d{2}$/.test(value)) return value;
  if (/^\d{4}-\d{2}-\d{2}/.test(value)) {
    return hourly && value.length >= 16
      ? value.slice(11, 16)
      : value.slice(5, 10);
  }
  return value;
}
