import { useEffect, useRef, useState } from "react";
import { usageCall } from "../native/usageClient";
import type { ChartPoint } from "./marketsShared";

/** Requests at most this many symbols per round, one round per interval. */
const BATCH = 8;
const INTERVAL_MS = 1500;
/** Refetch a symbol's points after this long (the engine caches for 5 minutes too). */
const STALE_MS = 5 * 60_000;
/** Pause after a round in which the sources refused every symbol. */
const BACKOFF_MS = 30_000;

interface SparklinesResponse {
  items: Record<string, ChartPoint[]>;
  attempted: number;
  fetched: number;
}

/**
 * Intraday points for list rows whose quote has none (pools over 24 stocks fetch
 * batch quotes without them). Rows opt in by carrying `data-spark-symbol`; only the
 * rows on screen are loaded, a few per round, so a 500-stock pool never becomes a
 * burst of requests. Loading pauses while the window is hidden or the sources refuse.
 *
 * `rowsKey` must change whenever the set of opted-in rows changes, so the observer
 * picks up the new elements.
 */
export function useLazySparklines(
  container: React.RefObject<HTMLElement | null>,
  enabled: boolean,
  rowsKey: string,
): Record<string, ChartPoint[]> {
  const [points, setPoints] = useState<Record<string, ChartPoint[]>>({});
  const visible = useRef(new Set<string>());
  const requestedAt = useRef(new Map<string, number>());

  // Track which opted-in rows are on screen (with a margin, so scrolling is ahead).
  useEffect(() => {
    const root = container.current;
    if (!enabled || !root) return;
    visible.current.clear();
    const observer = new IntersectionObserver(
      (entries) => {
        for (const entry of entries) {
          const symbol = (entry.target as HTMLElement).dataset.sparkSymbol;
          if (!symbol) continue;
          if (entry.isIntersecting) visible.current.add(symbol);
          else visible.current.delete(symbol);
        }
      },
      { rootMargin: "240px 0px" },
    );
    root
      .querySelectorAll<HTMLElement>("[data-spark-symbol]")
      .forEach((el) => observer.observe(el));
    return () => observer.disconnect();
  }, [container, enabled, rowsKey]);

  // Load the visible ones a few at a time.
  useEffect(() => {
    if (!enabled) return;
    let inFlight = false;
    let pausedUntil = 0;
    const timer = window.setInterval(async () => {
      const now = Date.now();
      if (
        inFlight ||
        now < pausedUntil ||
        document.visibilityState !== "visible"
      ) {
        return;
      }
      const due = [...visible.current]
        .filter((s) => now - (requestedAt.current.get(s) ?? 0) > STALE_MS)
        .slice(0, BATCH);
      if (due.length === 0) return;
      inFlight = true;
      for (const s of due) requestedAt.current.set(s, now);
      try {
        const res = await usageCall<SparklinesResponse>("markets_sparklines", {
          symbols: due,
        });
        if (res.items && Object.keys(res.items).length > 0) {
          setPoints((prev) => ({ ...prev, ...res.items }));
        }
        if (res.attempted > 0 && res.fetched === 0)
          pausedUntil = now + BACKOFF_MS;
      } catch {
        pausedUntil = now + BACKOFF_MS;
      } finally {
        inFlight = false;
      }
    }, INTERVAL_MS);
    return () => window.clearInterval(timer);
  }, [enabled]);

  return points;
}
