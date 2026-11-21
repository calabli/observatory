import { useMemo, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { Layers3 } from "lucide-react";
import { usageCall } from "../native/usageClient";
import { AppGlyph } from "../usage/AppGlyph";
import type { SourceStatus } from "../usage/types";
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
  turns: number;
  costUsd: number;
  avgContextTokens: number;
  outputTokens: number;
  lastAt: number;
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

interface InsightsData {
  cacheSavings: CacheSavings;
  forecast: CostForecast;
  rhythm: RhythmCell[];
  sessions: SessionBubble[];
  stability: ModelStability[];
  tokenMix: DailyTokenMix[];
  status: StatusHealth;
}

type RangeKey = "7d" | "14d" | "30d" | "month";
type HeatMode = "cost" | "requests" | "speed";
type StabMode = "ttft" | "speed";

const BUDGET_KEY = "observatory.insights.monthly-budget";
const WEEKDAYS = ["周一", "周二", "周三", "周四", "周五", "周六", "周日"];

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

function readBudget(): number {
  try {
    const raw = Number(localStorage.getItem(BUDGET_KEY));
    return Number.isFinite(raw) && raw > 0 ? raw : 180;
  } catch {
    return 180;
  }
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

export function InsightsWorkspace({
  sources,
  multiDevice,
  deviceIds,
}: {
  sources: SourceStatus[];
  multiDevice: boolean;
  deviceIds?: string[];
}) {
  const [range, setRange] = useState<RangeKey>("30d");
  const [app, setApp] = useState<string>("");
  const [heatMode, setHeatMode] = useState<HeatMode>("cost");
  const [stabMode, setStabMode] = useState<StabMode>("ttft");
  const [budget, setBudget] = useState<number>(readBudget);
  const [tip, setTip] = useState<{
    x: number;
    y: number;
    title: string;
    lines: string[];
  } | null>(null);

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
    queryKey: [
      "usage",
      "insights",
      range,
      app,
      multiDevice,
      deviceIds?.join(",") ?? "",
    ],
    queryFn: () =>
      usageCall<InsightsData>("insights", {
        startDate,
        endDate,
        appType: app || undefined,
        multiDevice,
        deviceIds: multiDevice ? deviceIds : undefined,
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

  const cs = data?.cacheSavings;
  const fc = data?.forecast;
  const st = data?.status;
  const fullCost = (cs?.actualCostUsd ?? 0) + (cs?.savedUsd ?? 0);
  const savedPct =
    fullCost > 0 ? Math.round(((cs?.savedUsd ?? 0) / fullCost) * 100) : 0;
  const overBudget = (fc?.projected30dUsd ?? 0) > budget;

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
            onClick={() => setApp("")}
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
              onClick={() => setApp(s.id)}
            >
              <AppGlyph app={s.id} />
              <span>{s.name}</span>
            </button>
          ))}
        </div>

        <div className="insights-pill-group" role="group" aria-label="时间范围">
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
      </div>

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
                30 天费用预测 · 预算 $
                <input
                  type="number"
                  min={10}
                  step={10}
                  className="insights-budget-input"
                  value={budget}
                  onChange={(e) => updateBudget(Number(e.target.value))}
                  aria-label="月度预算美元"
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
          />
          <div className="insights-meta-row">
            <span>
              日均 <strong>{fmtUsd(fc?.dailyAvgUsd ?? 0)}/天</strong>
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
                气泡越大代表单次平均上下文越长；右上角高耗会话提示及时开启新对话
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
                className={`insights-pill-btn${stabMode === "ttft" ? " active" : ""}`}
                onClick={() => setStabMode("ttft")}
              >
                首字 P50–P95
              </button>
              <button
                type="button"
                className={`insights-pill-btn${stabMode === "speed" ? " active" : ""}`}
                onClick={() => setStabMode("speed")}
              >
                速度 P10–P50
              </button>
            </div>
          </div>
          <StabilityDumbbell items={data?.stability ?? []} mode={stabMode} />
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
          <StatusPanel status={st} />
        </div>
      </section>

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
}: {
  dailyCosts: DailyCostPoint[];
  projected: number;
  budget: number;
}) {
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
  const pts =
    cumulative.length >= 2
      ? cumulative
          .map(
            (v, i) =>
              `${((i / (cumulative.length - 1)) * splitX).toFixed(1)},${yOf(v).toFixed(1)}`,
          )
          .join(" ")
      : `0,${yOf(0).toFixed(1)} ${splitX},${yOf(actualEnd).toFixed(1)}`;
  const budgetY = yOf(budget);
  const actualY = yOf(actualEnd);
  const projY = yOf(projected);

  return (
    <svg className="insights-svg" viewBox="0 0 280 50" style={{ height: 48 }}>
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
      <circle cx={splitX} cy={actualY} r={3.5} fill="var(--accent-violet)" />
      <circle
        cx={274}
        cy={projY}
        r={3.5}
        fill={projected > budget ? "var(--accent-red)" : "var(--accent-green)"}
      />
    </svg>
  );
}

function SessionScatter({
  sessions,
  sourceName,
  onHover,
  onLeave,
}: {
  sessions: SessionBubble[];
  sourceName: Map<string, string>;
  onHover: (e: React.MouseEvent, title: string, lines: string[]) => void;
  onLeave: () => void;
}) {
  if (sessions.length === 0) {
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
  const maxTurns = Math.max(10, ...sessions.map((s) => s.turns)) * 1.1;
  const maxCost = Math.max(0.1, ...sessions.map((s) => s.costUsd)) * 1.15;
  const maxCtx = Math.max(10_000, ...sessions.map((s) => s.avgContextTokens));
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
      {sessions.map((s) => {
        const cx = xOf(s.turns);
        const cy = yOf(s.costUsd);
        const r = 5 + (s.avgContextTokens / maxCtx) * 12;
        const col = appColor(s.appType);
        const label = sourceName.get(s.appType) ?? s.appType;
        const shortId =
          s.sessionId.length > 18
            ? `${s.sessionId.slice(0, 8)}…${s.sessionId.slice(-6)}`
            : s.sessionId;
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
            onMouseMove={(e) =>
              onHover(e, `${label} · ${shortId}`, [
                `${s.turns} 轮 · 费用 ${fmtUsd(s.costUsd)}`,
                `平均上下文 ${fmtTokens(s.avgContextTokens)} · 总输出 ${fmtTokens(s.outputTokens)}`,
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
}: {
  items: ModelStability[];
  mode: StabMode;
}) {
  const rows = items
    .map((m) => {
      const lo = mode === "ttft" ? m.ttftP50Ms : m.speedP10;
      const hi = mode === "ttft" ? m.ttftP95Ms : m.speedP50;
      const est = mode === "ttft" ? m.ttftEstimated : m.speedEstimated;
      return { model: m.model, lo, hi, est };
    })
    .filter(
      (r): r is { model: string; lo: number; hi: number; est: boolean } =>
        r.lo != null && r.hi != null,
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
        return (
          <div key={r.model}>
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
      {valid.map((p, i) => {
        const showLabel =
          i === 0 ||
          i === valid.length - 1 ||
          i % Math.max(1, Math.ceil(valid.length / 6)) === 0;
        const pcts = normalized[i];
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
              onMouseMove={(e) =>
                onHover(e, p.date, [
                  `缓存读取 ${pcts[0].toFixed(1)}% (${fmtTokens(p.cacheRead)})`,
                  `缓存写入 ${pcts[1].toFixed(1)}% (${fmtTokens(p.cacheCreation)})`,
                  `新输入 ${pcts[2].toFixed(1)}% (${fmtTokens(p.freshInput)})`,
                  `输出 ${pcts[3].toFixed(1)}% (${fmtTokens(p.output)})`,
                ])
              }
              onMouseLeave={onLeave}
            />
          </g>
        );
      })}
    </svg>
  );
}

function StatusPanel({ status }: { status?: StatusHealth }) {
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
            }}
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
