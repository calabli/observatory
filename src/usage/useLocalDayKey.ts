import { useCallback, useEffect, useState } from "react";

export function localDayKey(date = new Date()): string {
  return [
    date.getFullYear(),
    String(date.getMonth() + 1).padStart(2, "0"),
    String(date.getDate()).padStart(2, "0"),
  ].join("-");
}

export function localDayStart(dayKey: string): Date {
  const [y, m, d] = dayKey.split("-").map(Number);
  return new Date(y, (m || 1) - 1, d || 1, 0, 0, 0, 0);
}

export function msUntilNextLocalMidnight(now = new Date()): number {
  const next = new Date(
    now.getFullYear(),
    now.getMonth(),
    now.getDate() + 1,
    0,
    0,
    0,
    50,
  );
  return Math.max(1000, next.getTime() - now.getTime());
}

/**
 * Tracks the current local calendar day (`YYYY-MM-DD`).
 * Updates automatically at local midnight, on window focus / visibility restore
 * (e.g. waking from sleep the next morning), and on demand via `syncDayKey()`.
 */
export function useLocalDayKey(): [string, () => string] {
  const [dayKey, setDayKey] = useState(() => localDayKey());

  const syncDayKey = useCallback(() => {
    const current = localDayKey();
    setDayKey((prev) => (prev === current ? prev : current));
    return current;
  }, []);

  useEffect(() => {
    let timer: ReturnType<typeof setTimeout> | undefined;
    const schedule = () => {
      clearTimeout(timer);
      timer = setTimeout(() => {
        syncDayKey();
        schedule();
      }, msUntilNextLocalMidnight());
    };
    schedule();
    const onVisibility = () => {
      if (document.visibilityState === "visible") {
        syncDayKey();
        schedule();
      }
    };
    const onFocus = () => {
      syncDayKey();
      schedule();
    };
    document.addEventListener("visibilitychange", onVisibility);
    window.addEventListener("focus", onFocus);
    return () => {
      clearTimeout(timer);
      document.removeEventListener("visibilitychange", onVisibility);
      window.removeEventListener("focus", onFocus);
    };
  }, [syncDayKey]);

  return [dayKey, syncDayKey];
}
