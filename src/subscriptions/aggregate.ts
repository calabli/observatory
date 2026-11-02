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
