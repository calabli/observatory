import { type LanStatus, type SourceStatus } from "../usage/types";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useEffect, useMemo, useState } from "react";
import {
  BUDGET_KEY,
  DAILY_BUDGET_KEY,
  fmtTokens,
  fmtUsd,
  type HeatMode,
  type InsightsData,
  projectBase,
  rangeBounds,
  type RangeKey,
  readNumberStorage,
  type RhythmCell,
  type SessionSort,
  type StabMode,
  WEEKDAYS,
} from "./insightsShared";
import { usageCall } from "../native/usageClient";
import { useLocalDayKey } from "../usage/useLocalDayKey";
import {
  AlertTriangle,
  FileText,
  FolderGit2,
  Layers3,
  Search,
  X,
} from "lucide-react";
import { AppGlyph } from "../usage/AppGlyph";
import { INSIGHTS_RANGES, RANGE_PRESETS } from "../usage/timeRange";
import {
  ForecastSparkline,
  ProjectRanking,
  SessionScatter,
  StabilityDumbbell,
  StatusPanel,
  TokenMixArea,
} from "./InsightsCharts";
import { formatSubscriptionPrice } from "../subscriptions/format";
import { ReportExportModal } from "./ReportExportModal";
import { SessionDetailModal } from "./SessionDetailModal";

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
  const [dayKey, syncDayKey] = useLocalDayKey();
  useEffect(() => {
    if (scanRevision.data?.epoch === undefined) return;
    syncDayKey();
    void qc.invalidateQueries({ queryKey: ["usage", "insights"] });
  }, [scanRevision.data?.epoch, qc, syncDayKey]);

  const visibleSources = useMemo(
    () => sources.filter((s) => s.id !== "claude-desktop"),
    [sources],
  );
  const sourceName = useMemo(
    () => new Map(visibleSources.map((s) => [s.id, s.name])),
    [visibleSources],
  );

  const bounds = useMemo(() => rangeBounds(range, dayKey), [range, dayKey]);

  const query = useQuery({
    queryKey: ["usage", "insights", range, dayKey, app, effectiveMultiDevice],
    queryFn: () =>
      usageCall<InsightsData>("insights", {
        ...rangeBounds(range),
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
            {INSIGHTS_RANGES.map((k) => (
              <button
                key={k}
                type="button"
                className={`insights-pill-btn${range === k ? " active" : ""}`}
                onClick={() => setRange(k)}
              >
                {RANGE_PRESETS[k].label}
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
          rangeLabel={RANGE_PRESETS[range].label}
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
          startDate={bounds.startDate}
          endDate={bounds.endDate}
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
