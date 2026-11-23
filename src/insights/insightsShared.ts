import { INSIGHTS_RANGES, presetRange } from "../usage/timeRange";
import { localDayKey, localDayStart } from "../usage/useLocalDayKey";

export interface CacheSavings {
  actualCostUsd: number;
  cacheReadCostUsd: number;
  savedUsd: number;
  cacheHitRate: number;
  totalRequests: number;
  avgContextTokens: number;
}

export interface DailyCostPoint {
  date: string;
  cost: number;
}

export interface CostForecast {
  currentCostUsd: number;
  todayCostUsd: number;
  previousCostUsd: number;
  deltaPct: number | null;
  elapsedDays: number;
  dailyAvgUsd: number;
  projected30dUsd: number;
  dailyCosts: DailyCostPoint[];
}

export interface RhythmCell {
  weekday: number;
  hour: number;
  requests: number;
  costUsd: number;
  speedOutputTokens: number;
  speedDurationMs: number;
}

export interface SessionBubble {
  sessionId: string;
  appType: string;
  title: string | null;
  projectDir: string | null;
  turns: number;
  costUsd: number;
  avgContextTokens: number;
  totalTokens: number;
  outputTokens: number;
  lastAt: number;
}

export interface ProjectCostRow {
  projectDir: string;
  projectName: string;
  costUsd: number;
  requests: number;
  sessions: number;
  totalTokens: number;
  apps: string[];
}

export interface ModelStability {
  model: string;
  requests: number;
  ttftP50Ms: number | null;
  ttftP95Ms: number | null;
  ttftEstimated: boolean;
  speedP10: number | null;
  speedP50: number | null;
  speedEstimated: boolean;
}

export interface DailyTokenMix {
  date: string;
  freshInput: number;
  output: number;
  cacheCreation: number;
  cacheRead: number;
}

export interface StatusHealth {
  total: number;
  ok2xx: number;
  rateLimit429: number;
  server5xx: number;
  otherError: number;
  peak429Hour: number | null;
}

export interface SubscriptionRoi {
  id: string;
  name: string;
  appType: string;
  price: number;
  currency: string;
  billing: string;
  customIntervalDays?: number | null;
  rangeApiCostUsd: number;
  cycleApiCostUsd: number;
  cycleRequests: number;
  usdRoiMultiple: number | null;
}

export interface SessionTurnRow {
  requestId: string;
  createdAt: number;
  model: string;
  freshInput: number;
  outputTokens: number;
  cacheReadTokens: number;
  cacheCreationTokens: number;
  totalCostUsd: number;
  latencyMs: number;
  firstTokenMs: number | null;
  statusCode: number;
  dataSource: string;
  errorMessage: string | null;
}

export interface TraceSpan {
  id: string;
  kind: "llm" | "tool" | "subagent" | "gap" | string;
  depth: number;
  label: string;
  detail: string | null;
  startMs: number;
  endMs: number;
  offsetMs: number;
  durationMs: number;
  ttftMs: number | null;
  model: string | null;
  inputTokens: number;
  outputTokens: number;
  cacheReadTokens: number;
  costUsd: number;
  status: "ok" | "error" | string;
  errorMessage: string | null;
}

export interface ToolDurationStat {
  name: string;
  count: number;
  totalMs: number;
  errorCount: number;
}

export interface TaskTrace {
  index: number;
  prompt: string | null;
  startMs: number;
  endMs: number;
  durationMs: number;
  llmMs: number;
  toolMs: number;
  llmCalls: number;
  toolCalls: number;
  errorCount: number;
  totalTokens: number;
  totalCostUsd: number;
  topTools: ToolDurationStat[];
  spans: TraceSpan[];
}

export interface SessionTraceDetail {
  sessionId: string;
  appType: string;
  title: string | null;
  projectDir: string | null;
  gitBranch: string | null;
  resumeCommand: string | null;
  traceSource: "transcript" | "turns" | string;
  totalTurns: number;
  totalCostUsd: number;
  totalTokens: number;
  tasks: TaskTrace[];
  turns: SessionTurnRow[];
}

export interface InsightsData {
  cacheSavings: CacheSavings;
  forecast: CostForecast;
  rhythm: RhythmCell[];
  sessions: SessionBubble[];
  projects: ProjectCostRow[];
  stability: ModelStability[];
  tokenMix: DailyTokenMix[];
  status: StatusHealth;
  subscriptions: SubscriptionRoi[];
}

export type RangeKey = (typeof INSIGHTS_RANGES)[number];

export type HeatMode = "cost" | "requests" | "speed";

export type StabMode = "ttft" | "speed";

export type SessionSort = "cost" | "turns" | "context" | "recent";

export const BUDGET_KEY = "observatory.insights.monthly-budget";

export const DAILY_BUDGET_KEY = "observatory.insights.daily-budget";

export const WEEKDAYS = [
  "周一",
  "周二",
  "周三",
  "周四",
  "周五",
  "周六",
  "周日",
];

export const APP_COLORS: Record<string, string> = {
  claude: "var(--accent-orange)",
  codex: "var(--accent-blue)",
  opencode: "var(--accent-cyan)",
  agy: "var(--accent-violet)",
  dsh: "var(--accent-pink)",
  zcode: "var(--accent-green)",
  gemini: "var(--accent-blue)",
  grokbuild: "var(--accent-violet)",
  pi: "var(--accent-pink)",
  mcode: "var(--accent-orange)",
  kimi: "var(--accent-cyan)",
};

export const appColor = (id: string) =>
  APP_COLORS[id] ?? "var(--accent-violet)";

export function readNumberStorage(key: string, fallback: number): number {
  try {
    const raw = Number(localStorage.getItem(key));
    return Number.isFinite(raw) && raw > 0 ? raw : fallback;
  } catch {
    return fallback;
  }
}

export function projectBase(dir: string | null | undefined): string {
  if (!dir) return "";
  const trimmed = dir.replace(/[/\\]+$/, "");
  const parts = trimmed.split(/[/\\]/);
  return parts[parts.length - 1] || trimmed;
}

/** Rolling ranges stay open-ended so forecasts measure up to now. */
export const rangeBounds = (range: RangeKey, dayKey = localDayKey()) =>
  presetRange(range, localDayStart(dayKey), "open");

export function fmtUsd(v: number): string {
  if (v >= 100) return `$${v.toFixed(1)}`;
  return `$${v.toFixed(2)}`;
}

export function fmtTokens(n: number): string {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(2)}M`;
  if (n >= 1_000) return `${(n / 1_000).toFixed(1)}K`;
  return String(Math.round(n));
}

export function fmtDurationMs(ms: number): string {
  if (!Number.isFinite(ms) || ms <= 0) return "0ms";
  if (ms < 1000) return `${Math.round(ms)}ms`;
  if (ms < 60_000) return `${(ms / 1000).toFixed(ms < 10_000 ? 2 : 1)}s`;
  const mins = Math.floor(ms / 60_000);
  const secs = Math.round((ms % 60_000) / 1000);
  return `${mins}m ${secs}s`;
}

export function fmtClockMs(ms: number): string {
  const d = new Date(ms);
  const base = d.toLocaleTimeString("zh-CN", { hour12: false });
  const frac = String(Math.abs(ms) % 1000).padStart(3, "0");
  return `${base}.${frac}`;
}
