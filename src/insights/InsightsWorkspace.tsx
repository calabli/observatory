import { useEffect, useMemo, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import {
  Activity,
  AlertTriangle,
  Check,
  Code2,
  Copy,
  Download,
  Eye,
  FileText,
  FolderGit2,
  GitBranch,
  Layers3,
  ListTree,
  Search,
  Table2,
  Wrench,
  X,
} from "lucide-react";
import { usageCall } from "../native/usageClient";
import { AppGlyph } from "../usage/AppGlyph";
import { formatRequestSpeed } from "../usage/requestSpeed";
import { formatSubscriptionPrice } from "../subscriptions/format";
import type { LanStatus, SourceStatus } from "../usage/types";
import { buildHtmlReport, buildMarkdownReport } from "./reportBuilder";
import "./insights.css";

interface CacheSavings {
  actualCostUsd: number;
  cacheReadCostUsd: number;
  savedUsd: number;
  cacheHitRate: number;
  totalRequests: number;
  avgContextTokens: number;
}

interface DailyCostPoint {
  date: string;
  cost: number;
}

interface CostForecast {
  currentCostUsd: number;
  todayCostUsd: number;
  previousCostUsd: number;
  deltaPct: number | null;
  elapsedDays: number;
  dailyAvgUsd: number;
  projected30dUsd: number;
  dailyCosts: DailyCostPoint[];
}

interface RhythmCell {
  weekday: number;
  hour: number;
  requests: number;
  costUsd: number;
  speedOutputTokens: number;
  speedDurationMs: number;
}

interface SessionBubble {
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

interface ProjectCostRow {
  projectDir: string;
  projectName: string;
  costUsd: number;
  requests: number;
  sessions: number;
  totalTokens: number;
  apps: string[];
}

interface ModelStability {
  model: string;
  requests: number;
  ttftP50Ms: number | null;
  ttftP95Ms: number | null;
  ttftEstimated: boolean;
  speedP10: number | null;
  speedP50: number | null;
  speedEstimated: boolean;
}

interface DailyTokenMix {
  date: string;
  freshInput: number;
  output: number;
  cacheCreation: number;
  cacheRead: number;
}

interface StatusHealth {
  total: number;
  ok2xx: number;
  rateLimit429: number;
  server5xx: number;
  otherError: number;
  peak429Hour: number | null;
}

interface SubscriptionRoi {
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

interface SessionTurnRow {
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

interface TraceSpan {
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

interface ToolDurationStat {
  name: string;
  count: number;
  totalMs: number;
  errorCount: number;
}

interface TaskTrace {
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

interface SessionTraceDetail {
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

interface InsightsData {
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

type RangeKey = "7d" | "14d" | "30d" | "month";
type HeatMode = "cost" | "requests" | "speed";
type StabMode = "ttft" | "speed";
type SessionSort = "cost" | "turns" | "context" | "recent";

const BUDGET_KEY = "observatory.insights.monthly-budget";
const DAILY_BUDGET_KEY = "observatory.insights.daily-budget";
const WEEKDAYS = ["周一", "周二", "周三", "周四", "周五", "周六", "周日"];
const RANGE_LABELS: Record<RangeKey, string> = {
  "7d": "近 7 天",
  "14d": "近 14 天",
  "30d": "近 30 天",
  month: "本月",
};

const APP_COLORS: Record<string, string> = {
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

const appColor = (id: string) => APP_COLORS[id] ?? "var(--accent-violet)";

function readNumberStorage(key: string, fallback: number): number {
  try {
    const raw = Number(localStorage.getItem(key));
    return Number.isFinite(raw) && raw > 0 ? raw : fallback;
  } catch {
    return fallback;
  }
}

function projectBase(dir: string | null | undefined): string {
  if (!dir) return "";
  const trimmed = dir.replace(/[/\\]+$/, "");
  const parts = trimmed.split(/[/\\]/);
  return parts[parts.length - 1] || trimmed;
}

function rangeBounds(range: RangeKey): { startDate: number; endDate: number } {
  const now = new Date();
  const endDate = Math.floor(now.getTime() / 1000) + 1;
  if (range === "month") {
    const start = new Date(now.getFullYear(), now.getMonth(), 1);
    return { startDate: Math.floor(start.getTime() / 1000), endDate };
  }
  const days = range === "7d" ? 7 : range === "14d" ? 14 : 30;
  const start = new Date(now);
  start.setHours(0, 0, 0, 0);
  start.setDate(start.getDate() - (days - 1));
  return { startDate: Math.floor(start.getTime() / 1000), endDate };
}

function fmtUsd(v: number): string {
  if (v >= 100) return `$${v.toFixed(1)}`;
  return `$${v.toFixed(2)}`;
}

function fmtTokens(n: number): string {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(2)}M`;
  if (n >= 1_000) return `${(n / 1_000).toFixed(1)}K`;
  return String(Math.round(n));
}

export function InsightsWorkspace({ sources }: { sources: SourceStatus[] }) {
  const qc = useQueryClient();
  const [range, setRange] = useState<RangeKey>("30d");
  const [app, setApp] = useState<string>("");
  const [multiDevice, setMultiDevice] = useState<boolean>(false);
  const [heatMode, setHeatMode] = useState<HeatMode>("cost");
  const [stabMode, setStabMode] = useState<StabMode>("ttft");
  const [stabTouched, setStabTouched] = useState(false);
  const [budget, setBudget] = useState<number>(() =>
    readNumberStorage(BUDGET_KEY, 180),
  );
  const [dailyBudget, setDailyBudget] = useState<number>(() =>
    readNumberStorage(DAILY_BUDGET_KEY, 10),
  );
  const [sessionQuery, setSessionQuery] = useState("");
  const [sessionSort, setSessionSort] = useState<SessionSort>("cost");
  const [projectFilter, setProjectFilter] = useState<string | null>(null);
  const [reportOpen, setReportOpen] = useState(false);
  const [selectedSession, setSelectedSession] = useState<{
    sessionId: string;
    appType: string;
  } | null>(null);
  const [tip, setTip] = useState<{
    x: number;
    y: number;
    title: string;
    lines: string[];
  } | null>(null);

  const lan = useQuery({
    queryKey: ["usage", "lan"],
    queryFn: () => usageCall<LanStatus>("lan_status"),
    staleTime: 10000,
    refetchInterval: () =>
      document.visibilityState === "visible" ? 15000 : false,
    retry: false,
  });
  const enabledPeers = useMemo(
    () => (lan.data?.devices ?? []).filter((d) => d.enabled && d.paired),
    [lan.data?.devices],
  );
  const hasPeers = enabledPeers.length > 0;
  const effectiveMultiDevice = multiDevice && hasPeers;

  const scanRevision = useQuery({
    queryKey: ["usage", "scan-revision"],
    queryFn: () =>
      usageCall<{ revision: number; epoch: number }>("scan_revision"),
    refetchInterval: () =>
      document.visibilityState === "visible" ? 2000 : false,
    retry: false,
  });
  useEffect(() => {
    if (scanRevision.data?.epoch === undefined) return;
    void qc.invalidateQueries({ queryKey: ["usage", "insights"] });
  }, [scanRevision.data?.epoch, qc]);

  const visibleSources = useMemo(
    () => sources.filter((s) => s.id !== "claude-desktop"),
    [sources],
  );
  const sourceName = useMemo(
    () => new Map(visibleSources.map((s) => [s.id, s.name])),
    [visibleSources],
  );

  const { startDate, endDate } = useMemo(() => rangeBounds(range), [range]);

  const query = useQuery({
    queryKey: ["usage", "insights", range, app, effectiveMultiDevice],
    queryFn: () =>
      usageCall<InsightsData>("insights", {
        startDate,
        endDate,
        appType: app || undefined,
        multiDevice: effectiveMultiDevice,
      }),
    placeholderData: (prev) => prev,
  });

  const data = query.data;

  const updateBudget = (next: number) => {
    if (!Number.isFinite(next) || next <= 0) return;
    setBudget(next);
    try {
      localStorage.setItem(BUDGET_KEY, String(next));
    } catch {
      // ignore storage errors
    }
  };

  const updateDailyBudget = (next: number) => {
    if (!Number.isFinite(next) || next <= 0) return;
    setDailyBudget(next);
    try {
      localStorage.setItem(DAILY_BUDGET_KEY, String(next));
    } catch {
      // ignore storage errors
    }
  };

  const showTip = (e: React.MouseEvent, title: string, lines: string[]) => {
    const pad = 12;
    const x = Math.min(e.clientX + pad, window.innerWidth - 250);
    const y = Math.min(e.clientY + pad, window.innerHeight - 100);
    setTip({ x, y, title, lines });
  };

  // Precompute 7x24 matrix lookup
  const heatLookup = useMemo(() => {
    const map = new Map<string, RhythmCell>();
    for (const c of data?.rhythm ?? []) {
      map.set(`${c.weekday}-${c.hour}`, c);
    }
    let maxVal = 0;
    let peak: { weekday: number; hour: number; val: number } | null = null;
    for (const c of data?.rhythm ?? []) {
      const val =
        heatMode === "cost"
          ? c.costUsd
          : heatMode === "requests"
            ? c.requests
            : c.speedDurationMs > 0
              ? (c.speedOutputTokens * 1000) / c.speedDurationMs
              : 0;
      if (val > maxVal) {
        maxVal = val;
        peak = { weekday: c.weekday, hour: c.hour, val };
      }
    }
    return { map, maxVal, peak };
  }, [data?.rhythm, heatMode]);

  const heatColor =
    heatMode === "speed"
      ? "var(--accent-cyan)"
      : heatMode === "requests"
        ? "var(--accent-pink)"
        : "var(--accent-violet)";

  const hasTtft = useMemo(
    () => (data?.stability ?? []).some((m) => m.ttftP50Ms != null),
    [data?.stability],
  );
  const hasSpeed = useMemo(
    () => (data?.stability ?? []).some((m) => m.speedP50 != null),
    [data?.stability],
  );
  const effectiveStabMode: StabMode =
    !stabTouched && stabMode === "ttft" && !hasTtft && hasSpeed
      ? "speed"
      : stabMode;

  const cs = data?.cacheSavings;
  const fc = data?.forecast;
  const st = data?.status;
  const fullCost = (cs?.actualCostUsd ?? 0) + (cs?.savedUsd ?? 0);
  const savedPct =
    fullCost > 0 ? Math.round(((cs?.savedUsd ?? 0) / fullCost) * 100) : 0;
  const overBudget = (fc?.projected30dUsd ?? 0) > budget;
  const monthRatio = budget > 0 ? (fc?.projected30dUsd ?? 0) / budget : 0;
  const dailyRatio =
    dailyBudget > 0 ? (fc?.todayCostUsd ?? 0) / dailyBudget : 0;

  const budgetAlert = useMemo(() => {
    if (!fc) return null;
    if (dailyRatio >= 1) {
      return {
        level: "danger" as const,
        text: `今日已花费 ${fmtUsd(fc.todayCostUsd)}，已达每日预算 ${fmtUsd(dailyBudget)} 的 ${Math.round(dailyRatio * 100)}%。`,
      };
    }
    if (monthRatio >= 1) {
      return {
        level: "danger" as const,
        text: `按近 ${Math.max(1, Math.round(fc.elapsedDays))} 天有效日均推算，30 天预计花费 ${fmtUsd(fc.projected30dUsd)}，已超出月度预算 ${fmtUsd(budget)}。`,
      };
    }
    if (dailyRatio >= 0.8) {
      return {
        level: "warn" as const,
        text: `今日已花费 ${fmtUsd(fc.todayCostUsd)}，接近每日预算 ${fmtUsd(dailyBudget)}（${Math.round(dailyRatio * 100)}%）。`,
      };
    }
    if (monthRatio >= 0.8) {
      return {
        level: "warn" as const,
        text: `30 天预计花费 ${fmtUsd(fc.projected30dUsd)}，已达月度预算 ${fmtUsd(budget)} 的 ${Math.round(monthRatio * 100)}%。`,
      };
    }
    return null;
  }, [fc, dailyRatio, monthRatio, dailyBudget, budget]);

  const filteredSessions = useMemo(() => {
    const q = sessionQuery.trim().toLowerCase();
    const list = (data?.sessions ?? []).filter((s) => {
      if (projectFilter) {
        const norm = (s.projectDir ?? "").replace(/[/\\]+$/, "");
        if (norm !== projectFilter) return false;
      }
      if (!q) return true;
      return (
        s.sessionId.toLowerCase().includes(q) ||
        (s.title ?? "").toLowerCase().includes(q) ||
        (s.projectDir ?? "").toLowerCase().includes(q) ||
        (sourceName.get(s.appType) ?? s.appType).toLowerCase().includes(q)
      );
    });
    return [...list].sort((a, b) => {
      if (sessionSort === "turns") return b.turns - a.turns;
      if (sessionSort === "context")
        return b.avgContextTokens - a.avgContextTokens;
      if (sessionSort === "recent") return b.lastAt - a.lastAt;
      return b.costUsd - a.costUsd;
    });
  }, [data?.sessions, sessionQuery, sessionSort, projectFilter, sourceName]);

  return (
    <div className="insights-page">
      {/* Toolbar: App Filter with Original Icons + Range Pills */}
      <div className="insights-toolbar">
        <div
          className="insights-app-pills"
          role="group"
          aria-label="按应用筛选"
        >
          <button
            type="button"
            className={`insights-app-pill${app === "" ? " active" : ""}`}
            onClick={() => {
              setApp("");
              setStabTouched(false);
            }}
          >
            <span className="app-glyph" aria-hidden="true">
              <Layers3 size={14} />
            </span>
            <span>全部应用</span>
          </button>
          {visibleSources.map((s) => (
            <button
              type="button"
              key={s.id}
              className={`insights-app-pill${app === s.id ? " active" : ""}`}
              onClick={() => {
                setApp(s.id);
                setStabTouched(false);
              }}
            >
              <AppGlyph app={s.id} />
              <span>{s.name}</span>
            </button>
          ))}
        </div>

        <div className="insights-toolbar-right">
          {hasPeers && (
            <div
              className="insights-pill-group"
              role="group"
              aria-label="设备范围"
            >
              <button
                type="button"
                className={`insights-pill-btn${!effectiveMultiDevice ? " active" : ""}`}
                onClick={() => setMultiDevice(false)}
              >
                本机
              </button>
              <button
                type="button"
                className={`insights-pill-btn${effectiveMultiDevice ? " active" : ""}`}
                onClick={() => setMultiDevice(true)}
              >
                多设备 ({enabledPeers.length + 1})
              </button>
            </div>
          )}

          <div
            className="insights-pill-group"
            role="group"
            aria-label="时间范围"
          >
            {(
              [
                ["7d", "近 7 天"],
                ["14d", "近 14 天"],
                ["30d", "近 30 天"],
                ["month", "本月"],
              ] as const
            ).map(([k, label]) => (
              <button
                key={k}
                type="button"
                className={`insights-pill-btn${range === k ? " active" : ""}`}
                onClick={() => setRange(k)}
              >
                {label}
              </button>
            ))}
          </div>

          <button
            type="button"
            className="insights-export-btn"
            onClick={() => setReportOpen(true)}
            disabled={!data}
          >
            <FileText size={13} />
            <span>导出报告</span>
          </button>
        </div>
      </div>

      {budgetAlert && (
        <div className={`insights-alert ${budgetAlert.level}`} role="status">
          <AlertTriangle size={15} />
          <span>{budgetAlert.text}</span>
        </div>
      )}

      {query.isError && (
        <div className="insights-card">
          <div className="insights-empty">
            读取洞察数据失败：{String(query.error)}
          </div>
        </div>
      )}

      {/* Row 1: 3 Hero KPI Cards */}
      <section className="insights-kpi-grid">
        {/* 1. Cache Savings */}
        <div className="insights-card">
          <div className="insights-card-head">
            <div>
              <div className="insights-card-sub">缓存省钱金库</div>
              <div className="insights-kpi-value">
                <span
                  className="insights-kpi-num"
                  style={{ color: "var(--accent-green)" }}
                >
                  {fmtUsd(cs?.savedUsd ?? 0)}
                </span>
                <span className="insights-badge neutral">-{savedPct}%</span>
              </div>
            </div>
            <span className="insights-badge good">
              命中率 {((cs?.cacheHitRate ?? 0) * 100).toFixed(1)}%
            </span>
          </div>
          <div className="insights-bar-track">
            <div
              style={{
                width: `${fullCost > 0 ? ((cs?.actualCostUsd ?? 0) / fullCost) * 100 : 0}%`,
                background: "var(--accent-violet)",
              }}
            />
            <div
              style={{
                width: `${fullCost > 0 ? ((cs?.savedUsd ?? 0) / fullCost) * 100 : 0}%`,
                background: "var(--accent-green)",
              }}
            />
          </div>
          <div className="insights-meta-row">
            <span>
              <span
                className="insights-dot"
                style={{ background: "var(--accent-violet)" }}
              />
              实付 <strong>{fmtUsd(cs?.actualCostUsd ?? 0)}</strong>
            </span>
            <span>
              <span
                className="insights-dot"
                style={{ background: "var(--accent-green)" }}
              />
              若无缓存 <strong>{fmtUsd(fullCost)}</strong>
            </span>
          </div>
        </div>

        {/* 2. Context Size & Request Efficiency */}
        <div className="insights-card">
          <div className="insights-card-head">
            <div>
              <div className="insights-card-sub">平均单次上下文体积</div>
              <div className="insights-kpi-value">
                <span
                  className="insights-kpi-num"
                  style={{ color: "var(--accent-cyan)" }}
                >
                  {fmtTokens(cs?.avgContextTokens ?? 0)}
                </span>
                <span className="insights-card-sub">tokens / 次</span>
              </div>
            </div>
            <span className="insights-badge neutral">
              {(cs?.totalRequests ?? 0).toLocaleString()} 次请求
            </span>
          </div>
          <div className="insights-bar-track">
            <div
              style={{
                width: `${Math.min(100, ((cs?.avgContextTokens ?? 0) / 128_000) * 100)}%`,
                background:
                  (cs?.avgContextTokens ?? 0) > 80_000
                    ? "var(--accent-orange)"
                    : "var(--accent-cyan)",
              }}
            />
          </div>
          <div className="insights-meta-row">
            <span>
              单次均价{" "}
              <strong>
                {cs && cs.totalRequests > 0
                  ? `$${(cs.actualCostUsd / cs.totalRequests).toFixed(4)}`
                  : "—"}
              </strong>
            </span>
            <span>
              缓存读取费 <strong>{fmtUsd(cs?.cacheReadCostUsd ?? 0)}</strong>
            </span>
          </div>
        </div>

        {/* 3. Burn Rate & 30d Forecast */}
        <div className="insights-card">
          <div className="insights-card-head">
            <div>
              <div className="insights-card-sub">
                30 天预测 · 月预算 $
                <input
                  type="number"
                  min={10}
                  step={10}
                  className="insights-budget-input"
                  value={budget}
                  onChange={(e) => updateBudget(Number(e.target.value))}
                  aria-label="月度预算美元"
                />{" "}
                · 日预算 $
                <input
                  type="number"
                  min={1}
                  step={1}
                  className="insights-budget-input compact"
                  value={dailyBudget}
                  onChange={(e) => updateDailyBudget(Number(e.target.value))}
                  aria-label="每日预算美元"
                />
              </div>
              <div className="insights-kpi-value">
                <span className="insights-kpi-num">
                  {fmtUsd(fc?.projected30dUsd ?? 0)}
                </span>
                {fc?.deltaPct != null && (
                  <span
                    className={`insights-badge ${fc.deltaPct > 15 ? "warn" : "good"}`}
                  >
                    较上期 {fc.deltaPct >= 0 ? "+" : ""}
                    {fc.deltaPct.toFixed(1)}%
                  </span>
                )}
              </div>
            </div>
            <span
              className={`insights-badge ${overBudget ? "danger" : "good"}`}
            >
              {overBudget ? "预计超预算" : "预算内"}
            </span>
          </div>
          <ForecastSparkline
            dailyCosts={fc?.dailyCosts ?? []}
            projected={fc?.projected30dUsd ?? 0}
            budget={budget}
            onHover={showTip}
            onLeave={() => setTip(null)}
          />
          <div className="insights-meta-row">
            <span>
              有效 {Math.max(1, Math.round(fc?.elapsedDays ?? 1))} 天 · 日均{" "}
              <strong>{fmtUsd(fc?.dailyAvgUsd ?? 0)}/天</strong>
            </span>
            <span>
              {overBudget ? "预计超出 " : "距预算余 "}
              <strong>
                {fmtUsd(Math.abs(budget - (fc?.projected30dUsd ?? 0)))}
              </strong>
            </span>
          </div>
        </div>
      </section>

      {/* Active Subscription ROI (only shown when active subscriptions exist) */}
      {(data?.subscriptions?.length ?? 0) > 0 && (
        <section className="insights-card">
          <div className="insights-card-head">
            <div>
              <h2 className="insights-card-title">订阅回本仪</h2>
              <p className="insights-card-sub">
                对比订阅账单价与等效 API
                牌价（非美元币种保留原币种，不做估算汇率换算）
              </p>
            </div>
            <span className="insights-badge neutral">
              {data!.subscriptions.length} 个生效订阅
            </span>
          </div>
          <div className="insights-roi-grid">
            {data!.subscriptions.map((sub) => {
              const roi = sub.usdRoiMultiple;
              const pct =
                roi != null
                  ? Math.min(100, Math.max(4, (roi / Math.max(roi, 2)) * 100))
                  : 0;
              const markerPct =
                roi != null ? Math.min(95, (1 / Math.max(roi, 2)) * 100) : 0;
              return (
                <div key={sub.id} className="insights-roi-item">
                  <div className="insights-card-head">
                    <span className="insights-roi-title">
                      <AppGlyph app={sub.appType} />
                      <span>{sub.name}</span>
                    </span>
                    {roi != null ? (
                      <span
                        className={`insights-badge ${roi >= 1 ? "good" : "warn"}`}
                      >
                        {roi.toFixed(1)}× {roi >= 1 ? "已回本" : "未回本"}
                      </span>
                    ) : (
                      <span className="insights-badge neutral">
                        {formatSubscriptionPrice(
                          sub.price,
                          sub.currency,
                          sub.billing,
                          sub.customIntervalDays,
                        )}
                      </span>
                    )}
                  </div>
                  {roi != null && (
                    <div className="insights-roi-track">
                      <div
                        className="insights-roi-fill"
                        style={{ width: `${pct}%` }}
                      />
                      <div
                        className="insights-roi-marker"
                        style={{ left: `${markerPct}%` }}
                        title="回本线 (1.0×)"
                      />
                    </div>
                  )}
                  <div className="insights-meta-row">
                    <span>
                      本范围等效 <strong>{fmtUsd(sub.rangeApiCostUsd)}</strong>
                    </span>
                    <span>
                      本周期 <strong>{fmtUsd(sub.cycleApiCostUsd)}</strong> (
                      {sub.cycleRequests} 次)
                    </span>
                  </div>
                </div>
              );
            })}
          </div>
        </section>
      )}

      {/* Row 2: 7x24 Coding Rhythm Matrix */}
      <section className="insights-card">
        <div className="insights-card-head">
          <div>
            <h2 className="insights-card-title">7×24 编程节律矩阵</h2>
            <p className="insights-card-sub">
              按本地星期与小时透视活跃高峰，切换到「输出速度」可对比不同时段的模型响应快慢
            </p>
          </div>
          <div className="insights-pill-group subtle">
            {(
              [
                ["cost", "费用"],
                ["requests", "请求数"],
                ["speed", "输出速度"],
              ] as const
            ).map(([m, label]) => (
              <button
                key={m}
                type="button"
                className={`insights-pill-btn${heatMode === m ? " active" : ""}`}
                onClick={() => setHeatMode(m)}
              >
                {label}
              </button>
            ))}
          </div>
        </div>

        <div className="insights-heat-scroll">
          <div className="insights-heat-grid">
            <div />
            {Array.from({ length: 24 }, (_, h) => (
              <div key={h} className="insights-heat-axis">
                {h % 3 === 0 ? String(h).padStart(2, "0") : ""}
              </div>
            ))}
            {WEEKDAYS.map((dayLabel, d) => (
              <div key={dayLabel} style={{ display: "contents" }}>
                <div className="insights-heat-day">{dayLabel}</div>
                {Array.from({ length: 24 }, (_, h) => {
                  const cell = heatLookup.map.get(`${d}-${h}`);
                  const val = !cell
                    ? 0
                    : heatMode === "cost"
                      ? cell.costUsd
                      : heatMode === "requests"
                        ? cell.requests
                        : cell.speedDurationMs > 0
                          ? (cell.speedOutputTokens * 1000) /
                            cell.speedDurationMs
                          : 0;
                  const ratio =
                    heatLookup.maxVal > 0 ? val / heatLookup.maxVal : 0;
                  const pct =
                    val > 0 ? Math.max(14, Math.round(ratio * 100)) : 0;
                  const bg =
                    val > 0
                      ? `color-mix(in srgb, ${heatColor} ${pct}%, var(--soft))`
                      : "var(--soft)";
                  const metricLine =
                    heatMode === "cost"
                      ? `费用：${fmtUsd(val)}`
                      : heatMode === "requests"
                        ? `请求：${Math.round(val)} 次`
                        : val > 0
                          ? `速度：${val.toFixed(1)} Token/s`
                          : "无合格速度样本";
                  return (
                    <div
                      key={`${d}-${h}`}
                      className="insights-heat-cell"
                      style={{ background: bg }}
                      onMouseMove={(e) =>
                        showTip(
                          e,
                          `${dayLabel} ${String(h).padStart(2, "0")}:00–${String(h).padStart(2, "0")}:59`,
                          cell
                            ? [
                                metricLine,
                                `请求 ${cell.requests} 次 · 费用 ${fmtUsd(cell.costUsd)}`,
                              ]
                            : ["无调用记录"],
                        )
                      }
                      onMouseLeave={() => setTip(null)}
                    />
                  );
                })}
              </div>
            ))}
          </div>
        </div>

        <div className="insights-heat-footer">
          <div>
            {heatLookup.peak
              ? heatMode === "cost"
                ? `最烧钱时段：${WEEKDAYS[heatLookup.peak.weekday]} ${String(heatLookup.peak.hour).padStart(2, "0")}:00（${fmtUsd(heatLookup.peak.val)}）`
                : heatMode === "requests"
                  ? `最密集时段：${WEEKDAYS[heatLookup.peak.weekday]} ${String(heatLookup.peak.hour).padStart(2, "0")}:00（${Math.round(heatLookup.peak.val)} 次）`
                  : `最快响应时段：${WEEKDAYS[heatLookup.peak.weekday]} ${String(heatLookup.peak.hour).padStart(2, "0")}:00（${heatLookup.peak.val.toFixed(1)} Token/s）`
              : "当前时间范围内暂无明细数据"}
          </div>
          <div className="insights-heat-legend">
            <span>低</span>
            {[16, 40, 68, 100].map((p) => (
              <span
                key={p}
                className="insights-heat-swatch"
                style={{
                  background: `color-mix(in srgb, ${heatColor} ${p}%, var(--soft))`,
                }}
              />
            ))}
            <span>高</span>
          </div>
        </div>
      </section>

      {/* Row 3: Session Scatter + Stability Dumbbell */}
      <section className="insights-split-grid">
        <div className="insights-card">
          <div className="insights-card-head">
            <div>
              <h2 className="insights-card-title">
                会话健康度分布（轮次 × 费用 × 上下文体积）
              </h2>
              <p className="insights-card-sub">
                气泡越大代表单次平均上下文越长；点击任意气泡可下钻查看逐轮明细
              </p>
            </div>
            <span className="insights-badge neutral">
              Top {data?.sessions.length ?? 0} 会话
            </span>
          </div>
          <SessionScatter
            sessions={data?.sessions ?? []}
            sourceName={sourceName}
            onHover={showTip}
            onLeave={() => setTip(null)}
            onSelect={(s) => {
              setTip(null);
              setSelectedSession({
                sessionId: s.sessionId,
                appType: s.appType,
              });
            }}
          />
        </div>

        <div className="insights-card">
          <div className="insights-card-head">
            <div>
              <h2 className="insights-card-title">模型稳定性区间</h2>
              <p className="insights-card-sub">哑铃越短越稳定，暴露长尾抖动</p>
            </div>
            <div className="insights-pill-group subtle">
              <button
                type="button"
                className={`insights-pill-btn${effectiveStabMode === "ttft" ? " active" : ""}`}
                onClick={() => {
                  setStabTouched(true);
                  setStabMode("ttft");
                }}
              >
                首字 P50–P95
              </button>
              <button
                type="button"
                className={`insights-pill-btn${effectiveStabMode === "speed" ? " active" : ""}`}
                onClick={() => {
                  setStabTouched(true);
                  setStabMode("speed");
                }}
              >
                速度 P10–P50
              </button>
            </div>
          </div>
          <StabilityDumbbell
            items={data?.stability ?? []}
            mode={effectiveStabMode}
            onHover={showTip}
            onLeave={() => setTip(null)}
          />
        </div>
      </section>

      {/* Row 4: Token Mix 100% Stacked Area + Status Health */}
      <section className="insights-bottom-grid">
        <div className="insights-card">
          <div className="insights-card-head">
            <div>
              <h2 className="insights-card-title">
                Token 结构演变（100% 堆叠流图）
              </h2>
              <p className="insights-card-sub">
                观察缓存读取占比与新输入膨胀趋势
              </p>
            </div>
            <div className="insights-meta-row" style={{ gap: 12 }}>
              <span>
                <span
                  className="insights-dot"
                  style={{ background: "var(--accent-cyan)" }}
                />
                缓存读取
              </span>
              <span>
                <span
                  className="insights-dot"
                  style={{ background: "var(--accent-orange)" }}
                />
                缓存写入
              </span>
              <span>
                <span
                  className="insights-dot"
                  style={{ background: "var(--accent-violet)" }}
                />
                新输入
              </span>
              <span>
                <span
                  className="insights-dot"
                  style={{ background: "var(--accent-pink)" }}
                />
                输出
              </span>
            </div>
          </div>
          <TokenMixArea
            points={data?.tokenMix ?? []}
            onHover={showTip}
            onLeave={() => setTip(null)}
          />
        </div>

        <div
          className="insights-card"
          style={{ justifyContent: "space-between" }}
        >
          <div>
            <h2 className="insights-card-title">状态码与限流健康度</h2>
            <p className="insights-card-sub">快速定位 429 限流与 5xx 异常</p>
          </div>
          <StatusPanel
            status={st}
            onHover={showTip}
            onLeave={() => setTip(null)}
          />
        </div>
      </section>

      {/* Row 5: Project / Repo Cost Ranking */}
      {(data?.projects?.length ?? 0) > 0 && (
        <section className="insights-card">
          <div className="insights-card-head">
            <div>
              <h2 className="insights-card-title">项目 / 代码仓库费用排行</h2>
              <p className="insights-card-sub">
                按本地会话工作目录归因汇总；点击某个项目可筛选下方会话列表
              </p>
            </div>
            <span className="insights-badge neutral">
              {data!.projects.length} 个活跃项目
            </span>
          </div>
          <ProjectRanking
            projects={data!.projects}
            selectedDir={projectFilter}
            onSelect={(dir) =>
              setProjectFilter((prev) => (prev === dir ? null : dir))
            }
            onHover={showTip}
            onLeave={() => setTip(null)}
          />
        </section>
      )}

      {/* Row 6: Searchable Sessions Explorer */}
      <section className="insights-card">
        <div className="insights-card-head">
          <div>
            <h2 className="insights-card-title">会话排行与搜索</h2>
            <p className="insights-card-sub">
              按标题、项目路径或会话 ID 搜索，点击任意会话打开 Agent
              单次任务调用链瀑布图与逐轮明细
            </p>
          </div>
          <div className="insights-toolbar-right">
            <label className="insights-search-box">
              <Search size={13} />
              <input
                type="search"
                value={sessionQuery}
                onChange={(e) => setSessionQuery(e.target.value)}
                placeholder="搜索标题 / 项目 / ID…"
                aria-label="搜索会话"
              />
            </label>
            <div className="insights-pill-group subtle">
              {(
                [
                  ["cost", "费用"],
                  ["turns", "轮次"],
                  ["context", "上下文"],
                  ["recent", "最近"],
                ] as const
              ).map(([k, label]) => (
                <button
                  key={k}
                  type="button"
                  className={`insights-pill-btn${sessionSort === k ? " active" : ""}`}
                  onClick={() => setSessionSort(k)}
                >
                  {label}
                </button>
              ))}
            </div>
          </div>
        </div>

        {projectFilter && (
          <div className="insights-filter-chip-row">
            <span className="insights-filter-chip">
              <FolderGit2 size={12} />
              <span>已筛选项目：{projectBase(projectFilter)}</span>
              <button
                type="button"
                onClick={() => setProjectFilter(null)}
                aria-label="清除项目筛选"
              >
                <X size={12} />
              </button>
            </span>
          </div>
        )}

        {filteredSessions.length === 0 ? (
          <div className="insights-empty">未找到匹配的会话记录</div>
        ) : (
          <div className="insights-turns-wrap" style={{ maxHeight: 340 }}>
            <table className="insights-turns-table insights-session-table">
              <thead>
                <tr>
                  <th>会话</th>
                  <th>项目</th>
                  <th className="num">轮次</th>
                  <th className="num">平均上下文</th>
                  <th className="num">总输出</th>
                  <th className="num">费用</th>
                  <th className="num">最近活跃</th>
                </tr>
              </thead>
              <tbody>
                {filteredSessions.map((s) => {
                  const appLabel = sourceName.get(s.appType) ?? s.appType;
                  const shortId =
                    s.sessionId.length > 16
                      ? `${s.sessionId.slice(0, 8)}…${s.sessionId.slice(-4)}`
                      : s.sessionId;
                  return (
                    <tr
                      key={`${s.appType}:${s.sessionId}`}
                      className="insights-clickable-row"
                      onClick={() =>
                        setSelectedSession({
                          sessionId: s.sessionId,
                          appType: s.appType,
                        })
                      }
                    >
                      <td className="session-title-cell">
                        <span className="insights-session-name">
                          <AppGlyph app={s.appType} />
                          <span title={s.title ?? s.sessionId}>
                            {s.title ?? shortId}
                          </span>
                        </span>
                        <span className="insights-session-meta">
                          {appLabel}
                          {s.title ? ` · ${shortId}` : ""}
                        </span>
                      </td>
                      <td className="project-cell" title={s.projectDir ?? ""}>
                        {s.projectDir ? projectBase(s.projectDir) : "—"}
                      </td>
                      <td className="num">{s.turns}</td>
                      <td className="num">{fmtTokens(s.avgContextTokens)}</td>
                      <td className="num">{fmtTokens(s.outputTokens)}</td>
                      <td className="num">
                        <strong>{fmtUsd(s.costUsd)}</strong>
                      </td>
                      <td className="num">
                        {new Date(s.lastAt * 1000).toLocaleDateString("zh-CN", {
                          month: "2-digit",
                          day: "2-digit",
                          hour: "2-digit",
                          minute: "2-digit",
                          hour12: false,
                        })}
                      </td>
                    </tr>
                  );
                })}
              </tbody>
            </table>
          </div>
        )}
      </section>

      {reportOpen && data && (
        <ReportExportModal
          data={data}
          rangeLabel={RANGE_LABELS[range]}
          scopeLabel={
            app
              ? (sourceName.get(app) ?? app)
              : effectiveMultiDevice
                ? `全部应用 · 多设备 (${enabledPeers.length + 1})`
                : "全部应用 · 本机"
          }
          sourceName={sourceName}
          budget={budget}
          dailyBudget={dailyBudget}
          onClose={() => setReportOpen(false)}
        />
      )}

      {selectedSession && (
        <SessionDetailModal
          sessionId={selectedSession.sessionId}
          appType={selectedSession.appType}
          appName={
            sourceName.get(selectedSession.appType) ?? selectedSession.appType
          }
          startDate={startDate}
          endDate={endDate}
          multiDevice={effectiveMultiDevice}
          onClose={() => setSelectedSession(null)}
        />
      )}

      {tip && (
        <div className="insights-tooltip" style={{ left: tip.x, top: tip.y }}>
          <strong>{tip.title}</strong>
          {tip.lines.map((line, i) => (
            <div key={i} style={{ color: "var(--text-secondary)" }}>
              {line}
            </div>
          ))}
        </div>
      )}
    </div>
  );
}

function ForecastSparkline({
  dailyCosts,
  projected,
  budget,
  onHover,
  onLeave,
}: {
  dailyCosts: DailyCostPoint[];
  projected: number;
  budget: number;
  onHover: (e: React.MouseEvent, title: string, lines: string[]) => void;
  onLeave: () => void;
}) {
  const [hoverIdx, setHoverIdx] = useState<number | null>(null);
  const cumulative = useMemo(() => {
    let acc = 0;
    return dailyCosts.map((d) => {
      acc += d.cost;
      return acc;
    });
  }, [dailyCosts]);

  const actualEnd = cumulative[cumulative.length - 1] ?? 0;
  const maxY = Math.max(actualEnd, projected, budget, 1) * 1.15;
  const yOf = (v: number) => 44 - (v / maxY) * 38;
  const splitX = 180;
  const xOf = (i: number) =>
    cumulative.length >= 2 ? (i / (cumulative.length - 1)) * splitX : splitX;
  const pts =
    cumulative.length >= 2
      ? cumulative
          .map((v, i) => `${xOf(i).toFixed(1)},${yOf(v).toFixed(1)}`)
          .join(" ")
      : `0,${yOf(0).toFixed(1)} ${splitX},${yOf(actualEnd).toFixed(1)}`;
  const budgetY = yOf(budget);
  const actualY = yOf(actualEnd);
  const projY = yOf(projected);
  const bandW =
    dailyCosts.length > 0 ? splitX / Math.max(1, dailyCosts.length) : splitX;

  return (
    <svg
      className="insights-svg"
      viewBox="0 0 280 50"
      style={{ height: 48 }}
      onMouseLeave={() => {
        setHoverIdx(null);
        onLeave();
      }}
    >
      <line
        x1={0}
        y1={budgetY}
        x2={280}
        y2={budgetY}
        stroke="var(--accent-red)"
        strokeWidth={1}
        strokeDasharray="3 3"
        opacity={0.6}
      />
      <polyline
        fill="none"
        stroke="var(--accent-violet)"
        strokeWidth={2.2}
        strokeLinecap="round"
        strokeLinejoin="round"
        points={pts}
      />
      <line
        x1={splitX}
        y1={actualY}
        x2={274}
        y2={projY}
        stroke="var(--accent-violet)"
        strokeWidth={2}
        strokeDasharray="4 3"
        opacity={0.7}
      />
      {hoverIdx != null && dailyCosts[hoverIdx] && (
        <>
          <line
            x1={xOf(hoverIdx)}
            y1={4}
            x2={xOf(hoverIdx)}
            y2={46}
            stroke="var(--accent-violet)"
            strokeWidth={1}
            strokeDasharray="2 2"
            opacity={0.55}
          />
          <circle
            cx={xOf(hoverIdx)}
            cy={yOf(cumulative[hoverIdx])}
            r={3.5}
            fill="var(--accent-violet)"
            stroke="var(--surface)"
            strokeWidth={1.2}
          />
        </>
      )}
      <circle cx={splitX} cy={actualY} r={3.5} fill="var(--accent-violet)" />
      <circle
        cx={274}
        cy={projY}
        r={3.5}
        fill={projected > budget ? "var(--accent-red)" : "var(--accent-green)"}
      />
      {dailyCosts.map((d, i) => (
        <rect
          key={d.date}
          x={Math.max(0, xOf(i) - bandW / 2)}
          y={0}
          width={bandW}
          height={50}
          fill="transparent"
          style={{ cursor: "crosshair" }}
          onMouseMove={(e) => {
            setHoverIdx(i);
            onHover(e, d.date, [
              `当日费用：${fmtUsd(d.cost)}`,
              `范围累计：${fmtUsd(cumulative[i])}`,
            ]);
          }}
        />
      ))}
      <rect
        x={splitX + 2}
        y={0}
        width={280 - splitX - 2}
        height={50}
        fill="transparent"
        style={{ cursor: "crosshair" }}
        onMouseMove={(e) => {
          setHoverIdx(null);
          onHover(e, "30 天预测趋势", [
            `当前累计：${fmtUsd(actualEnd)}`,
            `30 天预测：${fmtUsd(projected)}`,
            `月度预算：${fmtUsd(budget)}（${projected > budget ? "预计超支" : "预算内"}）`,
          ]);
        }}
      />
    </svg>
  );
}

function SessionScatter({
  sessions,
  sourceName,
  onHover,
  onLeave,
  onSelect,
}: {
  sessions: SessionBubble[];
  sourceName: Map<string, string>;
  onHover: (e: React.MouseEvent, title: string, lines: string[]) => void;
  onLeave: () => void;
  onSelect: (session: SessionBubble) => void;
}) {
  const scatterSessions = sessions.slice(0, 40);
  if (scatterSessions.length === 0) {
    return (
      <div className="insights-empty">当前筛选范围内暂无会话级明细记录</div>
    );
  }
  const W = 560;
  const H = 220;
  const padL = 48;
  const padR = 18;
  const padT = 14;
  const padB = 28;
  const maxTurns = Math.max(10, ...scatterSessions.map((s) => s.turns)) * 1.1;
  const maxCost =
    Math.max(0.1, ...scatterSessions.map((s) => s.costUsd)) * 1.15;
  const maxCtx = Math.max(
    10_000,
    ...scatterSessions.map((s) => s.avgContextTokens),
  );
  const xOf = (t: number) => padL + (t / maxTurns) * (W - padL - padR);
  const yOf = (c: number) => H - padB - (c / maxCost) * (H - padT - padB);

  const yTicks = [0, 0.25, 0.5, 0.75, 1].map((p) => maxCost * p);
  const xTicks = [0, 0.25, 0.5, 0.75, 1].map((p) => Math.round(maxTurns * p));

  return (
    <svg
      className="insights-svg"
      viewBox={`0 0 ${W} ${H}`}
      style={{ height: 220 }}
    >
      {yTicks.map((v, i) => {
        const y = yOf(v);
        return (
          <g key={i}>
            <line
              x1={padL}
              y1={y}
              x2={W - padR}
              y2={y}
              stroke="var(--line)"
              strokeDasharray="3 3"
              opacity={0.7}
            />
            <text
              x={padL - 8}
              y={y + 4}
              textAnchor="end"
              fill="var(--muted)"
              fontSize={11}
            >
              {fmtUsd(v)}
            </text>
          </g>
        );
      })}
      {xTicks.map((v, i) => (
        <text
          key={i}
          x={xOf(v)}
          y={H - 8}
          textAnchor="middle"
          fill="var(--muted)"
          fontSize={11}
        >
          {v} 轮
        </text>
      ))}
      {scatterSessions.map((s) => {
        const cx = xOf(s.turns);
        const cy = yOf(s.costUsd);
        const r = 5 + (s.avgContextTokens / maxCtx) * 12;
        const col = appColor(s.appType);
        const label = sourceName.get(s.appType) ?? s.appType;
        const shortId =
          s.sessionId.length > 18
            ? `${s.sessionId.slice(0, 8)}…${s.sessionId.slice(-6)}`
            : s.sessionId;
        const heading = s.title ? s.title : `${label} · ${shortId}`;
        const proj = projectBase(s.projectDir);
        return (
          <circle
            key={`${s.appType}:${s.sessionId}`}
            cx={cx}
            cy={cy}
            r={r}
            fill={col}
            fillOpacity={0.55}
            stroke={col}
            strokeWidth={1.5}
            style={{ cursor: "pointer" }}
            onClick={() => onSelect(s)}
            onMouseMove={(e) =>
              onHover(e, heading, [
                `${label}${proj ? ` · ${proj}` : ""} · ${s.turns} 轮`,
                `费用 ${fmtUsd(s.costUsd)} · 平均上下文 ${fmtTokens(s.avgContextTokens)}`,
                "点击查看逐轮明细",
              ])
            }
            onMouseLeave={onLeave}
          />
        );
      })}
    </svg>
  );
}

function StabilityDumbbell({
  items,
  mode,
  onHover,
  onLeave,
}: {
  items: ModelStability[];
  mode: StabMode;
  onHover: (e: React.MouseEvent, title: string, lines: string[]) => void;
  onLeave: () => void;
}) {
  const rows = items
    .map((m) => {
      const lo = mode === "ttft" ? m.ttftP50Ms : m.speedP10;
      const hi = mode === "ttft" ? m.ttftP95Ms : m.speedP50;
      const est = mode === "ttft" ? m.ttftEstimated : m.speedEstimated;
      return { raw: m, model: m.model, lo, hi, est };
    })
    .filter(
      (
        r,
      ): r is {
        raw: ModelStability;
        model: string;
        lo: number;
        hi: number;
        est: boolean;
      } => r.lo != null && r.hi != null,
    );

  if (rows.length === 0) {
    return (
      <div className="insights-empty">
        当前范围暂无足够的{mode === "ttft" ? "首字耗时" : "输出速度"}样本
      </div>
    );
  }

  const maxVal = Math.max(1, ...rows.map((r) => Math.max(r.lo, r.hi))) * 1.1;
  const unit = mode === "ttft" ? "ms" : "tok/s";
  const palette = [
    "var(--accent-violet)",
    "var(--accent-cyan)",
    "var(--accent-pink)",
    "var(--accent-orange)",
    "var(--accent-blue)",
    "var(--accent-green)",
  ];

  return (
    <div className="insights-dumbbell-list">
      {rows.map((r, i) => {
        const minV = Math.min(r.lo, r.hi);
        const maxV = Math.max(r.lo, r.hi);
        const leftPct = (minV / maxVal) * 100;
        const widthPct = Math.max(1.5, ((maxV - minV) / maxVal) * 100);
        const color = palette[i % palette.length];
        const fmt = (v: number) =>
          mode === "ttft" ? String(Math.round(v)) : v.toFixed(1);
        const tipLines = [`样本数：${r.raw.requests.toLocaleString()} 次`];
        if (r.raw.ttftP50Ms != null && r.raw.ttftP95Ms != null) {
          tipLines.push(
            `首字延迟：P50 ${Math.round(r.raw.ttftP50Ms)}ms · P95 ${Math.round(r.raw.ttftP95Ms)}ms${r.raw.ttftEstimated ? "（估算）" : ""}`,
          );
        }
        if (r.raw.speedP10 != null && r.raw.speedP50 != null) {
          tipLines.push(
            `输出速度：P10 ${r.raw.speedP10.toFixed(1)} · P50 ${r.raw.speedP50.toFixed(1)} tok/s${r.raw.speedEstimated ? "（估算）" : ""}`,
          );
        }
        return (
          <div
            key={r.model}
            className="insights-dumbbell-item"
            onMouseMove={(e) => onHover(e, r.model, tipLines)}
            onMouseLeave={onLeave}
          >
            <div className="insights-dumbbell-head">
              <span className="insights-dumbbell-model" title={r.model}>
                {r.model}
              </span>
              <span className="insights-dumbbell-val">
                {r.est ? "≈ " : ""}
                {fmt(minV)} – {fmt(maxV)} {unit}
              </span>
            </div>
            <div className="insights-dumbbell-track">
              <div className="insights-dumbbell-rail" />
              <div
                className="insights-dumbbell-span"
                style={{
                  left: `${leftPct}%`,
                  width: `${widthPct}%`,
                  background: color,
                }}
              />
              <div
                className="insights-dumbbell-node"
                style={{ left: `${leftPct}%`, background: color }}
              />
              <div
                className="insights-dumbbell-node"
                style={{ left: `${leftPct + widthPct}%`, background: color }}
              />
            </div>
          </div>
        );
      })}
    </div>
  );
}

function TokenMixArea({
  points,
  onHover,
  onLeave,
}: {
  points: DailyTokenMix[];
  onHover: (e: React.MouseEvent, title: string, lines: string[]) => void;
  onLeave: () => void;
}) {
  const [hoverIdx, setHoverIdx] = useState<number | null>(null);
  const valid = points.filter(
    (p) => p.freshInput + p.output + p.cacheCreation + p.cacheRead > 0,
  );
  if (valid.length === 0) {
    return <div className="insights-empty">当前范围暂无 Token 结构数据</div>;
  }

  const W = 640;
  const H = 165;
  const padL = 34;
  const padR = 12;
  const padT = 10;
  const padB = 24;
  const xOf = (i: number) =>
    valid.length === 1
      ? (padL + W - padR) / 2
      : padL + (i / (valid.length - 1)) * (W - padL - padR);
  const yOf = (p: number) => H - padB - (p / 100) * (H - padT - padB);

  const normalized = valid.map((p) => {
    const sum = p.freshInput + p.output + p.cacheCreation + p.cacheRead;
    return [
      (p.cacheRead / sum) * 100,
      (p.cacheCreation / sum) * 100,
      (p.freshInput / sum) * 100,
      (p.output / sum) * 100,
    ];
  });

  const colors = [
    "var(--accent-cyan)",
    "var(--accent-orange)",
    "var(--accent-violet)",
    "var(--accent-pink)",
  ];

  const base = new Array(valid.length).fill(0);
  const polygons = colors.map((color, layer) => {
    const top = base.map((b, i) => b + normalized[i][layer]);
    const pts =
      valid.length === 1
        ? `${padL},${yOf(top[0])} ${W - padR},${yOf(top[0])} ${W - padR},${yOf(base[0])} ${padL},${yOf(base[0])}`
        : `${top.map((v, i) => `${xOf(i).toFixed(1)},${yOf(v).toFixed(1)}`).join(" ")} ${base
            .map((v, i) => `${xOf(i).toFixed(1)},${yOf(v).toFixed(1)}`)
            .reverse()
            .join(" ")}`;
    for (let i = 0; i < valid.length; i++) base[i] = top[i];
    return { color, pts };
  });

  return (
    <svg
      className="insights-svg"
      viewBox={`0 0 ${W} ${H}`}
      style={{ height: 165 }}
      onMouseLeave={() => {
        setHoverIdx(null);
        onLeave();
      }}
    >
      {[0, 50, 100].map((p) => (
        <g key={p}>
          <line
            x1={padL}
            y1={yOf(p)}
            x2={W - padR}
            y2={yOf(p)}
            stroke="var(--line)"
            strokeDasharray="3 3"
            opacity={0.6}
          />
          <text
            x={padL - 6}
            y={yOf(p) + 4}
            textAnchor="end"
            fill="var(--muted)"
            fontSize={11}
          >
            {p}%
          </text>
        </g>
      ))}
      {polygons.map((poly, idx) => (
        <polygon key={idx} points={poly.pts} fill={poly.color} opacity={0.75} />
      ))}
      {hoverIdx != null && valid[hoverIdx] && (
        <line
          x1={xOf(hoverIdx)}
          y1={padT}
          x2={xOf(hoverIdx)}
          y2={H - padB}
          stroke="var(--text)"
          strokeWidth={1.2}
          strokeDasharray="3 2"
          opacity={0.55}
        />
      )}
      {valid.map((p, i) => {
        const showLabel =
          i === 0 ||
          i === valid.length - 1 ||
          i % Math.max(1, Math.ceil(valid.length / 6)) === 0;
        const pcts = normalized[i];
        const sum = p.freshInput + p.output + p.cacheCreation + p.cacheRead;
        return (
          <g key={p.date}>
            {showLabel && (
              <text
                x={xOf(i)}
                y={H - 6}
                textAnchor="middle"
                fill="var(--muted)"
                fontSize={11}
              >
                {p.date.slice(5)}
              </text>
            )}
            <rect
              x={xOf(i) - (W - padL - padR) / Math.max(2, valid.length * 2)}
              y={padT}
              width={(W - padL - padR) / Math.max(1, valid.length)}
              height={H - padT - padB}
              fill="transparent"
              style={{ cursor: "crosshair" }}
              onMouseMove={(e) => {
                setHoverIdx(i);
                onHover(e, p.date, [
                  `总消耗 ${fmtTokens(sum)}`,
                  `缓存读取 ${pcts[0].toFixed(1)}% (${fmtTokens(p.cacheRead)})`,
                  `缓存写入 ${pcts[1].toFixed(1)}% (${fmtTokens(p.cacheCreation)})`,
                  `新输入 ${pcts[2].toFixed(1)}% (${fmtTokens(p.freshInput)})`,
                  `输出 ${pcts[3].toFixed(1)}% (${fmtTokens(p.output)})`,
                ]);
              }}
            />
          </g>
        );
      })}
    </svg>
  );
}

function StatusPanel({
  status,
  onHover,
  onLeave,
}: {
  status?: StatusHealth;
  onHover: (e: React.MouseEvent, title: string, lines: string[]) => void;
  onLeave: () => void;
}) {
  const total = status?.total ?? 0;
  if (total === 0) {
    return <div className="insights-empty">当前范围暂无状态码明细</div>;
  }
  const items = [
    {
      label: "2xx 正常完成",
      count: status!.ok2xx,
      color: "var(--accent-green)",
    },
    {
      label: "429 限流重试",
      count: status!.rateLimit429,
      color: "var(--accent-orange)",
    },
    {
      label: "5xx 服务端异常",
      count: status!.server5xx,
      color: "var(--accent-red)",
    },
    {
      label: "其他异常 / 中断",
      count: status!.otherError,
      color: "var(--muted)",
    },
  ];
  const okRate = (status!.ok2xx / total) * 100;
  const diag =
    status!.rateLimit429 > 0 && status!.peak429Hour != null
      ? `共记录 ${status!.rateLimit429} 次 429 限流，高发于 ${String(status!.peak429Hour).padStart(2, "0")}:00 时段。`
      : status!.server5xx > 0
        ? `共出现 ${status!.server5xx} 次 5xx 服务端错误，整体成功率 ${okRate.toFixed(2)}%。`
        : `整体成功率 ${okRate.toFixed(2)}%，当前范围无 429 限流或 5xx 异常。`;

  return (
    <>
      <div className="insights-bar-track">
        {items.map((it) => (
          <div
            key={it.label}
            style={{
              width: `${(it.count / total) * 100}%`,
              background: it.color,
              cursor: it.count > 0 ? "pointer" : "default",
            }}
            onMouseMove={(e) =>
              it.count > 0 &&
              onHover(e, it.label, [
                `请求数：${it.count.toLocaleString()} 次`,
                `占比：${((it.count / total) * 100).toFixed(2)}%（共 ${total.toLocaleString()} 次）`,
              ])
            }
            onMouseLeave={onLeave}
          />
        ))}
      </div>
      <div className="insights-status-rows">
        {items.map((it) => (
          <div key={it.label} className="insights-status-row">
            <span style={{ color: "var(--text-secondary)" }}>
              <span className="insights-dot" style={{ background: it.color }} />
              {it.label}
            </span>
            <span>
              <strong>{it.count.toLocaleString()}</strong>{" "}
              <span style={{ color: "var(--muted)" }}>
                ({((it.count / total) * 100).toFixed(1)}%)
              </span>
            </span>
          </div>
        ))}
      </div>
      <div className="insights-diag-box">
        <strong>自动诊断：</strong>
        {diag}
      </div>
    </>
  );
}

function fmtDurationMs(ms: number): string {
  if (!Number.isFinite(ms) || ms <= 0) return "0ms";
  if (ms < 1000) return `${Math.round(ms)}ms`;
  if (ms < 60_000) return `${(ms / 1000).toFixed(ms < 10_000 ? 2 : 1)}s`;
  const mins = Math.floor(ms / 60_000);
  const secs = Math.round((ms % 60_000) / 1000);
  return `${mins}m ${secs}s`;
}

function fmtClockMs(ms: number): string {
  const d = new Date(ms);
  const base = d.toLocaleTimeString("zh-CN", { hour12: false });
  const frac = String(Math.abs(ms) % 1000).padStart(3, "0");
  return `${base}.${frac}`;
}

function TaskWaterfall({
  task,
  onHover,
  onLeave,
}: {
  task: TaskTrace;
  onHover: (e: React.MouseEvent, title: string, lines: string[]) => void;
  onLeave: () => void;
}) {
  const [selectedSpanId, setSelectedSpanId] = useState<string | null>(null);

  useEffect(() => {
    setSelectedSpanId(null);
  }, [task.index]);

  const wallMs = Math.max(1, task.durationMs);
  const llmPct = Math.min(100, (task.llmMs / wallMs) * 100);
  const toolPct = Math.min(100 - llmPct, (task.toolMs / wallMs) * 100);
  const otherPct = Math.max(0, 100 - llmPct - toolPct);

  const selectedSpan =
    task.spans.find((s) => s.id === selectedSpanId) ?? task.spans[0] ?? null;

  const ticks = [0, 0.25, 0.5, 0.75, 1];

  return (
    <div className="insights-waterfall">
      <div className="insights-wf-summary">
        <div className="insights-wf-prompt-row">
          <div className="insights-wf-prompt">
            <span className="insights-badge neutral">任务 #{task.index}</span>
            <strong>{task.prompt ?? `任务段 #${task.index}`}</strong>
          </div>
          <div className="insights-wf-kpis">
            <span>总耗时 {fmtDurationMs(task.durationMs)}</span>
            <span>·</span>
            <span>{task.llmCalls} 次模型</span>
            {task.toolCalls > 0 && (
              <>
                <span>·</span>
                <span>{task.toolCalls} 次工具</span>
              </>
            )}
            {task.errorCount > 0 && (
              <>
                <span>·</span>
                <span className="insights-wf-err-text">
                  {task.errorCount} 异常
                </span>
              </>
            )}
            <span>·</span>
            <strong>{fmtUsd(task.totalCostUsd)}</strong>
          </div>
        </div>

        <div className="insights-wf-attr-row">
          <div className="insights-bar-track" style={{ height: 7, flex: 1 }}>
            <div
              style={{
                width: `${llmPct.toFixed(1)}%`,
                background: "var(--accent-violet)",
              }}
              title={`模型推理 ${fmtDurationMs(task.llmMs)}`}
            />
            <div
              style={{
                width: `${toolPct.toFixed(1)}%`,
                background: "var(--accent-cyan)",
              }}
              title={`工具执行 ${fmtDurationMs(task.toolMs)}`}
            />
            <div
              style={{
                width: `${otherPct.toFixed(1)}%`,
                background: "var(--border-subtle)",
              }}
              title="等待 / 间隔"
            />
          </div>
          <div className="insights-wf-legend">
            <span>
              <span
                className="insights-dot"
                style={{ background: "var(--accent-violet)" }}
              />
              模型 {fmtDurationMs(task.llmMs)} ({llmPct.toFixed(0)}%)
            </span>
            {task.toolMs > 0 && (
              <span>
                <span
                  className="insights-dot"
                  style={{ background: "var(--accent-cyan)" }}
                />
                工具 {fmtDurationMs(task.toolMs)} ({toolPct.toFixed(0)}%)
              </span>
            )}
          </div>
        </div>

        {task.topTools.length > 0 && (
          <div className="insights-wf-tools">
            <Wrench size={12} />
            {task.topTools.map((tool) => (
              <span
                key={tool.name}
                className={`insights-wf-tool-chip${tool.errorCount > 0 ? " err" : ""}`}
              >
                <code>{tool.name}</code>
                <span>×{tool.count}</span>
                <span className="muted">{fmtDurationMs(tool.totalMs)}</span>
              </span>
            ))}
          </div>
        )}
      </div>

      <div className="insights-wf-board">
        <div className="insights-wf-axis-row">
          <div className="insights-wf-label-col">调用节点（模型 / 工具）</div>
          <div className="insights-wf-track-col insights-wf-axis">
            {ticks.map((r) => (
              <span
                key={r}
                className="insights-wf-tick"
                style={{ left: `${r * 100}%` }}
              >
                {fmtDurationMs(wallMs * r)}
              </span>
            ))}
          </div>
        </div>

        <div className="insights-wf-rows">
          {task.spans.map((span) => {
            const leftPct = Math.min(99, (span.offsetMs / wallMs) * 100);
            const widthPct = Math.max(
              0.8,
              Math.min(100 - leftPct, (span.durationMs / wallMs) * 100),
            );
            const ttftRatio =
              span.kind === "llm" &&
              span.ttftMs != null &&
              span.ttftMs > 0 &&
              span.durationMs > 0
                ? Math.min(0.95, span.ttftMs / span.durationMs)
                : 0;
            const kindTag =
              span.status === "error"
                ? "ERR"
                : span.kind === "llm"
                  ? "LLM"
                  : span.kind === "subagent"
                    ? "AGENT"
                    : span.kind === "gap"
                      ? "GAP"
                      : "TOOL";
            const barClass =
              span.status === "error"
                ? "err"
                : span.kind === "llm"
                  ? "llm"
                  : span.kind === "subagent"
                    ? "subagent"
                    : span.kind === "gap"
                      ? "gap"
                      : "tool";
            const isSelected = selectedSpan?.id === span.id;
            const labelInsideRight = leftPct + widthPct > 82;

            const tipLines = [
              ...(span.detail ? [`详情：${span.detail}`] : []),
              `耗时：${fmtDurationMs(span.durationMs)}（偏移 +${fmtDurationMs(span.offsetMs)}）`,
              ...(span.ttftMs != null && span.ttftMs > 0
                ? [`首字 TTFT：${fmtDurationMs(span.ttftMs)}`]
                : []),
              ...(span.kind === "llm"
                ? [
                    `Token：新输入 ${fmtTokens(span.inputTokens)} · 缓存读 ${fmtTokens(span.cacheReadTokens)} · 输出 ${fmtTokens(span.outputTokens)}`,
                    `费用：${fmtUsd(span.costUsd)}`,
                  ]
                : []),
              ...(span.errorMessage ? [`异常：${span.errorMessage}`] : []),
            ];

            return (
              <button
                key={span.id}
                type="button"
                className={`insights-wf-row${isSelected ? " active" : ""}${span.depth > 0 ? " child" : ""}`}
                onClick={() => setSelectedSpanId(span.id)}
                onMouseMove={(e) =>
                  onHover(e, `[${kindTag}] ${span.label}`, tipLines)
                }
                onMouseLeave={onLeave}
              >
                <div className="insights-wf-label-col">
                  {span.depth > 0 && (
                    <span className="insights-wf-tree">└</span>
                  )}
                  <span className={`insights-wf-tag ${barClass}`}>
                    {kindTag}
                  </span>
                  <span className="insights-wf-name" title={span.label}>
                    {span.label}
                  </span>
                  {span.detail && (
                    <span className="insights-wf-detail" title={span.detail}>
                      {span.detail}
                    </span>
                  )}
                </div>
                <div className="insights-wf-track-col">
                  <div
                    className={`insights-wf-bar ${barClass}`}
                    style={{
                      left: `${leftPct.toFixed(2)}%`,
                      width: `${widthPct.toFixed(2)}%`,
                    }}
                  >
                    {ttftRatio > 0 && (
                      <div
                        className="insights-wf-ttft"
                        style={{ width: `${(ttftRatio * 100).toFixed(1)}%` }}
                        title={`TTFT ${fmtDurationMs(span.ttftMs!)}`}
                      />
                    )}
                  </div>
                  <span
                    className={`insights-wf-dur${labelInsideRight ? " left" : ""}`}
                    style={
                      labelInsideRight
                        ? { right: `${(100 - leftPct + 0.8).toFixed(2)}%` }
                        : { left: `${(leftPct + widthPct + 0.8).toFixed(2)}%` }
                    }
                  >
                    {fmtDurationMs(span.durationMs)}
                    {span.costUsd > 0 ? ` · ${fmtUsd(span.costUsd)}` : ""}
                  </span>
                </div>
              </button>
            );
          })}
        </div>
      </div>

      {selectedSpan && (
        <div className="insights-wf-inspector">
          <div className="insights-wf-insp-head">
            <span
              className={`insights-wf-tag ${selectedSpan.status === "error" ? "err" : selectedSpan.kind}`}
            >
              {selectedSpan.kind.toUpperCase()}
            </span>
            <strong>{selectedSpan.label}</strong>
            {selectedSpan.detail && (
              <span className="muted">{selectedSpan.detail}</span>
            )}
          </div>
          <div className="insights-wf-insp-meta">
            <span>
              起止 {fmtClockMs(selectedSpan.startMs)} →{" "}
              {fmtClockMs(selectedSpan.endMs)}
            </span>
            <span>
              偏移 +{fmtDurationMs(selectedSpan.offsetMs)} · 耗时{" "}
              <strong>{fmtDurationMs(selectedSpan.durationMs)}</strong>
            </span>
            {selectedSpan.ttftMs != null && selectedSpan.ttftMs > 0 && (
              <span>首字 TTFT {fmtDurationMs(selectedSpan.ttftMs)}</span>
            )}
            {selectedSpan.kind === "llm" && (
              <>
                <span>
                  新输入 {fmtTokens(selectedSpan.inputTokens)} · 缓存读{" "}
                  {fmtTokens(selectedSpan.cacheReadTokens)} · 输出{" "}
                  {fmtTokens(selectedSpan.outputTokens)}
                </span>
                <span>
                  费用 <strong>{fmtUsd(selectedSpan.costUsd)}</strong>
                </span>
              </>
            )}
            {selectedSpan.errorMessage && (
              <span className="insights-wf-err-text">
                异常：{selectedSpan.errorMessage}
              </span>
            )}
          </div>
        </div>
      )}
    </div>
  );
}

function SessionDetailModal({
  sessionId,
  appType,
  appName,
  startDate,
  endDate,
  multiDevice,
  onClose,
}: {
  sessionId: string;
  appType: string;
  appName: string;
  startDate: number;
  endDate: number;
  multiDevice: boolean;
  onClose: () => void;
}) {
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

  const query = useQuery({
    queryKey: [
      "usage",
      "session-trace",
      sessionId,
      appType,
      startDate,
      endDate,
      multiDevice,
    ],
    queryFn: () =>
      usageCall<SessionTraceDetail>("session_trace", {
        sessionId,
        appType,
        startDate,
        endDate,
        multiDevice,
      }),
  });

  const [copied, setCopied] = useState(false);
  const [viewMode, setViewMode] = useState<"waterfall" | "turns">("waterfall");
  const [selectedTaskIdx, setSelectedTaskIdx] = useState<number | null>(null);
  const [ctxHoverIdx, setCtxHoverIdx] = useState<number | null>(null);
  const [modalTip, setModalTip] = useState<{
    x: number;
    y: number;
    title: string;
    lines: string[];
  } | null>(null);

  const showModalTip = (
    e: React.MouseEvent,
    title: string,
    lines: string[],
  ) => {
    setModalTip({
      x: Math.min(e.clientX + 14, window.innerWidth - 310),
      y: Math.min(e.clientY + 14, window.innerHeight - 160),
      title,
      lines,
    });
  };

  const detail = query.data;
  const turns = detail?.turns ?? [];
  const tasks = useMemo(() => detail?.tasks ?? [], [detail?.tasks]);
  const ctxSeries = turns.map(
    (t) => t.freshInput + t.cacheReadTokens + t.cacheCreationTokens,
  );
  const maxCtx = Math.max(1, ...ctxSeries);

  const defaultTaskIdx = useMemo(() => {
    if (tasks.length === 0) return null;
    let best = tasks[0];
    for (const t of tasks) {
      const score = (x: TaskTrace) =>
        x.toolCalls * 1000 + x.totalCostUsd * 1000 + x.durationMs / 1000;
      if (score(t) > score(best)) best = t;
    }
    return best.index;
  }, [tasks]);

  const activeTask =
    tasks.find((t) => t.index === (selectedTaskIdx ?? defaultTaskIdx)) ??
    tasks[0] ??
    null;

  const copyResume = () => {
    if (!detail?.resumeCommand) return;
    void navigator.clipboard?.writeText(detail.resumeCommand).then(() => {
      setCopied(true);
      window.setTimeout(() => setCopied(false), 1600);
    });
  };

  return (
    <div
      className="insights-modal-backdrop"
      role="dialog"
      aria-modal="true"
      aria-label="Agent 调用链与会话明细"
      onClick={onClose}
    >
      <div className="insights-modal wide" onClick={(e) => e.stopPropagation()}>
        <div className="insights-modal-head">
          <div className="insights-modal-title-wrap">
            <div className="insights-modal-title">
              <AppGlyph app={appType} />
              <span>{detail?.title ?? appName}</span>
            </div>
            <div className="insights-modal-sub">
              <span>{appName}</span>
              <span>·</span>
              <code>{sessionId}</code>
              {detail?.projectDir && (
                <>
                  <span>·</span>
                  <span title={detail.projectDir}>{detail.projectDir}</span>
                </>
              )}
              {detail?.gitBranch && (
                <>
                  <span>·</span>
                  <span className="insights-branch-chip">
                    <GitBranch size={11} />
                    {detail.gitBranch}
                  </span>
                </>
              )}
            </div>
          </div>
          <div className="insights-modal-actions">
            {detail?.resumeCommand && (
              <button
                type="button"
                className="insights-copy-btn"
                onClick={copyResume}
                title={detail.resumeCommand}
              >
                {copied ? <Check size={13} /> : <Copy size={13} />}
                <span>{copied ? "已复制命令" : "复制恢复命令"}</span>
              </button>
            )}
            <button
              type="button"
              className="insights-modal-close"
              onClick={onClose}
              aria-label="关闭会话明细"
            >
              <X size={16} />
            </button>
          </div>
        </div>

        {query.isPending ? (
          <div className="insights-empty">正在解析调用链与会话明细…</div>
        ) : query.isError ? (
          <div className="insights-empty">读取失败：{String(query.error)}</div>
        ) : detail ? (
          <>
            <div className="insights-modal-bar">
              <div className="insights-modal-stats">
                <span className="insights-badge neutral">
                  {tasks.length} 个任务 · {detail.totalTurns} 轮调用
                </span>
                <span className="insights-badge neutral">
                  总费用 {fmtUsd(detail.totalCostUsd)}
                </span>
                <span className="insights-badge neutral">
                  总 Token {fmtTokens(detail.totalTokens)}
                </span>
                {ctxSeries.length > 0 && (
                  <span className="insights-badge neutral">
                    峰值上下文 {fmtTokens(maxCtx)}
                  </span>
                )}
                <span
                  className={`insights-badge ${detail.traceSource === "transcript" ? "good" : "neutral"}`}
                >
                  <Activity size={11} />{" "}
                  {detail.traceSource === "transcript"
                    ? "完整调用链（含工具）"
                    : "模型轮次瀑布"}
                </span>
              </div>

              <div className="insights-pill-group subtle">
                <button
                  type="button"
                  className={`insights-pill-btn${viewMode === "waterfall" ? " active" : ""}`}
                  onClick={() => setViewMode("waterfall")}
                >
                  <ListTree size={13} />
                  <span>调用链瀑布图</span>
                </button>
                <button
                  type="button"
                  className={`insights-pill-btn${viewMode === "turns" ? " active" : ""}`}
                  onClick={() => setViewMode("turns")}
                >
                  <Table2 size={13} />
                  <span>逐轮明细表</span>
                </button>
              </div>
            </div>

            {viewMode === "waterfall" ? (
              tasks.length === 0 ? (
                <div className="insights-empty">
                  当前时间范围内没有可展示的调用链任务
                </div>
              ) : (
                <div className="insights-wf-shell">
                  {tasks.length > 1 && (
                    <div className="insights-task-strip" role="tablist">
                      {tasks.map((t) => {
                        const active = activeTask?.index === t.index;
                        return (
                          <button
                            key={t.index}
                            type="button"
                            role="tab"
                            aria-selected={active}
                            className={`insights-task-pill${active ? " active" : ""}`}
                            onClick={() => setSelectedTaskIdx(t.index)}
                            title={t.prompt ?? `任务 #${t.index}`}
                          >
                            <span className="insights-task-pill-top">
                              <strong>#{t.index}</strong>
                              <span>{fmtDurationMs(t.durationMs)}</span>
                              {t.toolCalls > 0 && (
                                <span className="muted">
                                  {t.toolCalls} 工具
                                </span>
                              )}
                              {t.errorCount > 0 && (
                                <span className="insights-wf-err-text">!</span>
                              )}
                            </span>
                            <span className="insights-task-pill-prompt">
                              {t.prompt ?? `任务段 #${t.index}`}
                            </span>
                          </button>
                        );
                      })}
                    </div>
                  )}
                  {activeTask && (
                    <TaskWaterfall
                      task={activeTask}
                      onHover={showModalTip}
                      onLeave={() => setModalTip(null)}
                    />
                  )}
                </div>
              )
            ) : (
              <>
                {ctxSeries.length >= 2 && (
                  <div className="insights-modal-spark">
                    <div className="insights-card-sub">
                      上下文增长趋势（按轮次，悬浮查看单轮详情）
                    </div>
                    <svg
                      className="insights-svg"
                      viewBox="0 0 600 56"
                      style={{ height: 56 }}
                      onMouseLeave={() => {
                        setCtxHoverIdx(null);
                        setModalTip(null);
                      }}
                    >
                      <polyline
                        fill="none"
                        stroke="var(--accent-cyan)"
                        strokeWidth={2}
                        strokeLinecap="round"
                        strokeLinejoin="round"
                        points={ctxSeries
                          .map(
                            (v, i) =>
                              `${((i / (ctxSeries.length - 1)) * 588 + 6).toFixed(1)},${(50 - (v / maxCtx) * 42).toFixed(1)}`,
                          )
                          .join(" ")}
                      />
                      {ctxHoverIdx != null && turns[ctxHoverIdx] && (
                        <>
                          <line
                            x1={
                              (ctxHoverIdx / (ctxSeries.length - 1)) * 588 + 6
                            }
                            y1={4}
                            x2={
                              (ctxHoverIdx / (ctxSeries.length - 1)) * 588 + 6
                            }
                            y2={52}
                            stroke="var(--accent-cyan)"
                            strokeWidth={1}
                            strokeDasharray="2 2"
                            opacity={0.7}
                          />
                          <circle
                            cx={
                              (ctxHoverIdx / (ctxSeries.length - 1)) * 588 + 6
                            }
                            cy={50 - (ctxSeries[ctxHoverIdx] / maxCtx) * 42}
                            r={3.5}
                            fill="var(--accent-cyan)"
                            stroke="var(--surface)"
                            strokeWidth={1.2}
                          />
                        </>
                      )}
                      {turns.map((t, i) => {
                        const cx = (i / (ctxSeries.length - 1)) * 588 + 6;
                        const band = 588 / Math.max(1, ctxSeries.length - 1);
                        return (
                          <rect
                            key={t.requestId}
                            x={Math.max(0, cx - band / 2)}
                            y={0}
                            width={band}
                            height={56}
                            fill="transparent"
                            style={{ cursor: "crosshair" }}
                            onMouseMove={(e) => {
                              setCtxHoverIdx(i);
                              showModalTip(
                                e,
                                `第 ${i + 1} 轮 · ${new Date(t.createdAt * 1000).toLocaleTimeString("zh-CN", { hour12: false })}`,
                                [
                                  `模型：${t.model}`,
                                  `上下文体积：${fmtTokens(ctxSeries[i])}（新输入 ${fmtTokens(t.freshInput)} · 缓存读 ${fmtTokens(t.cacheReadTokens)}）`,
                                  `输出：${fmtTokens(t.outputTokens)} · 费用：${fmtUsd(t.totalCostUsd)}`,
                                ],
                              );
                            }}
                          />
                        );
                      })}
                    </svg>
                  </div>
                )}

                <div className="insights-turns-wrap">
                  <table className="insights-turns-table">
                    <thead>
                      <tr>
                        <th>#</th>
                        <th>时间</th>
                        <th>模型</th>
                        <th className="num">新输入</th>
                        <th className="num">缓存读</th>
                        <th className="num">输出</th>
                        <th className="num">耗时</th>
                        <th className="num">速度</th>
                        <th className="num">费用</th>
                        <th className="num">状态</th>
                      </tr>
                    </thead>
                    <tbody>
                      {turns.map((t, idx) => (
                        <tr key={t.requestId}>
                          <td className="num">{idx + 1}</td>
                          <td>
                            {new Date(t.createdAt * 1000).toLocaleTimeString(
                              "zh-CN",
                              { hour12: false },
                            )}
                          </td>
                          <td className="model-cell" title={t.model}>
                            {t.model}
                          </td>
                          <td className="num">{fmtTokens(t.freshInput)}</td>
                          <td className="num">
                            {fmtTokens(t.cacheReadTokens)}
                          </td>
                          <td className="num">{fmtTokens(t.outputTokens)}</td>
                          <td className="num">
                            {t.latencyMs > 0
                              ? `${(t.latencyMs / 1000).toFixed(1)}s`
                              : "—"}
                          </td>
                          <td className="num">
                            {formatRequestSpeed({
                              outputTokens: t.outputTokens,
                              latencyMs: t.latencyMs,
                              firstTokenMs: t.firstTokenMs,
                              dataSource: t.dataSource,
                            })}
                          </td>
                          <td className="num">{fmtUsd(t.totalCostUsd)}</td>
                          <td className="num">
                            <span
                              className={`insights-status-code ${t.statusCode >= 200 && t.statusCode < 300 ? "ok" : "err"}`}
                            >
                              {t.statusCode || "ERR"}
                            </span>
                          </td>
                        </tr>
                      ))}
                    </tbody>
                  </table>
                </div>
              </>
            )}
          </>
        ) : null}
      </div>
      {modalTip && (
        <div
          className="insights-tooltip"
          style={{ left: modalTip.x, top: modalTip.y, zIndex: 95 }}
        >
          <strong>{modalTip.title}</strong>
          {modalTip.lines.map((line, i) => (
            <div key={i} style={{ color: "var(--text-secondary)" }}>
              {line}
            </div>
          ))}
        </div>
      )}
    </div>
  );
}

function ProjectRanking({
  projects,
  selectedDir,
  onSelect,
  onHover,
  onLeave,
}: {
  projects: ProjectCostRow[];
  selectedDir: string | null;
  onSelect: (dir: string) => void;
  onHover: (e: React.MouseEvent, title: string, lines: string[]) => void;
  onLeave: () => void;
}) {
  const peak = Math.max(0.01, ...projects.map((p) => p.costUsd));
  const totalCost = projects.reduce((acc, p) => acc + p.costUsd, 0);
  return (
    <div className="insights-project-list">
      {projects.map((p) => {
        const active = selectedDir === p.projectDir;
        const share = totalCost > 0 ? (p.costUsd / totalCost) * 100 : 0;
        return (
          <button
            key={p.projectDir}
            type="button"
            className={`insights-project-row${active ? " active" : ""}`}
            onClick={() => onSelect(p.projectDir)}
            onMouseMove={(e) =>
              onHover(e, p.projectName, [
                `路径：${p.projectDir}`,
                `费用：${fmtUsd(p.costUsd)}（占比 ${share.toFixed(1)}%）`,
                `规模：${p.sessions} 会话 · ${p.requests} 次请求 · ${fmtTokens(p.totalTokens)} Token`,
                "点击可按此项目筛选下方会话",
              ])
            }
            onMouseLeave={onLeave}
          >
            <div className="insights-project-head">
              <span className="insights-project-name">
                <FolderGit2 size={14} />
                <span>{p.projectName}</span>
                <span className="insights-project-apps">
                  {p.apps.map((id) => (
                    <AppGlyph key={id} app={id} />
                  ))}
                </span>
              </span>
              <span className="insights-project-stats">
                <span>
                  {p.sessions} 会话 · {p.requests} 次 ·{" "}
                  {fmtTokens(p.totalTokens)}
                </span>
                <strong>{fmtUsd(p.costUsd)}</strong>
              </span>
            </div>
            <div className="insights-bar-track" style={{ height: 6 }}>
              <div
                style={{
                  width: `${Math.max(2, (p.costUsd / peak) * 100)}%`,
                  background: active
                    ? "var(--accent-pink)"
                    : "var(--accent-violet)",
                }}
              />
            </div>
          </button>
        );
      })}
    </div>
  );
}

function ReportExportModal({
  data,
  rangeLabel,
  scopeLabel,
  sourceName,
  budget,
  dailyBudget,
  onClose,
}: {
  data: InsightsData;
  rangeLabel: string;
  scopeLabel: string;
  sourceName: Map<string, string>;
  budget: number;
  dailyBudget: number;
  onClose: () => void;
}) {
  const [mode, setMode] = useState<"html" | "markdown">("html");
  const [copied, setCopied] = useState(false);

  const reportOpts = useMemo(
    () => ({ data, rangeLabel, scopeLabel, sourceName, budget, dailyBudget }),
    [data, rangeLabel, scopeLabel, sourceName, budget, dailyBudget],
  );
  const markdown = useMemo(() => buildMarkdownReport(reportOpts), [reportOpts]);
  const html = useMemo(() => buildHtmlReport(reportOpts), [reportOpts]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

  const copyCurrent = () => {
    const content = mode === "html" ? html : markdown;
    void navigator.clipboard?.writeText(content).then(() => {
      setCopied(true);
      window.setTimeout(() => setCopied(false), 1600);
    });
  };

  const downloadFile = (content: string, mime: string, ext: "html" | "md") => {
    const blob = new Blob([content], { type: mime });
    const url = URL.createObjectURL(blob);
    const a = document.createElement("a");
    const stamp = new Date().toISOString().slice(0, 10);
    a.href = url;
    a.download = `observatory-report-${stamp}.${ext}`;
    a.click();
    URL.revokeObjectURL(url);
  };

  return (
    <div
      className="insights-modal-backdrop"
      role="dialog"
      aria-modal="true"
      aria-label="预览与导出复盘报告"
      onClick={onClose}
    >
      <div
        className="insights-modal wide report-modal"
        onClick={(e) => e.stopPropagation()}
      >
        <div className="insights-modal-head">
          <div className="insights-modal-title-wrap">
            <div className="insights-modal-title">
              <FileText size={16} />
              <span>AI 编程复盘报告（{rangeLabel}）</span>
            </div>
            <div className="insights-modal-sub">
              <span>{scopeLabel}</span>
              <span>·</span>
              <span>
                离线单文件 HTML 含内嵌 SVG 图表，支持浏览器打印存为 PDF
              </span>
            </div>
          </div>
          <div className="insights-modal-actions">
            <div className="insights-pill-group subtle">
              <button
                type="button"
                className={`insights-pill-btn${mode === "html" ? " active" : ""}`}
                onClick={() => setMode("html")}
              >
                <Eye size={13} />
                <span>图文预览</span>
              </button>
              <button
                type="button"
                className={`insights-pill-btn${mode === "markdown" ? " active" : ""}`}
                onClick={() => setMode("markdown")}
              >
                <Code2 size={13} />
                <span>Markdown</span>
              </button>
            </div>
            <button
              type="button"
              className="insights-copy-btn"
              onClick={copyCurrent}
            >
              {copied ? <Check size={13} /> : <Copy size={13} />}
              <span>
                {copied
                  ? "已复制"
                  : mode === "html"
                    ? "复制 HTML"
                    : "复制 Markdown"}
              </span>
            </button>
            <button
              type="button"
              className="insights-copy-btn primary"
              onClick={() =>
                downloadFile(html, "text/html;charset=utf-8", "html")
              }
            >
              <Download size={13} />
              <span>导出 HTML</span>
            </button>
            <button
              type="button"
              className="insights-copy-btn"
              onClick={() =>
                downloadFile(markdown, "text/markdown;charset=utf-8", "md")
              }
            >
              <Download size={13} />
              <span>导出 .md</span>
            </button>
            <button
              type="button"
              className="insights-modal-close"
              onClick={onClose}
              aria-label="关闭报告预览"
            >
              <X size={16} />
            </button>
          </div>
        </div>
        {mode === "html" ? (
          <iframe
            className="insights-report-frame"
            srcDoc={html}
            sandbox="allow-scripts"
            title="AI 编程复盘报告图文预览"
          />
        ) : (
          <pre className="insights-report-pre">{markdown}</pre>
        )}
      </div>
    </div>
  );
}
