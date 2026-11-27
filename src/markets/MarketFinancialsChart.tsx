import { useEffect, useMemo, useRef, useState } from "react";
import { Maximize2, RefreshCw } from "lucide-react";
import {
  calcFloatingTooltipPos,
  computeFinancialPeriods,
  CURRENCY_SYM,
  fmtFinancialPeriod,
  fmtFinancialValue,
  fmtSigned,
  isEquitySymbol,
  type FinancialPeriodMode,
  type StockFinancials,
  toneColor,
  trendTone,
} from "./marketsShared";

export type FinancialChartTab = "income" | "cashflow" | "eps";
export type FinancialWindowMode = "5" | "10" | "all";

const COLOR_REVENUE = "#3b82f6";
const COLOR_NET_POS = "#10b981";
const COLOR_NEG = "#ef4444";
const COLOR_GROSS_MARGIN = "#f59e0b";
const COLOR_NET_MARGIN = "#06b6d4";
const COLOR_OCF = "#0ea5e9";
const COLOR_FCF = "#8b5cf6";
const COLOR_EPS = "#6366f1";

function fmtEps(v: number | null | undefined, currency: string): string {
  if (v == null || !Number.isFinite(v)) return "—";
  const sign = v < 0 ? "-" : "";
  const sym = CURRENCY_SYM[currency] ?? "";
  return `${sign}${sym}${Math.abs(v).toFixed(2)}`;
}

function fmtPct(v: number | null | undefined): string {
  if (v == null || !Number.isFinite(v)) return "—";
  return `${v.toFixed(1)}%`;
}

function fmtRatio(v: number | null | undefined): string {
  if (v == null || !Number.isFinite(v) || v <= 0) return "—";
  return v >= 100 ? v.toFixed(0) : v.toFixed(1);
}

/** Build a clean [min, max] axis covering 0 and all finite values with breathing room. */
function calcBarScale(values: number[]): { min: number; max: number } {
  const finite = values.filter((v) => Number.isFinite(v));
  if (finite.length === 0) return { min: 0, max: 1 };
  const rawMin = Math.min(0, ...finite);
  const rawMax = Math.max(0, ...finite);
  if (rawMin === 0 && rawMax === 0) return { min: 0, max: 1 };
  const topPad = rawMax > 0 ? rawMax * 0.14 : 0;
  const botPad = rawMin < 0 ? Math.abs(rawMin) * 0.14 : 0;
  return { min: rawMin - botPad, max: rawMax + topPad };
}

export function MarketFinancialsChart({
  symbol,
  financials,
  loading,
  error,
  colorScheme = "cn",
  expanded = false,
  onExpand,
  onRetry,
}: {
  symbol: string;
  financials?: StockFinancials | null;
  loading: boolean;
  error: string | null;
  colorScheme?: "cn" | "us";
  expanded?: boolean;
  onExpand?: () => void;
  onRetry?: () => void;
}) {
  const [periodMode, setPeriodMode] = useState<FinancialPeriodMode>("annual");
  const [chartTab, setChartTab] = useState<FinancialChartTab>("income");
  const [windowMode, setWindowMode] = useState<FinancialWindowMode>("10");
  const [hover, setHover] = useState<{
    idx: number;
    y: number;
    wrapW: number;
    wrapH: number;
    tipW: number;
    tipH: number;
  } | null>(null);
  const wrapRef = useRef<HTMLDivElement>(null);
  const tipRef = useRef<HTMLDivElement>(null);
  const [measuredW, setMeasuredW] = useState<number>(expanded ? 1040 : 720);

  useEffect(() => {
    setHover(null);
  }, [symbol, periodMode, chartTab, windowMode]);

  // If the current period mode has no rows but the other does, switch automatically.
  const annualCount = financials?.annual?.length ?? 0;
  const quarterlyCount = financials?.quarterly?.length ?? 0;
  const effectiveMode: FinancialPeriodMode =
    periodMode === "annual" && annualCount === 0 && quarterlyCount > 0
      ? "quarterly"
      : periodMode === "quarterly" && quarterlyCount === 0 && annualCount > 0
        ? "annual"
        : periodMode;

  useEffect(() => {
    const el = wrapRef.current;
    if (!el) return;
    const update = () => {
      const w = Math.floor(el.getBoundingClientRect().width);
      if (w > 120) setMeasuredW((prev) => (prev === w ? prev : w));
    };
    update();
    const obs = new ResizeObserver(update);
    obs.observe(el);
    return () => obs.disconnect();
  }, [expanded, annualCount, quarterlyCount, loading]);

  const rawPeriods = useMemo(
    () =>
      effectiveMode === "annual"
        ? (financials?.annual ?? [])
        : (financials?.quarterly ?? []),
    [financials, effectiveMode],
  );

  const allPeriods = useMemo(
    () => computeFinancialPeriods(rawPeriods, effectiveMode),
    [rawPeriods, effectiveMode],
  );

  const periods = useMemo(() => {
    if (windowMode === "all") return allPeriods;
    const limit = windowMode === "5" ? 5 : 10;
    return allPeriods.length > limit ? allPeriods.slice(-limit) : allPeriods;
  }, [allPeriods, windowMode]);

  const currency = financials?.currency || "USD";

  if (!isEquitySymbol(symbol)) {
    return (
      <div className="markets-chart-empty">
        当前标的为指数或非公司证券，无财务报表数据
      </div>
    );
  }

  if (loading && !financials) {
    return <div className="markets-chart-empty">正在加载财务报表数据…</div>;
  }

  if (error && !financials) {
    return (
      <div className="markets-chart-empty">
        <span>财报数据加载失败：{error}</span>
        {onRetry && (
          <button
            type="button"
            className="markets-mini-toggle"
            style={{ marginLeft: 8 }}
            onClick={onRetry}
          >
            <RefreshCw size={11} />
            <span>重试</span>
          </button>
        )}
      </div>
    );
  }

  if (periods.length === 0) {
    return (
      <div className="markets-chart-empty">
        暂无该标的的财务报表数据（部分 ETF / 新上市标的可能暂无公开财报）
      </div>
    );
  }

  const hoverIdx =
    hover != null && hover.idx >= 0 && hover.idx < periods.length
      ? hover.idx
      : null;
  const activeIdx = hoverIdx ?? periods.length - 1;
  const active = periods[activeIdx];

  const hasCashflow = periods.some(
    (p) => p.operatingCashFlow != null || p.freeCashFlow != null,
  );
  const hasEps = periods.some((p) => p.dilutedEps != null);
  const hasMargins =
    chartTab === "income" &&
    periods.some((p) => p.grossMarginPct != null || p.netMarginPct != null);

  // Dimensions
  const W = Math.max(320, measuredW);
  const H = expanded ? 320 : 230;
  const padL = 66;
  const padR = hasMargins ? 46 : 18;
  const padT = 18;
  const padB = 28;
  const plotW = Math.max(120, W - padL - padR);
  const plotH = H - padT - padB;

  const barValues: number[] = [];
  for (const p of periods) {
    if (chartTab === "income") {
      if (p.revenue != null) barValues.push(p.revenue);
      if (p.netIncome != null) barValues.push(p.netIncome);
    } else if (chartTab === "cashflow") {
      if (p.operatingCashFlow != null) barValues.push(p.operatingCashFlow);
      if (p.freeCashFlow != null) barValues.push(p.freeCashFlow);
    } else {
      if (p.dilutedEps != null) barValues.push(p.dilutedEps);
    }
  }
  const { min: barMin, max: barMax } = calcBarScale(barValues);
  const barSpan = Math.max(1e-9, barMax - barMin);
  const yOfBar = (v: number) => padT + (1 - (v - barMin) / barSpan) * plotH;
  const zeroY = yOfBar(0);

  const yTicks = [0, 0.33, 0.67, 1].map((r) => barMin + barSpan * r);

  // Secondary Y scale for Gross/Net Margin %
  const marginValues: number[] = [];
  if (hasMargins) {
    for (const p of periods) {
      if (p.grossMarginPct != null) marginValues.push(p.grossMarginPct);
      if (p.netMarginPct != null) marginValues.push(p.netMarginPct);
    }
  }
  const mRawMin = marginValues.length ? Math.min(0, ...marginValues) : 0;
  const mRawMax = marginValues.length ? Math.max(20, ...marginValues) : 100;
  const mPad = Math.max((mRawMax - mRawMin) * 0.12, 4);
  const mMin = mRawMin < 0 ? mRawMin - mPad : 0;
  const mMax = mRawMax + mPad;
  const yOfMargin = (v: number) =>
    padT + (1 - (v - mMin) / Math.max(1e-9, mMax - mMin)) * plotH;
  const mTicks = [0, 0.5, 1].map((r) => mMin + (mMax - mMin) * r);

  const slotW = plotW / periods.length;
  const slotCenterX = (i: number) => padL + (i + 0.5) * slotW;

  const grossLinePts = hasMargins
    ? periods
        .map((p, i) =>
          p.grossMarginPct != null
            ? {
                x: slotCenterX(i),
                y: yOfMargin(p.grossMarginPct),
                v: p.grossMarginPct,
                i,
              }
            : null,
        )
        .filter(
          (pt): pt is { x: number; y: number; v: number; i: number } =>
            pt != null,
        )
    : [];
  const netLinePts = hasMargins
    ? periods
        .map((p, i) =>
          p.netMarginPct != null
            ? {
                x: slotCenterX(i),
                y: yOfMargin(p.netMarginPct),
                v: p.netMarginPct,
                i,
              }
            : null,
        )
        .filter(
          (pt): pt is { x: number; y: number; v: number; i: number } =>
            pt != null,
        )
    : [];

  const barRect = (x: number, w: number, val: number) => {
    const yVal = yOfBar(val);
    const y = Math.min(zeroY, yVal);
    const h = Math.max(1.5, Math.abs(zeroY - yVal));
    return { x, y, width: w, height: h };
  };

  const singleBarW = Math.min(44, Math.max(6, slotW * 0.52));
  const pairedBarW = Math.min(28, Math.max(3.5, slotW * 0.34));
  const pairGap = slotW < 18 ? 1.5 : Math.max(2, Math.min(5, slotW * 0.06));
  const minLabelW = effectiveMode === "annual" ? 44 : 42;
  const xLabelStep = Math.max(
    1,
    Math.ceil((periods.length * minLabelW) / Math.max(120, plotW)),
  );
  const showAllEpsLabels = slotW >= 44;

  const renderGrowth = (v: number | null, label: string) => {
    if (v == null) return null;
    return (
      <em
        className="markets-fin-growth"
        style={{ color: toneColor(trendTone(v, colorScheme)) }}
      >
        {label} {fmtSigned(v, "%")}
      </em>
    );
  };

  const handlePointerMove = (e: React.PointerEvent<HTMLDivElement>) => {
    const rect = e.currentTarget.getBoundingClientRect();
    const relX = ((e.clientX - rect.left) / Math.max(1, rect.width)) * W;
    if (relX < padL - 8 || relX > W - padR + 8) {
      setHover(null);
      return;
    }
    const rawIdx = Math.floor((relX - padL) / Math.max(1e-6, slotW));
    const idx = Math.max(0, Math.min(periods.length - 1, rawIdx));
    const y = e.clientY - rect.top;
    const tipW = tipRef.current?.offsetWidth || 222;
    const tipH = tipRef.current?.offsetHeight || 156;
    setHover((prev) =>
      prev &&
      prev.idx === idx &&
      Math.abs(prev.y - y) < 2 &&
      Math.abs(prev.wrapW - rect.width) < 1 &&
      Math.abs(prev.wrapH - rect.height) < 1 &&
      prev.tipW === tipW &&
      prev.tipH === tipH
        ? prev
        : { idx, y, wrapW: rect.width, wrapH: rect.height, tipW, tipH },
    );
  };

  return (
    <div className="markets-fin-shell">
      {/* Controls & Valuation strip */}
      <div className="markets-kline-toolbar">
        <div className="markets-kline-toolbar-left">
          <div className="markets-pill-group">
            <button
              type="button"
              className={`markets-pill-btn${effectiveMode === "annual" ? " active" : ""}`}
              disabled={annualCount === 0}
              onClick={() => setPeriodMode("annual")}
            >
              年报 ({annualCount})
            </button>
            <button
              type="button"
              className={`markets-pill-btn${effectiveMode === "quarterly" ? " active" : ""}`}
              disabled={quarterlyCount === 0}
              onClick={() => setPeriodMode("quarterly")}
            >
              季报 ({quarterlyCount})
            </button>
          </div>

          <div className="markets-pill-group">
            <button
              type="button"
              className={`markets-pill-btn${chartTab === "income" ? " active" : ""}`}
              onClick={() => setChartTab("income")}
            >
              营收与利润
            </button>
            <button
              type="button"
              className={`markets-pill-btn${chartTab === "cashflow" ? " active" : ""}`}
              disabled={!hasCashflow}
              onClick={() => setChartTab("cashflow")}
            >
              现金流
            </button>
            <button
              type="button"
              className={`markets-pill-btn${chartTab === "eps" ? " active" : ""}`}
              disabled={!hasEps}
              onClick={() => setChartTab("eps")}
            >
              每股收益
            </button>
          </div>

          {allPeriods.length > 5 && (
            <div className="markets-pill-group" aria-label="显示期数">
              <button
                type="button"
                className={`markets-pill-btn${windowMode === "5" ? " active" : ""}`}
                onClick={() => setWindowMode("5")}
              >
                近 5 期
              </button>
              {allPeriods.length > 10 && (
                <button
                  type="button"
                  className={`markets-pill-btn${windowMode === "10" ? " active" : ""}`}
                  onClick={() => setWindowMode("10")}
                >
                  近 10 期
                </button>
              )}
              <button
                type="button"
                className={`markets-pill-btn${windowMode === "all" || (windowMode === "10" && allPeriods.length <= 10) ? " active" : ""}`}
                onClick={() => setWindowMode("all")}
              >
                全部 ({allPeriods.length})
              </button>
            </div>
          )}
        </div>

        <div className="markets-kline-toolbar-right">
          <span className="markets-badge neutral">财报币种 {currency}</span>
          {financials?.trailingPe != null && (
            <span className="markets-badge neutral" title="滚动市盈率 (PE TTM)">
              PE {fmtRatio(financials.trailingPe)}
            </span>
          )}
          {financials?.trailingPs != null && (
            <span className="markets-badge neutral" title="滚动市销率 (PS TTM)">
              PS {fmtRatio(financials.trailingPs)}
            </span>
          )}
          {financials?.trailingPb != null && (
            <span className="markets-badge neutral" title="市净率 (PB)">
              PB {fmtRatio(financials.trailingPb)}
            </span>
          )}
          {!expanded && onExpand && (
            <button
              type="button"
              className="markets-mini-toggle"
              onClick={onExpand}
              title="放大查看财报图表"
            >
              <Maximize2 size={11} />
              <span>放大</span>
            </button>
          )}
        </div>
      </div>

      {/* Period KPI Readout (updates live on hover) */}
      <div className="markets-fin-kpi-strip">
        <div className="markets-fin-kpi">
          <span>报告期</span>
          <strong>{active.asOfDate}</strong>
        </div>
        {chartTab === "income" && (
          <>
            <div className="markets-fin-kpi">
              <span>营业收入</span>
              <strong>
                {fmtFinancialValue(active.revenue, currency)}
                {renderGrowth(
                  active.revenueYoYPct,
                  effectiveMode === "annual" ? "同比" : "同比",
                )}
                {effectiveMode === "quarterly" &&
                  active.revenueYoYPct == null &&
                  renderGrowth(active.revenueQoQPct, "环比")}
              </strong>
            </div>
            <div className="markets-fin-kpi">
              <span>净利润</span>
              <strong>
                {fmtFinancialValue(active.netIncome, currency)}
                {renderGrowth(active.netIncomeYoYPct, "同比")}
              </strong>
            </div>
            {active.grossMarginPct != null && (
              <div className="markets-fin-kpi">
                <span>毛利率</span>
                <strong>{fmtPct(active.grossMarginPct)}</strong>
              </div>
            )}
            {active.operatingMarginPct != null && (
              <div className="markets-fin-kpi">
                <span>营业利润率</span>
                <strong>{fmtPct(active.operatingMarginPct)}</strong>
              </div>
            )}
            {active.netMarginPct != null && (
              <div className="markets-fin-kpi">
                <span>净利率</span>
                <strong>{fmtPct(active.netMarginPct)}</strong>
              </div>
            )}
            {active.dilutedEps != null && (
              <div className="markets-fin-kpi">
                <span>稀释 EPS</span>
                <strong>{fmtEps(active.dilutedEps, currency)}</strong>
              </div>
            )}
          </>
        )}
        {chartTab === "cashflow" && (
          <>
            <div className="markets-fin-kpi">
              <span>经营现金流</span>
              <strong>
                {fmtFinancialValue(active.operatingCashFlow, currency)}
              </strong>
            </div>
            <div className="markets-fin-kpi">
              <span>自由现金流</span>
              <strong>
                {fmtFinancialValue(active.freeCashFlow, currency)}
              </strong>
            </div>
            {active.revenue != null &&
              active.revenue > 0 &&
              active.freeCashFlow != null && (
                <div className="markets-fin-kpi">
                  <span>FCF / 营收</span>
                  <strong>
                    {fmtPct((active.freeCashFlow / active.revenue) * 100)}
                  </strong>
                </div>
              )}
            {active.netIncome != null &&
              active.netIncome > 0 &&
              active.operatingCashFlow != null && (
                <div className="markets-fin-kpi">
                  <span>净现比 (OCF/净利)</span>
                  <strong>
                    {(active.operatingCashFlow / active.netIncome).toFixed(2)}×
                  </strong>
                </div>
              )}
          </>
        )}
        {chartTab === "eps" && (
          <>
            <div className="markets-fin-kpi">
              <span>稀释 EPS</span>
              <strong>{fmtEps(active.dilutedEps, currency)}</strong>
            </div>
            <div className="markets-fin-kpi">
              <span>营业收入</span>
              <strong>{fmtFinancialValue(active.revenue, currency)}</strong>
            </div>
            <div className="markets-fin-kpi">
              <span>净利润</span>
              <strong>{fmtFinancialValue(active.netIncome, currency)}</strong>
            </div>
          </>
        )}
      </div>

      {/* Legend */}
      <div className="markets-fin-legend">
        {chartTab === "income" && (
          <>
            <span>
              <i style={{ background: COLOR_REVENUE }} />
              营业收入
            </span>
            <span>
              <i style={{ background: COLOR_NET_POS }} />
              净利润（亏损为红）
            </span>
            {grossLinePts.length >= 2 && (
              <span>
                <em style={{ background: COLOR_GROSS_MARGIN }} />
                毛利率（右轴）
              </span>
            )}
            {netLinePts.length >= 2 && (
              <span>
                <em style={{ background: COLOR_NET_MARGIN }} />
                净利率（右轴）
              </span>
            )}
          </>
        )}
        {chartTab === "cashflow" && (
          <>
            <span>
              <i style={{ background: COLOR_OCF }} />
              经营现金流
            </span>
            <span>
              <i style={{ background: COLOR_FCF }} />
              自由现金流（负值为红）
            </span>
          </>
        )}
        {chartTab === "eps" && (
          <span>
            <i style={{ background: COLOR_EPS }} />
            稀释每股收益 ({currency})
          </span>
        )}
      </div>

      {/* Interactive SVG Canvas */}
      <div
        className="markets-chart-wrap"
        ref={wrapRef}
        onPointerMove={handlePointerMove}
        onPointerLeave={() => setHover(null)}
      >
        <svg
          width={W}
          height={H}
          viewBox={`0 0 ${W} ${H}`}
          className="markets-detail-svg"
          role="img"
          aria-label={`${symbol} 财务报表图表`}
        >
          {/* Horizontal gridlines + Left Y labels */}
          {yTicks.map((tv, i) => {
            const y = yOfBar(tv);
            return (
              <g key={`yl-${i}`}>
                <line
                  x1={padL}
                  y1={y}
                  x2={W - padR}
                  y2={y}
                  stroke="var(--line)"
                  strokeWidth={0.8}
                  strokeDasharray="3 3"
                />
                <text
                  x={padL - 7}
                  y={y + 3.5}
                  textAnchor="end"
                  fontSize="10"
                  fill="var(--muted)"
                >
                  {chartTab === "eps"
                    ? fmtEps(tv, currency)
                    : fmtFinancialValue(tv, currency)}
                </text>
              </g>
            );
          })}

          {/* Zero baseline */}
          <line
            x1={padL}
            y1={zeroY}
            x2={W - padR}
            y2={zeroY}
            stroke="var(--border-strong, var(--line))"
            strokeWidth={1.1}
          />

          {/* Right Y axis for margin percentages */}
          {hasMargins &&
            mTicks.map((mv, i) => {
              const y = yOfMargin(mv);
              return (
                <text
                  key={`yr-${i}`}
                  x={W - padR + 7}
                  y={y + 3.5}
                  textAnchor="start"
                  fontSize="10"
                  fill="var(--muted)"
                >
                  {mv.toFixed(0)}%
                </text>
              );
            })}

          {/* Hover column highlight */}
          {periods.map((p, i) => {
            const slotLeft = padL + i * slotW;
            const isActive = i === activeIdx;
            return (
              <rect
                key={`bg-${p.asOfDate}`}
                x={slotLeft + 2}
                y={padT}
                width={Math.max(4, slotW - 4)}
                height={plotH}
                rx={6}
                fill={isActive ? "var(--soft)" : "transparent"}
                opacity={isActive ? 0.85 : 0}
              />
            );
          })}

          {/* Bars */}
          {periods.map((p, i) => {
            const cx = slotCenterX(i);
            const isDim = hoverIdx != null && hoverIdx !== i;
            const opacity = isDim ? 0.55 : 0.94;

            if (chartTab === "income") {
              const leftX = cx - pairedBarW - pairGap / 2;
              const rightX = cx + pairGap / 2;
              const revBox =
                p.revenue != null
                  ? barRect(leftX, pairedBarW, p.revenue)
                  : null;
              const netBox =
                p.netIncome != null
                  ? barRect(rightX, pairedBarW, p.netIncome)
                  : null;
              return (
                <g key={`bar-${p.asOfDate}`}>
                  {revBox && (
                    <rect
                      x={revBox.x}
                      y={revBox.y}
                      width={revBox.width}
                      height={revBox.height}
                      rx={3}
                      fill={COLOR_REVENUE}
                      opacity={opacity}
                    />
                  )}
                  {netBox && (
                    <rect
                      x={netBox.x}
                      y={netBox.y}
                      width={netBox.width}
                      height={netBox.height}
                      rx={3}
                      fill={(p.netIncome ?? 0) >= 0 ? COLOR_NET_POS : COLOR_NEG}
                      opacity={opacity}
                    />
                  )}
                </g>
              );
            }

            if (chartTab === "cashflow") {
              const leftX = cx - pairedBarW - pairGap / 2;
              const rightX = cx + pairGap / 2;
              const ocfBox =
                p.operatingCashFlow != null
                  ? barRect(leftX, pairedBarW, p.operatingCashFlow)
                  : null;
              const fcfBox =
                p.freeCashFlow != null
                  ? barRect(rightX, pairedBarW, p.freeCashFlow)
                  : null;
              return (
                <g key={`cf-${p.asOfDate}`}>
                  {ocfBox && (
                    <rect
                      x={ocfBox.x}
                      y={ocfBox.y}
                      width={ocfBox.width}
                      height={ocfBox.height}
                      rx={3}
                      fill={
                        (p.operatingCashFlow ?? 0) >= 0 ? COLOR_OCF : COLOR_NEG
                      }
                      opacity={opacity}
                    />
                  )}
                  {fcfBox && (
                    <rect
                      x={fcfBox.x}
                      y={fcfBox.y}
                      width={fcfBox.width}
                      height={fcfBox.height}
                      rx={3}
                      fill={(p.freeCashFlow ?? 0) >= 0 ? COLOR_FCF : COLOR_NEG}
                      opacity={opacity}
                    />
                  )}
                </g>
              );
            }

            // EPS single bar with direct label (when room permits, or on active bar)
            if (p.dilutedEps == null) return null;
            const epsBox = barRect(
              cx - singleBarW / 2,
              singleBarW,
              p.dilutedEps,
            );
            const labelY =
              p.dilutedEps >= 0 ? epsBox.y - 5 : epsBox.y + epsBox.height + 11;
            const showLabel = showAllEpsLabels || i === activeIdx;
            return (
              <g key={`eps-${p.asOfDate}`}>
                <rect
                  x={epsBox.x}
                  y={epsBox.y}
                  width={epsBox.width}
                  height={epsBox.height}
                  rx={Math.min(4, singleBarW / 2)}
                  fill={p.dilutedEps >= 0 ? COLOR_EPS : COLOR_NEG}
                  opacity={opacity}
                />
                {showLabel && (
                  <text
                    x={cx}
                    y={Math.max(padT + 9, Math.min(H - padB - 2, labelY))}
                    textAnchor="middle"
                    fontSize="10"
                    fontWeight={i === activeIdx ? "700" : "500"}
                    fill="var(--text-secondary)"
                  >
                    {fmtEps(p.dilutedEps, currency)}
                  </text>
                )}
              </g>
            );
          })}

          {/* Margin lines overlay */}
          {grossLinePts.length >= 2 && (
            <polyline
              fill="none"
              stroke={COLOR_GROSS_MARGIN}
              strokeWidth={2}
              strokeLinecap="round"
              strokeLinejoin="round"
              points={grossLinePts
                .map((pt) => `${pt.x.toFixed(1)},${pt.y.toFixed(1)}`)
                .join(" ")}
            />
          )}
          {netLinePts.length >= 2 && (
            <polyline
              fill="none"
              stroke={COLOR_NET_MARGIN}
              strokeWidth={2}
              strokeLinecap="round"
              strokeLinejoin="round"
              points={netLinePts
                .map((pt) => `${pt.x.toFixed(1)},${pt.y.toFixed(1)}`)
                .join(" ")}
            />
          )}
          {grossLinePts.map((pt) => (
            <circle
              key={`gm-${pt.i}`}
              cx={pt.x}
              cy={pt.y}
              r={pt.i === activeIdx ? 4.5 : 3}
              fill={COLOR_GROSS_MARGIN}
              stroke="var(--surface)"
              strokeWidth={1.5}
            />
          ))}
          {netLinePts.map((pt) => (
            <circle
              key={`nm-${pt.i}`}
              cx={pt.x}
              cy={pt.y}
              r={pt.i === activeIdx ? 4.5 : 3}
              fill={COLOR_NET_MARGIN}
              stroke="var(--surface)"
              strokeWidth={1.5}
            />
          ))}

          {/* X-axis period labels (auto-thinned for 10-40 bars so text never overlaps) */}
          {periods.map((p, i) => {
            const isLast = i === periods.length - 1;
            const keep =
              i % xLabelStep === 0 ||
              isLast ||
              (hoverIdx != null && i === hoverIdx);
            if (!keep) return null;
            // Avoid crowding the penultimate stepped label against the last label
            if (
              !isLast &&
              i !== hoverIdx &&
              periods.length - 1 - i < Math.ceil(xLabelStep * 0.6)
            ) {
              return null;
            }
            return (
              <text
                key={`xl-${p.asOfDate}`}
                x={slotCenterX(i)}
                y={H - 8}
                textAnchor="middle"
                fontSize="10.5"
                fontWeight={i === activeIdx ? "700" : "400"}
                fill={i === activeIdx ? "var(--text)" : "var(--muted)"}
              >
                {fmtFinancialPeriod(p.asOfDate, effectiveMode)}
              </text>
            );
          })}

          {/* Vertical hover guide line */}
          {hoverIdx != null && (
            <line
              x1={slotCenterX(activeIdx)}
              y1={padT}
              x2={slotCenterX(activeIdx)}
              y2={padT + plotH}
              stroke="var(--text)"
              strokeWidth={1}
              strokeDasharray="3 2"
              opacity={0.35}
              pointerEvents="none"
            />
          )}
        </svg>

        {hoverIdx != null &&
          hover != null &&
          (() => {
            const tipPos = calcFloatingTooltipPos({
              anchorX: (slotCenterX(activeIdx) / W) * hover.wrapW,
              anchorY: hover.y,
              containerW: hover.wrapW,
              containerH: hover.wrapH,
              tipW: hover.tipW,
              tipH: hover.tipH,
            });
            const revGrowth =
              active.revenueYoYPct != null
                ? { pct: active.revenueYoYPct, tag: "同比" }
                : effectiveMode === "quarterly" && active.revenueQoQPct != null
                  ? { pct: active.revenueQoQPct, tag: "环比" }
                  : null;
            return (
              <div
                ref={tipRef}
                className="markets-chart-tip"
                style={{ left: tipPos.left, top: tipPos.top }}
              >
                <div className="markets-chart-tip-head">
                  <strong>
                    {fmtFinancialPeriod(active.asOfDate, effectiveMode)}
                  </strong>
                  <span className="markets-chart-tip-date">
                    {active.asOfDate}
                  </span>
                </div>

                {chartTab === "income" && (
                  <>
                    <div className="markets-chart-tip-row">
                      <span className="markets-chart-tip-label">
                        <i
                          className="markets-chart-tip-dot"
                          style={{ background: COLOR_REVENUE }}
                        />
                        营业收入
                      </span>
                      <span className="markets-chart-tip-val">
                        <strong>
                          {fmtFinancialValue(active.revenue, currency)}
                        </strong>
                        {revGrowth && (
                          <em
                            style={{
                              color: toneColor(
                                trendTone(revGrowth.pct, colorScheme),
                              ),
                            }}
                          >
                            {fmtSigned(revGrowth.pct, "%")}
                          </em>
                        )}
                      </span>
                    </div>
                    <div className="markets-chart-tip-row">
                      <span className="markets-chart-tip-label">
                        <i
                          className="markets-chart-tip-dot"
                          style={{
                            background:
                              (active.netIncome ?? 0) >= 0
                                ? COLOR_NET_POS
                                : COLOR_NEG,
                          }}
                        />
                        净利润
                      </span>
                      <span className="markets-chart-tip-val">
                        <strong>
                          {fmtFinancialValue(active.netIncome, currency)}
                        </strong>
                        {active.netIncomeYoYPct != null && (
                          <em
                            style={{
                              color: toneColor(
                                trendTone(active.netIncomeYoYPct, colorScheme),
                              ),
                            }}
                          >
                            {fmtSigned(active.netIncomeYoYPct, "%")}
                          </em>
                        )}
                      </span>
                    </div>
                    {(active.grossProfit != null ||
                      active.operatingIncome != null) && (
                      <div className="markets-chart-tip-grid">
                        {active.grossProfit != null && (
                          <div className="markets-chart-tip-cell">
                            <span>毛利</span>
                            <b>
                              {fmtFinancialValue(active.grossProfit, currency)}
                            </b>
                          </div>
                        )}
                        {active.operatingIncome != null && (
                          <div className="markets-chart-tip-cell">
                            <span>营利</span>
                            <b>
                              {fmtFinancialValue(
                                active.operatingIncome,
                                currency,
                              )}
                            </b>
                          </div>
                        )}
                      </div>
                    )}
                    {(active.grossMarginPct != null ||
                      active.netMarginPct != null ||
                      active.operatingMarginPct != null ||
                      active.dilutedEps != null) && (
                      <>
                        <div className="markets-chart-tip-sep" />
                        <div className="markets-chart-tip-grid">
                          {active.grossMarginPct != null && (
                            <div className="markets-chart-tip-cell">
                              <span className="markets-chart-tip-label">
                                <i
                                  className="markets-chart-tip-dot line"
                                  style={{ background: COLOR_GROSS_MARGIN }}
                                />
                                毛利率
                              </span>
                              <b>{fmtPct(active.grossMarginPct)}</b>
                            </div>
                          )}
                          {active.netMarginPct != null && (
                            <div className="markets-chart-tip-cell">
                              <span className="markets-chart-tip-label">
                                <i
                                  className="markets-chart-tip-dot line"
                                  style={{ background: COLOR_NET_MARGIN }}
                                />
                                净利率
                              </span>
                              <b>{fmtPct(active.netMarginPct)}</b>
                            </div>
                          )}
                          {active.operatingMarginPct != null && (
                            <div className="markets-chart-tip-cell">
                              <span>营利率</span>
                              <b>{fmtPct(active.operatingMarginPct)}</b>
                            </div>
                          )}
                          {active.dilutedEps != null && (
                            <div className="markets-chart-tip-cell">
                              <span>EPS</span>
                              <b>{fmtEps(active.dilutedEps, currency)}</b>
                            </div>
                          )}
                        </div>
                      </>
                    )}
                  </>
                )}

                {chartTab === "cashflow" && (
                  <>
                    <div className="markets-chart-tip-row">
                      <span className="markets-chart-tip-label">
                        <i
                          className="markets-chart-tip-dot"
                          style={{
                            background:
                              (active.operatingCashFlow ?? 0) >= 0
                                ? COLOR_OCF
                                : COLOR_NEG,
                          }}
                        />
                        经营现金流
                      </span>
                      <span className="markets-chart-tip-val">
                        <strong>
                          {fmtFinancialValue(
                            active.operatingCashFlow,
                            currency,
                          )}
                        </strong>
                      </span>
                    </div>
                    <div className="markets-chart-tip-row">
                      <span className="markets-chart-tip-label">
                        <i
                          className="markets-chart-tip-dot"
                          style={{
                            background:
                              (active.freeCashFlow ?? 0) >= 0
                                ? COLOR_FCF
                                : COLOR_NEG,
                          }}
                        />
                        自由现金流
                      </span>
                      <span className="markets-chart-tip-val">
                        <strong>
                          {fmtFinancialValue(active.freeCashFlow, currency)}
                        </strong>
                      </span>
                    </div>
                    {((active.revenue != null &&
                      active.revenue > 0 &&
                      active.freeCashFlow != null) ||
                      (active.netIncome != null &&
                        active.netIncome > 0 &&
                        active.operatingCashFlow != null)) && (
                      <>
                        <div className="markets-chart-tip-sep" />
                        <div className="markets-chart-tip-grid">
                          {active.revenue != null &&
                            active.revenue > 0 &&
                            active.freeCashFlow != null && (
                              <div className="markets-chart-tip-cell">
                                <span>FCF/营收</span>
                                <b>
                                  {fmtPct(
                                    (active.freeCashFlow / active.revenue) *
                                      100,
                                  )}
                                </b>
                              </div>
                            )}
                          {active.netIncome != null &&
                            active.netIncome > 0 &&
                            active.operatingCashFlow != null && (
                              <div className="markets-chart-tip-cell">
                                <span>净现比</span>
                                <b>
                                  {(
                                    active.operatingCashFlow / active.netIncome
                                  ).toFixed(2)}
                                  ×
                                </b>
                              </div>
                            )}
                        </div>
                      </>
                    )}
                  </>
                )}

                {chartTab === "eps" && (
                  <>
                    <div className="markets-chart-tip-row">
                      <span className="markets-chart-tip-label">
                        <i
                          className="markets-chart-tip-dot"
                          style={{
                            background:
                              (active.dilutedEps ?? 0) >= 0
                                ? COLOR_EPS
                                : COLOR_NEG,
                          }}
                        />
                        稀释 EPS
                      </span>
                      <span className="markets-chart-tip-val">
                        <strong>{fmtEps(active.dilutedEps, currency)}</strong>
                      </span>
                    </div>
                    {active.revenue != null && (
                      <div className="markets-chart-tip-row">
                        <span className="markets-chart-tip-label">
                          营业收入
                        </span>
                        <span className="markets-chart-tip-val">
                          {fmtFinancialValue(active.revenue, currency)}
                        </span>
                      </div>
                    )}
                    {active.netIncome != null && (
                      <div className="markets-chart-tip-row">
                        <span className="markets-chart-tip-label">净利润</span>
                        <span className="markets-chart-tip-val">
                          {fmtFinancialValue(active.netIncome, currency)}
                        </span>
                      </div>
                    )}
                    {active.netMarginPct != null && (
                      <div className="markets-chart-tip-row">
                        <span className="markets-chart-tip-label">净利率</span>
                        <span className="markets-chart-tip-val">
                          {fmtPct(active.netMarginPct)}
                        </span>
                      </div>
                    )}
                  </>
                )}
              </div>
            );
          })()}
      </div>

      <div className="markets-kline-hint">
        悬停各报告期柱状查看营收、利润率与现金流详情 · 数据源：
        {financials?.source ?? "Yahoo Finance"}（本地缓存 24 小时）
      </div>
    </div>
  );
}
