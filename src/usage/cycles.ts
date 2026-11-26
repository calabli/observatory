import type { UsageSummary } from "../types/usage";

export type CycleType = "weekly" | "rolling" | "monthly" | "custom_days";
export type QuotaMetric = "none" | "cost" | "tokens" | "requests";

export interface UsageCyclePlan {
  id: string;
  name: string;
  type: CycleType;
  /** 0 = Sunday .. 6 = Saturday (for weekly) */
  weekDay: number;
  /** "HH:mm" in local timezone (for weekly & monthly & custom_days) */
  time: string;
  /** Window length in hours (for rolling, e.g. 5, 24, 168) */
  rollingHours: number;
  /** Day of month 1..31 (for monthly) */
  monthDay: number;
  /** Cycle length in days (for custom_days) */
  customDays: number;
  /** YYYY-MM-DD local anchor start date (for custom_days) */
  anchorDate: string;
  /** Optional linked app filter; "" means all apps */
  appType: string;
  /** Optional reference quota dimension */
  quotaMetric: QuotaMetric;
  /** Reference quota ceiling: USD for cost, millions of tokens for tokens, count for requests */
  quotaLimit: number;
}

export interface CycleWindow {
  start: Date;
  end: Date;
  /** Inclusive Unix seconds start (matches usage_summary_impl `created_at >= startDate`) */
  startDate: number;
  /** Inclusive Unix seconds end (end - 1s, zero overlap between adjacent cycles) */
  endDate: number;
  totalMs: number;
  elapsedMs: number;
  remainingMs: number;
  /** 0..1 fraction of cycle time elapsed */
  timePct: number;
}

export type PaceStatus = "healthy" | "fast" | "over" | "completed" | "tracking";

export interface CyclePace {
  hasQuota: boolean;
  usedValue: number;
  limitValue: number;
  /** 0..Infinity percentage (e.g. 34.2) */
  usagePct: number;
  /** 0..100 percentage */
  timePct: number;
  /** usagePct - timePct */
  diffPct: number;
  status: PaceStatus;
  projectedValue: number | null;
  dailyAvgUsed: number;
  dailyAvgRemaining: number | null;
}

export const WEEKDAY_LABELS = [
  "周日",
  "周一",
  "周二",
  "周三",
  "周四",
  "周五",
  "周六",
] as const;

export const DEFAULT_CYCLE_PLANS: UsageCyclePlan[] = [
  {
    id: "builtin-weekly-mon",
    name: "自然周 · 周一重置",
    type: "weekly",
    weekDay: 1,
    time: "00:00",
    rollingHours: 5,
    monthDay: 1,
    customDays: 7,
    anchorDate: "2026-01-05",
    appType: "",
    quotaMetric: "none",
    quotaLimit: 0,
  },
  {
    id: "builtin-rolling-5h",
    name: "5 小时滑动窗",
    type: "rolling",
    weekDay: 1,
    time: "00:00",
    rollingHours: 5,
    monthDay: 1,
    customDays: 7,
    anchorDate: "2026-01-05",
    appType: "",
    quotaMetric: "none",
    quotaLimit: 0,
  },
  {
    id: "builtin-monthly-1",
    name: "自然月 · 1 日重置",
    type: "monthly",
    weekDay: 1,
    time: "00:00",
    rollingHours: 5,
    monthDay: 1,
    customDays: 30,
    anchorDate: "2026-01-01",
    appType: "",
    quotaMetric: "none",
    quotaLimit: 0,
  },
];

function pad2(n: number) {
  return String(n).padStart(2, "0");
}

function parseHM(time: string): [number, number] {
  const match = /^(\d{1,2}):(\d{2})(?::\d{2})?$/.exec((time || "").trim());
  if (!match) return [0, 0];
  const h = Math.min(23, Math.max(0, Number(match[1]) || 0));
  const m = Math.min(59, Math.max(0, Number(match[2]) || 0));
  return [h, m];
}

function clampInt(value: unknown, min: number, max: number, fallback: number) {
  const n = Math.round(Number(value));
  if (!Number.isFinite(n)) return fallback;
  return Math.min(max, Math.max(min, n));
}

function localDateKey(d = new Date()) {
  return `${d.getFullYear()}-${pad2(d.getMonth() + 1)}-${pad2(d.getDate())}`;
}

export function normalizeCyclePlan(
  raw: Partial<UsageCyclePlan>,
  index = 0,
): UsageCyclePlan {
  const type: CycleType =
    raw.type === "rolling" ||
    raw.type === "monthly" ||
    raw.type === "custom_days"
      ? raw.type
      : "weekly";
  const quotaMetric: QuotaMetric =
    raw.quotaMetric === "cost" ||
    raw.quotaMetric === "tokens" ||
    raw.quotaMetric === "requests"
      ? raw.quotaMetric
      : "none";
  const [h, m] = parseHM(raw.time ?? "00:00");
  const limit = Number(raw.quotaLimit);
  return {
    id:
      typeof raw.id === "string" && raw.id.trim()
        ? raw.id.trim()
        : `cycle-${index}-${Date.now()}`,
    name:
      typeof raw.name === "string" && raw.name.trim()
        ? raw.name.trim().slice(0, 60)
        : "未命名周期",
    type,
    weekDay: clampInt(raw.weekDay, 0, 6, 1),
    time: `${pad2(h)}:${pad2(m)}`,
    rollingHours: clampInt(raw.rollingHours, 1, 720, 5),
    monthDay: clampInt(raw.monthDay, 1, 31, 1),
    customDays: clampInt(raw.customDays, 1, 365, 14),
    anchorDate:
      typeof raw.anchorDate === "string" &&
      /^\d{4}-\d{2}-\d{2}$/.test(raw.anchorDate)
        ? raw.anchorDate
        : localDateKey(),
    appType: typeof raw.appType === "string" ? raw.appType.trim() : "",
    quotaMetric,
    quotaLimit: Number.isFinite(limit) && limit > 0 ? limit : 0,
  };
}

function makeMonthAnchor(
  year: number,
  monthZeroBased: number,
  day: number,
  hour: number,
  minute: number,
) {
  // Normalize year/month via JS Date rollover
  const probe = new Date(year, monthZeroBased, 1, 12, 0, 0, 0);
  const y = probe.getFullYear();
  const mo = probe.getMonth();
  const daysInMonth = new Date(y, mo + 1, 0).getDate();
  return new Date(y, mo, Math.min(day, daysInMonth), hour, minute, 0, 0);
}

function resolveLocalDateTime(
  year: number,
  monthZeroBased: number,
  day: number,
  hour: number,
  minute: number,
) {
  const d = new Date(year, monthZeroBased, day, 12, 0, 0, 0);
  d.setHours(hour, minute, 0, 0);
  return d;
}

/**
 * Compute the local cycle window `[start, end)` for a given plan and offset
 * (`0` = current cycle, `-1` = previous cycle, `-2` = two cycles ago, etc.).
 */
export function computeCycleWindow(
  plan: UsageCyclePlan,
  now = new Date(),
  offset = 0,
): CycleWindow {
  const safeOffset = Number.isInteger(offset) ? offset : 0;
  let start: Date;
  let end: Date;

  if (plan.type === "weekly") {
    const [h, m] = parseHM(plan.time);
    const day = clampInt(plan.weekDay, 0, 6, 1);
    const back = (now.getDay() - day + 7) % 7;
    const base = new Date(
      now.getFullYear(),
      now.getMonth(),
      now.getDate() - back,
      12,
      0,
      0,
      0,
    );
    base.setHours(h, m, 0, 0);
    if (base.getTime() > now.getTime()) {
      base.setDate(base.getDate() - 7);
      base.setHours(h, m, 0, 0);
    }
    start = new Date(
      base.getFullYear(),
      base.getMonth(),
      base.getDate() + safeOffset * 7,
      12,
      0,
      0,
      0,
    );
    start.setHours(h, m, 0, 0);
    end = new Date(
      start.getFullYear(),
      start.getMonth(),
      start.getDate() + 7,
      12,
      0,
      0,
      0,
    );
    end.setHours(h, m, 0, 0);
  } else if (plan.type === "rolling") {
    const hours = clampInt(plan.rollingHours, 1, 720, 5);
    const spanMs = hours * 3600 * 1000;
    // Align to whole second for deterministic caching keys
    const alignedNowMs = Math.floor(now.getTime() / 1000) * 1000;
    end = new Date(alignedNowMs + safeOffset * spanMs);
    start = new Date(end.getTime() - spanMs);
  } else if (plan.type === "monthly") {
    const [h, m] = parseHM(plan.time);
    const day = clampInt(plan.monthDay, 1, 31, 1);
    let mo = now.getMonth();
    const y = now.getFullYear();
    const curCandidate = makeMonthAnchor(y, mo, day, h, m);
    if (curCandidate.getTime() > now.getTime()) {
      mo -= 1;
    }
    const targetMo = mo + safeOffset;
    start = makeMonthAnchor(y, targetMo, day, h, m);
    end = makeMonthAnchor(y, targetMo + 1, day, h, m);
  } else {
    // custom_days
    const [h, m] = parseHM(plan.time);
    const days = clampInt(plan.customDays, 1, 365, 14);
    const match = /^(\d{4})-(\d{2})-(\d{2})$/.exec(plan.anchorDate || "");
    const anchor = match
      ? resolveLocalDateTime(
          Number(match[1]),
          Number(match[2]) - 1,
          Number(match[3]),
          h,
          m,
        )
      : resolveLocalDateTime(now.getFullYear(), now.getMonth(), 1, h, m);
    const spanMs = days * 86400 * 1000;
    const cycleIndex =
      Math.floor((now.getTime() - anchor.getTime()) / spanMs) + safeOffset;
    const startDayOffset = cycleIndex * days;
    start = new Date(
      anchor.getFullYear(),
      anchor.getMonth(),
      anchor.getDate() + startDayOffset,
      12,
      0,
      0,
      0,
    );
    start.setHours(h, m, 0, 0);
    end = new Date(
      start.getFullYear(),
      start.getMonth(),
      start.getDate() + days,
      12,
      0,
      0,
      0,
    );
    end.setHours(h, m, 0, 0);
  }

  const startSec = Math.floor(start.getTime() / 1000);
  const endExclusiveSec = Math.floor(end.getTime() / 1000);
  const endDate = Math.max(startSec, endExclusiveSec - 1);
  const totalMs = Math.max(1000, end.getTime() - start.getTime());
  const elapsedMs =
    safeOffset < 0
      ? totalMs
      : Math.max(0, Math.min(totalMs, now.getTime() - start.getTime()));
  const remainingMs = Math.max(0, end.getTime() - now.getTime());
  const timePct = Math.max(0, Math.min(1, elapsedMs / totalMs));

  return {
    start,
    end,
    startDate: startSec,
    endDate,
    totalMs,
    elapsedMs,
    remainingMs,
    timePct,
  };
}

export function extractQuotaUsage(
  summary: UsageSummary | undefined,
  metric: QuotaMetric,
): number {
  if (!summary) return 0;
  if (metric === "cost") return Number(summary.totalCost || 0);
  if (metric === "tokens") return (summary.realTotalTokens || 0) / 1_000_000;
  if (metric === "requests") return summary.totalRequests || 0;
  return 0;
}

export function computeCyclePace(
  plan: UsageCyclePlan,
  summary: UsageSummary | undefined,
  window: CycleWindow,
  offset = 0,
): CyclePace {
  const hasQuota = plan.quotaMetric !== "none" && plan.quotaLimit > 0;
  const usedValue = extractQuotaUsage(summary, plan.quotaMetric);
  const limitValue = hasQuota ? plan.quotaLimit : 0;
  const usagePct = hasQuota ? (usedValue / limitValue) * 100 : 0;
  const timePct = window.timePct * 100;
  const diffPct = usagePct - timePct;

  let status: PaceStatus;
  if (!hasQuota) {
    status = offset < 0 ? "completed" : "tracking";
  } else if (usagePct > 100) {
    status = "over";
  } else if (offset < 0) {
    status = "completed";
  } else if (diffPct > 10 && timePct >= 5) {
    status = "fast";
  } else {
    status = "healthy";
  }

  const projectedValue =
    hasQuota && offset === 0 && window.timePct >= 0.03
      ? usedValue / window.timePct
      : null;

  const totalDays = Math.max(1 / 24, window.totalMs / 86_400_000);
  const elapsedDays = Math.max(1 / 24, window.elapsedMs / 86_400_000);
  const remainDays = Math.max(
    1 / 24,
    (window.totalMs - window.elapsedMs) / 86_400_000,
  );
  const dailyAvgUsed = usedValue / (offset < 0 ? totalDays : elapsedDays);
  const dailyAvgRemaining =
    hasQuota && offset === 0
      ? Math.max(0, limitValue - usedValue) / remainDays
      : null;

  return {
    hasQuota,
    usedValue,
    limitValue,
    usagePct,
    timePct,
    diffPct,
    status,
    projectedValue,
    dailyAvgUsed,
    dailyAvgRemaining,
  };
}

export function formatCycleDateTime(d: Date): string {
  return `${d.getFullYear()}/${pad2(d.getMonth() + 1)}/${pad2(d.getDate())} ${pad2(d.getHours())}:${pad2(d.getMinutes())}`;
}

export function formatCycleShortDate(d: Date): string {
  return `${pad2(d.getMonth() + 1)}/${pad2(d.getDate())} ${pad2(d.getHours())}:${pad2(d.getMinutes())}`;
}

export function formatCycleRange(start: Date, end: Date): string {
  return `${formatCycleDateTime(start)} → ${formatCycleDateTime(end)}`;
}

export function formatCycleRule(plan: UsageCyclePlan): string {
  if (plan.type === "weekly") {
    return `每${WEEKDAY_LABELS[plan.weekDay] ?? "周一"} ${plan.time} 重置`;
  }
  if (plan.type === "rolling") {
    if (plan.rollingHours % 24 === 0 && plan.rollingHours >= 48) {
      return `滚动 ${plan.rollingHours / 24} 天窗口`;
    }
    return `滑动窗口 · ${plan.rollingHours} 小时`;
  }
  if (plan.type === "monthly") {
    return `每月 ${plan.monthDay} 日 ${plan.time} 重置`;
  }
  return `每 ${plan.customDays} 天循环 (${plan.time})`;
}

export function formatCountdown(ms: number): string {
  if (ms <= 0) return "即将重置";
  const totalMin = Math.floor(ms / 60_000);
  const days = Math.floor(totalMin / 1440);
  const hours = Math.floor((totalMin % 1440) / 60);
  const mins = totalMin % 60;
  if (days > 0) return `${days}天 ${hours}小时 ${mins}分`;
  if (hours > 0) return `${hours}小时 ${mins}分`;
  return `${Math.max(1, mins)}分钟`;
}

export function formatQuotaValue(value: number, metric: QuotaMetric): string {
  if (metric === "cost") return `$${value.toFixed(2)}`;
  if (metric === "tokens") {
    if (value >= 1000) return `${(value / 1000).toFixed(2)}B Token`;
    if (value >= 1) return `${value.toFixed(1)}M Token`;
    return `${Math.round(value * 1000)}K Token`;
  }
  if (metric === "requests") return `${Math.round(value).toLocaleString()} 次`;
  return "—";
}

export function periodDeltaPct(
  current: number,
  previous: number,
): number | null {
  if (
    !Number.isFinite(current) ||
    !Number.isFinite(previous) ||
    previous <= 0
  ) {
    return null;
  }
  return ((current - previous) / previous) * 100;
}
