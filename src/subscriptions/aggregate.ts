/** Query each interval independently so one unavailable historical slice does not hide others. */
export async function aggregateIndependently<W, T>(
  windows: W[],
  fetchWindow: (window: W) => Promise<T>,
): Promise<Array<{ window: W; value: T | null; error: unknown | null }>> {
  // Fire all requests in parallel and absorb errors per window: the error
  // isolation semantics are unchanged, but total latency drops from the sum of
  // all windows to the slowest one. The backend's single connection lock
  // serializes the actual queries anyway; what we save here is the chained
  // round-trip wait.
  return Promise.all(
    windows.map(async (window) => {
      try {
        return { window, value: await fetchWindow(window), error: null };
      } catch (error) {
        return { window, value: null, error };
      }
    }),
  );
}

/** Use one attribution rule across all windows; mixing rules can double-count a boundary day. */
export async function aggregateConsistently<W, T>(
  windows: W[],
  exact: (window: W) => Promise<T>,
  fallback: (window: W) => Promise<T>,
  needsFallback: (error: unknown) => boolean,
): Promise<Array<{ window: W; value: T | null; error: unknown | null }>> {
  const precise = await aggregateIndependently(windows, exact);
  return precise.some(
    (result) => result.error != null && needsFallback(result.error),
  )
    ? aggregateIndependently(windows, fallback)
    : precise;
}
