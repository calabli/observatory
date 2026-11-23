/** Local datetime inputs use an inclusive end, matching the native query. */
export function customTimeRange(
  start: string,
  end: string,
): { startDate: number; endDate: number } | null {
  if (!start || !end) return null;
  const first = new Date(start).getTime();
  const last = new Date(end).getTime();
  if (!Number.isFinite(first) || !Number.isFinite(last) || first > last)
    return null;
  return {
    startDate: Math.floor(first / 1000),
    endDate: Math.floor(last / 1000),
  };
}

/**
 * Every preset time range in the app, in one place. To add, rename or reorder
 * a range, edit `RANGE_PRESETS` and the per-page lists below; the pages read
 * their options, labels and bounds from here.
 *
 * `from` and `to` are the first and last day as offsets from today (0 = today,
 * -1 = yesterday), both inclusive; `from: "monthStart"` is the 1st of this month.
 */
export const RANGE_PRESETS = {
  today: { label: "今天", from: 0, to: 0 },
  yesterday: { label: "昨天", from: -1, to: -1 },
  "3d": { label: "近 3 天", from: -2, to: 0 },
  "7d": { label: "近 7 天", from: -6, to: 0 },
  "14d": { label: "近 14 天", from: -13, to: 0 },
  "30d": { label: "近 30 天", from: -29, to: 0 },
  month: { label: "本月", from: "monthStart", to: 0 },
} as const satisfies Record<
  string,
  { label: string; from: number | "monthStart"; to: number }
>;

export type RangePreset = keyof typeof RANGE_PRESETS;

/** Which presets each page offers, in display order. */
export const USAGE_RANGES = [
  "today",
  "yesterday",
  "3d",
  "7d",
  "30d",
] as const satisfies readonly RangePreset[];
export const INSIGHTS_RANGES = [
  "today",
  "yesterday",
  "3d",
  "7d",
  "14d",
  "30d",
  "month",
] as const satisfies readonly RangePreset[];
export const TRAY_RANGES = [
  "today",
  "7d",
] as const satisfies readonly RangePreset[];

/**
 * How a range that includes today ends:
 * - `dayEnd`: the last second of today (the usage page's closed day ranges);
 * - `now`: the current second;
 * - `open`: no end date, so the engine measures up to now (Insights forecasts).
 * Ranges that end before today always close at 23:59:59 of their last day.
 */
export type RangeEnd = "dayEnd" | "now" | "open";

/** Query bounds for a preset, counted from `todayStart` (local midnight). */
export function presetRange(
  preset: RangePreset,
  todayStart: Date,
  end: RangeEnd = "dayEnd",
  now: number = Date.now(),
): { startDate: number; endDate?: number } {
  const { from, to } = RANGE_PRESETS[preset];
  const start = new Date(todayStart);
  if (from === "monthStart") start.setDate(1);
  else start.setDate(start.getDate() + from);
  const startDate = Math.floor(start.getTime() / 1000);
  if (to === 0 && end === "open") return { startDate };
  if (to === 0 && end === "now") {
    return { startDate, endDate: Math.floor(now / 1000) };
  }
  const last = new Date(todayStart);
  last.setDate(last.getDate() + to + 1);
  return { startDate, endDate: Math.floor(last.getTime() / 1000) - 1 };
}

/** Keys saved by older versions of the usage page. */
const LEGACY_RANGE_KEYS: Record<string, RangePreset> = {
  days3: "3d",
  week: "7d",
  month: "30d",
};

/** A saved range key as a preset this page offers, or null. */
export function parseRangePreset(
  value: unknown,
  allowed: readonly RangePreset[],
  legacy = false,
): RangePreset | null {
  if (typeof value !== "string") return null;
  const key = (legacy ? LEGACY_RANGE_KEYS[value] : undefined) ?? value;
  return (allowed as readonly string[]).includes(key)
    ? (key as RangePreset)
    : null;
}
