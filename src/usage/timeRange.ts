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
