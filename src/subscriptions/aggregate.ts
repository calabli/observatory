/** Query each interval independently so one unavailable historical slice does not hide others. */
export async function aggregateIndependently<W, T>(
  windows: W[], fetchWindow: (window: W) => Promise<T>,
): Promise<Array<{ window: W; value: T | null; error: unknown | null }>> {
  const results: Array<{ window: W; value: T | null; error: unknown | null }> = [];
  for (const window of windows) {
    try { results.push({ window, value: await fetchWindow(window), error: null }); }
    catch (error) { results.push({ window, value: null, error }); }
  }
  return results;
}

/** Use one attribution rule across all windows; mixing rules can double-count a boundary day. */
export async function aggregateConsistently<W, T>(
  windows: W[], exact: (window: W) => Promise<T>,
  fallback: (window: W) => Promise<T>, needsFallback: (error: unknown) => boolean,
): Promise<Array<{ window: W; value: T | null; error: unknown | null }>> {
  const precise = await aggregateIndependently(windows, exact);
  return precise.some((result) => result.error != null && needsFallback(result.error))
    ? aggregateIndependently(windows, fallback)
    : precise;
}
