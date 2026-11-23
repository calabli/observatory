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

export type DayPreset = "today" | "yesterday" | "days3" | "week" | "month";

/** First and last day of each preset, as offsets from today (inclusive). */
const PRESET_DAYS: Record<DayPreset, [number, number]> = {
  today: [0, 0],
  yesterday: [-1, -1],
  days3: [-2, 0],
  week: [-6, 0],
  month: [-29, 0],
};

/**
 * Whole local days for a preset, counted from `todayStart` (local midnight).
 * The end is the last second of the final day, matching the native query.
 */
export function presetDayRange(
  preset: DayPreset,
  todayStart: Date,
): { startDate: number; endDate: number } {
  const [first, last] = PRESET_DAYS[preset];
  const start = new Date(todayStart);
  start.setDate(start.getDate() + first);
  const end = new Date(todayStart);
  end.setDate(end.getDate() + last + 1);
  return {
    startDate: Math.floor(start.getTime() / 1000),
    endDate: Math.floor(end.getTime() / 1000) - 1,
  };
}
