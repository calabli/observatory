import {
  calcFloatingTooltipPos,
  type ChartPoint,
  type ChartRange,
  fmtChartTime,
  fmtPrice,
  fmtSigned,
  fmtVolume,
  type LiveExtremes,
  toneColor,
  trendTone,
} from "./marketsShared";
import {
  alignBenchmarkReturns,
  calcKDJ,
  calcMACD,
  calcRangeStats,
  calcRSI,
  calcSMA,
} from "./chartIndicators";
import { useEffect, useMemo, useRef, useState } from "react";
import { Maximize2, RotateCcw } from "lucide-react";

export type ChartStyleMode = "candle" | "line";
export type SubIndicatorMode = "vol" | "macd" | "rsi" | "kdj" | "none";

export function MiniSparkline({
  points,
  prevClose,
  color,
  width = 62,
  height = 20,
}: {
  points: ChartPoint[];
  prevClose: number | null;
  color: string;
  width?: number;
  height?: number;
}) {
  if (points.length < 2) {
    return (
      <span
        className="markets-mini-empty"
        style={{ width, height }}
        title="无分时数据：超过 24 只的股票池用批量报价实时刷新，不含分时走势；点击该行查看走势图"
      >
        —
      </span>
    );
  }
  const prices = points.map((p) => p.price);
  const lo = Math.min(...prices, ...(prevClose ? [prevClose] : []));
  const hi = Math.max(...prices, ...(prevClose ? [prevClose] : []));
  const span = Math.max(hi - lo, hi * 0.002, 1e-6);
  const yOf = (v: number) => 2 + (1 - (v - lo) / span) * (height - 4);
  const pts = points
    .map(
      (p, i) =>
        `${(2 + (i / (points.length - 1)) * (width - 4)).toFixed(1)},${yOf(p.price).toFixed(1)}`,
    )
    .join(" ");

  return (
    <svg width={width} height={height} className="markets-mini-svg">
      {prevClose != null && (
        <line
          x1={2}
          y1={yOf(prevClose)}
          x2={width - 2}
          y2={yOf(prevClose)}
          stroke="var(--muted)"
          strokeWidth={0.8}
          strokeDasharray="2 2"
          opacity={0.45}
        />
      )}
      <polyline
        fill="none"
        stroke={color}
        strokeWidth={1.5}
        strokeLinecap="round"
        strokeLinejoin="round"
        points={pts}
      />
    </svg>
  );
}

const MA_CONFIGS = [
  { period: 5, label: "MA5", color: "#f59e0b" },
  { period: 10, label: "MA10", color: "#06b6d4" },
  { period: 20, label: "MA20", color: "#a855f7" },
  { period: 60, label: "MA60", color: "#3b82f6" },
] as const;

export function InteractivePriceChart({
  symbol,
  points,
  prevClose,
  currency,
  range,
  color,
  colorScheme = "cn",
  loading,
  error,
  extremes,
  fiftyTwoWeekHigh,
  fiftyTwoWeekLow,
  benchmarkName,
  benchmarkPoints,
  benchmarkLoading,
  showBenchmark = false,
  onToggleBenchmark,
  onExpand,
  expanded = false,
}: {
  symbol?: string;
  points: ChartPoint[];
  prevClose: number | null;
  currency: string;
  range: ChartRange;
  color: string;
  colorScheme?: "cn" | "us";
  loading: boolean;
  error: string | null;
  extremes?: LiveExtremes | null;
  fiftyTwoWeekHigh?: number | null;
  fiftyTwoWeekLow?: number | null;
  benchmarkName?: string;
  benchmarkPoints?: ChartPoint[];
  benchmarkLoading?: boolean;
  showBenchmark?: boolean;
  onToggleBenchmark?: () => void;
  onExpand?: () => void;
  expanded?: boolean;
}) {
  const hasOhlc = useMemo(
    () => points.some((p) => p.open != null && p.high != null && p.low != null),
    [points],
  );
  const [userStyle, setUserStyle] = useState<ChartStyleMode | null>(null);
  const [showMA, setShowMA] = useState<boolean>(true);
  const [showRefLines, setShowRefLines] = useState<boolean>(false);
  const [subMode, setSubMode] = useState<SubIndicatorMode>("vol");
  const [hover, setHover] = useState<{
    idx: number;
    y: number;
    wrapW: number;
    wrapH: number;
    tipW: number;
    tipH: number;
  } | null>(null);
  const [windowSlice, setWindowSlice] = useState<{
    start: number;
    end: number;
  } | null>(null);
  const wrapRef = useRef<HTMLDivElement>(null);
  const tipRef = useRef<HTMLDivElement>(null);
  const [measuredW, setMeasuredW] = useState<number>(expanded ? 1080 : 760);
  const dragRef = useRef<{
    dragging: boolean;
    startClientX: number;
    origStart: number;
    origEnd: number;
    width: number;
  } | null>(null);

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
  }, [expanded, points.length, loading]);

  // Reset zoom/pan and style override when switching symbol or range
  useEffect(() => {
    setWindowSlice(null);
    setHover(null);
    setUserStyle(null);
  }, [symbol, range]);

  const effectiveStyle: ChartStyleMode = showBenchmark
    ? "line"
    : (userStyle ?? (range === "1d" || !hasOhlc ? "line" : "candle"));

  const upColor =
    colorScheme === "cn" ? "var(--accent-red)" : "var(--accent-green)";
  const downColor =
    colorScheme === "cn" ? "var(--accent-green)" : "var(--accent-red)";

  // Compute indicators over full series so zoomed slices have warm MA/MACD/RSI/KDJ values
  const fullIndicators = useMemo(() => {
    const closes = points.map((p) => p.price);
    const mas = MA_CONFIGS.map((cfg) => ({
      ...cfg,
      values: calcSMA(closes, cfg.period),
    }));
    const macd = calcMACD(closes);
    const rsi6 = calcRSI(closes, 6);
    const rsi12 = calcRSI(closes, 12);
    const rsi24 = calcRSI(closes, 24);
    const kdj = calcKDJ(points);
    return { mas, macd, rsi6, rsi12, rsi24, kdj };
  }, [points]);

  const totalLen = points.length;
  const startIdx = windowSlice
    ? Math.max(0, Math.min(windowSlice.start, Math.max(0, totalLen - 2)))
    : 0;
  const endIdx = windowSlice
    ? Math.max(startIdx + 1, Math.min(windowSlice.end, totalLen - 1))
    : Math.max(0, totalLen - 1);
  const isZoomed =
    windowSlice != null && (startIdx > 0 || endIdx < totalLen - 1);

  const visiblePoints = useMemo(
    () => points.slice(startIdx, endIdx + 1),
    [points, startIdx, endIdx],
  );

  const rangeStats = useMemo(
    () =>
      calcRangeStats(
        visiblePoints,
        range === "1d" && !isZoomed ? prevClose : null,
      ),
    [visiblePoints, range, isZoomed, prevClose],
  );

  const alignedBench = useMemo(() => {
    if (!showBenchmark || !benchmarkPoints || benchmarkPoints.length < 2) {
      return null;
    }
    return alignBenchmarkReturns(visiblePoints, benchmarkPoints);
  }, [showBenchmark, benchmarkPoints, visiblePoints]);

  if (loading) {
    return <div className="markets-chart-empty">正在加载行情曲线…</div>;
  }
  if (points.length < 2) {
    return (
      <div className="markets-chart-empty">
        {error ? `曲线加载失败：${error}` : "当前区间暂无足够成交曲线数据"}
      </div>
    );
  }

  const visCount = visiblePoints.length;
  const W = Math.max(300, measuredW);
  const mainH = expanded ? 300 : W < 440 ? 196 : 210;
  const subGap = subMode === "none" ? 0 : 18;
  const subH = subMode === "none" ? 0 : expanded ? 116 : W < 440 ? 72 : 82;
  const padL = W < 440 ? 48 : 62;
  const padR = W < 440 ? 10 : 16;
  const padT = 14;
  const padB = 22;
  const H = padT + mainH + subGap + subH + padB;
  const plotW = W - padL - padR;

  const xOf = (localIdx: number) =>
    visCount <= 1
      ? padL + plotW / 2
      : padL + (localIdx / (visCount - 1)) * plotW;

  // Y scale for Main Chart (Price mode vs Relative Return % Benchmark mode)
  let minY: number;
  let maxY: number;
  if (showBenchmark && alignedBench) {
    const vals: number[] = [...alignedBench.stockPct];
    for (const b of alignedBench.benchPct) {
      if (b != null) vals.push(b);
    }
    const rawMin = Math.min(0, ...vals);
    const rawMax = Math.max(0, ...vals);
    const pad = Math.max((rawMax - rawMin) * 0.1, 0.5);
    minY = rawMin - pad;
    maxY = rawMax + pad;
  } else {
    const lows: number[] = [];
    const highs: number[] = [];
    for (let i = 0; i < visCount; i++) {
      const pt = visiblePoints[i];
      lows.push(pt.low ?? pt.price);
      highs.push(pt.high ?? pt.price);
      if (showMA && range !== "1d") {
        for (const ma of fullIndicators.mas) {
          const mv = ma.values[startIdx + i];
          if (mv != null) {
            lows.push(mv);
            highs.push(mv);
          }
        }
      }
    }
    if (range === "1d" && prevClose != null) {
      lows.push(prevClose);
      highs.push(prevClose);
    }
    const rawMin = Math.min(...lows);
    const rawMax = Math.max(...highs);
    const pad = Math.max((rawMax - rawMin) * 0.08, rawMax * 0.001, 1e-4);
    minY = rawMin - pad;
    maxY = rawMax + pad;
  }

  const yOf = (v: number) =>
    padT + (1 - (v - minY) / Math.max(1e-9, maxY - minY)) * mainH;
  const yTicks = [0, 0.33, 0.67, 1].map((r) => minY + (maxY - minY) * r);
  const xTickCount = W >= 900 ? 8 : W >= 620 ? 6 : 4;
  const xTickIndices = Array.from(
    new Set(
      Array.from({ length: xTickCount + 1 }, (_, i) =>
        Math.round(((visCount - 1) * i) / xTickCount),
      ),
    ),
  );

  // Reference lines that fit inside (or close to) visible price scale
  const refLines: Array<{ label: string; value: number; color: string }> = [];
  if (showRefLines && !showBenchmark) {
    const candidates: Array<{
      label: string;
      value: number | null | undefined;
      color: string;
    }> = [
      { label: "历史最高", value: extremes?.ath, color: "#ef4444" },
      { label: "52周高", value: fiftyTwoWeekHigh, color: "#f97316" },
      { label: "52周低", value: fiftyTwoWeekLow, color: "#06b6d4" },
      { label: "历史最低", value: extremes?.atl, color: "#10b981" },
    ];
    for (const c of candidates) {
      if (c.value != null && c.value >= minY && c.value <= maxY) {
        refLines.push({ label: c.label, value: c.value, color: c.color });
      }
    }
  }

  // Hover point details
  const clampedHover =
    hover != null ? Math.max(0, Math.min(visCount - 1, hover.idx)) : null;
  const activeLocalIdx = clampedHover ?? visCount - 1;
  const activeGlobalIdx = startIdx + activeLocalIdx;
  const activePt = visiblePoints[activeLocalIdx];
  const prevPt = activeGlobalIdx > 0 ? points[activeGlobalIdx - 1] : null;
  const barRef =
    range === "1d"
      ? (prevClose ?? points[0].price)
      : (prevPt?.price ?? activePt.open ?? activePt.price);
  const barDiff = activePt.price - barRef;
  const barPct = barRef > 0 ? (barDiff / barRef) * 100 : 0;

  // Sub-chart layout & scaling
  const subTop = padT + mainH + subGap;
  const subBottom = subTop + subH;

  let volMax = 1;
  if (subMode === "vol") {
    for (const pt of visiblePoints) {
      if (pt.volume != null && pt.volume > volMax) volMax = pt.volume;
    }
  }

  let macdAbsMax = 0.01;
  if (subMode === "macd") {
    for (let i = startIdx; i <= endIdx; i++) {
      macdAbsMax = Math.max(
        macdAbsMax,
        Math.abs(fullIndicators.macd.dif[i] ?? 0),
        Math.abs(fullIndicators.macd.dea[i] ?? 0),
        Math.abs(fullIndicators.macd.hist[i] ?? 0),
      );
    }
  }

  const subYMacd = (v: number) =>
    subTop + (0.5 - (v / (macdAbsMax * 1.15)) * 0.48) * subH;
  const subY0to100 = (v: number) =>
    subTop + (1 - Math.max(-10, Math.min(110, v)) / 100) * subH;

  const candleW = Math.max(
    1.8,
    Math.min(expanded ? 12 : 8, (plotW / Math.max(1, visCount)) * 0.68),
  );

  // Wheel Zoom handler (zoom around cursor X)
  const handleWheel = (e: React.WheelEvent<HTMLDivElement>) => {
    if (totalLen < 16) return;
    const rect = e.currentTarget.getBoundingClientRect();
    const relX = ((e.clientX - rect.left) / Math.max(1, rect.width)) * W;
    const anchorRatio = Math.max(0, Math.min(1, (relX - padL) / plotW));
    const curSpan = endIdx - startIdx + 1;
    const factor = e.deltaY > 0 ? 1.2 : 0.82;
    const nextSpan = Math.max(
      14,
      Math.min(totalLen, Math.round(curSpan * factor)),
    );
    if (nextSpan >= totalLen) {
      setWindowSlice(null);
      return;
    }
    const anchorIdx = startIdx + Math.round(anchorRatio * (curSpan - 1));
    let nextStart = anchorIdx - Math.round(anchorRatio * (nextSpan - 1));
    nextStart = Math.max(0, Math.min(totalLen - nextSpan, nextStart));
    setWindowSlice({ start: nextStart, end: nextStart + nextSpan - 1 });
  };

  const updateHoverFromPointer = (
    e: React.PointerEvent<HTMLDivElement>,
    rect: DOMRect,
  ) => {
    const relX = ((e.clientX - rect.left) / Math.max(1, rect.width)) * W;
    if (relX < padL - 8 || relX > W - padR + 8) {
      setHover(null);
      return;
    }
    const ratio = Math.max(0, Math.min(1, (relX - padL) / plotW));
    const idx = Math.round(ratio * (visCount - 1));
    const y = e.clientY - rect.top;
    const tipW = tipRef.current?.offsetWidth || 216;
    const tipH = tipRef.current?.offsetHeight || 148;
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

  // Drag Pan / Touch Scrub handlers
  const handlePointerDown = (e: React.PointerEvent<HTMLDivElement>) => {
    if (e.pointerType === "touch" && !isZoomed) {
      updateHoverFromPointer(e, e.currentTarget.getBoundingClientRect());
      return;
    }
    if (e.button !== 0 || !isZoomed) return;
    e.currentTarget.setPointerCapture(e.pointerId);
    setHover(null);
    dragRef.current = {
      dragging: true,
      startClientX: e.clientX,
      origStart: startIdx,
      origEnd: endIdx,
      width: Math.max(1, e.currentTarget.getBoundingClientRect().width),
    };
  };

  const handlePointerMove = (e: React.PointerEvent<HTMLDivElement>) => {
    const rect = e.currentTarget.getBoundingClientRect();
    if (dragRef.current?.dragging) {
      const dxPx = e.clientX - dragRef.current.startClientX;
      const span = dragRef.current.origEnd - dragRef.current.origStart + 1;
      const shift = -Math.round((dxPx / dragRef.current.width) * span);
      let nextStart = dragRef.current.origStart + shift;
      nextStart = Math.max(0, Math.min(totalLen - span, nextStart));
      setWindowSlice({ start: nextStart, end: nextStart + span - 1 });
      return;
    }
    updateHoverFromPointer(e, rect);
  };

  const handlePointerUp = () => {
    if (dragRef.current) dragRef.current.dragging = false;
  };

  const buildPolyline = (
    vals: Array<number | null>,
    yMap: (v: number) => number,
  ) => {
    const segments: string[] = [];
    let current: string[] = [];
    for (let i = 0; i < vals.length; i++) {
      const v = vals[i];
      if (v != null && Number.isFinite(v)) {
        current.push(`${xOf(i).toFixed(1)},${yMap(v).toFixed(1)}`);
      } else if (current.length > 0) {
        segments.push(current.join(" "));
        current = [];
      }
    }
    if (current.length > 0) segments.push(current.join(" "));
    return segments;
  };

  const linePts = visiblePoints
    .map((p, i) => `${xOf(i).toFixed(1)},${yOf(p.price).toFixed(1)}`)
    .join(" ");
  const areaPts = `${padL},${padT + mainH} ${linePts} ${padL + plotW},${padT + mainH}`;

  return (
    <div className={`markets-kline-shell${expanded ? " expanded" : ""}`}>
      {/* Chart Control Toolbar */}
      <div className="markets-kline-toolbar">
        <div className="markets-kline-toolbar-left">
          {!showBenchmark && (
            <div className="markets-pill-group">
              <button
                type="button"
                className={`markets-pill-btn${effectiveStyle === "candle" ? " active" : ""}`}
                disabled={!hasOhlc}
                onClick={() => setUserStyle("candle")}
                title={hasOhlc ? "蜡烛 K 线" : "分时模式无 OHLC 蜡烛数据"}
              >
                K线
              </button>
              <button
                type="button"
                className={`markets-pill-btn${effectiveStyle === "line" ? " active" : ""}`}
                onClick={() => setUserStyle("line")}
              >
                折线
              </button>
            </div>
          )}

          {!showBenchmark && range !== "1d" && (
            <button
              type="button"
              className={`markets-mini-toggle${showMA ? " active" : ""}`}
              onClick={() => setShowMA((v) => !v)}
              title="显示 MA5 / MA10 / MA20 / MA60 均线"
            >
              均线 MA
            </button>
          )}

          {!showBenchmark && (
            <button
              type="button"
              className={`markets-mini-toggle${showRefLines ? " active" : ""}`}
              onClick={() => setShowRefLines((v) => !v)}
              title="在主图叠加历史高低 / 52周高低参考线"
            >
              高低线
            </button>
          )}

          {onToggleBenchmark && benchmarkName && (
            <button
              type="button"
              className={`markets-mini-toggle${showBenchmark ? " active" : ""}`}
              onClick={onToggleBenchmark}
              title={`与 ${benchmarkName} 归一化对比超额收益 (Alpha)`}
            >
              {benchmarkLoading ? "加载大盘…" : `叠 ${benchmarkName}`}
            </button>
          )}
        </div>

        <div className="markets-kline-toolbar-right">
          <div className="markets-pill-group">
            {(
              [
                ["vol", "量能"],
                ["macd", "MACD"],
                ["rsi", "RSI"],
                ["kdj", "KDJ"],
                ["none", "无副图"],
              ] as const
            ).map(([m, label]) => (
              <button
                key={m}
                type="button"
                className={`markets-pill-btn${subMode === m ? " active" : ""}`}
                onClick={() => setSubMode(m)}
              >
                {label}
              </button>
            ))}
          </div>

          {isZoomed && (
            <button
              type="button"
              className="markets-mini-toggle"
              onClick={() => setWindowSlice(null)}
              title="重置缩放与拖拽"
            >
              <RotateCcw size={11} />
              <span>重置</span>
            </button>
          )}

          {onExpand && !expanded && (
            <button
              type="button"
              className="markets-mini-toggle"
              onClick={onExpand}
              title="放大看图（支持滚轮缩放与拖拽）"
            >
              <Maximize2 size={11} />
              <span>放大</span>
            </button>
          )}
        </div>
      </div>

      {/* Range Statistics Strip */}
      {rangeStats && (
        <div className="markets-range-strip">
          <span>
            区间涨跌{" "}
            <b
              style={{
                color: toneColor(trendTone(rangeStats.changePct, colorScheme)),
              }}
            >
              {fmtSigned(rangeStats.changePct, "%")}
            </b>
          </span>
          <span>
            高 <b>{fmtPrice(rangeStats.high, currency)}</b>
          </span>
          <span>
            低 <b>{fmtPrice(rangeStats.low, currency)}</b>
          </span>
          <span>
            最大回撤{" "}
            <b
              style={{
                color: toneColor(
                  trendTone(rangeStats.maxDrawdownPct, colorScheme),
                ),
              }}
            >
              {fmtSigned(rangeStats.maxDrawdownPct, "%")}
            </b>
          </span>
          <span>
            振幅 <b>{rangeStats.amplitudePct.toFixed(1)}%</b>
          </span>
          {rangeStats.avgVolume != null && (
            <span>
              均量 <b>{fmtVolume(rangeStats.avgVolume)}</b>
            </span>
          )}
          {showBenchmark && alignedBench && benchmarkName && (
            <>
              <span>
                {benchmarkName}{" "}
                <b>{fmtSigned(alignedBench.benchTotalPct, "%")}</b>
              </span>
              <span>
                超额 Alpha{" "}
                <b
                  style={{
                    color: toneColor(
                      trendTone(alignedBench.alphaPct, colorScheme),
                    ),
                  }}
                >
                  {fmtSigned(alignedBench.alphaPct, "%")}
                </b>
              </span>
            </>
          )}
        </div>
      )}

      {/* Live Crosshair OHLCV + MA HUD */}
      <div className="markets-kline-hud">
        <span className="hud-time">{fmtChartTime(activePt.ts, range)}</span>
        {showBenchmark && alignedBench ? (
          <>
            <span>
              标的{" "}
              <b style={{ color }}>
                {fmtSigned(alignedBench.stockPct[activeLocalIdx], "%")}
              </b>
            </span>
            <span>
              {benchmarkName}{" "}
              <b style={{ color: "#f59e0b" }}>
                {fmtSigned(alignedBench.benchPct[activeLocalIdx], "%")}
              </b>
            </span>
          </>
        ) : (
          <>
            {activePt.open != null && (
              <span>
                开 <b>{fmtPrice(activePt.open, currency)}</b>
              </span>
            )}
            {activePt.high != null && (
              <span>
                高 <b>{fmtPrice(activePt.high, currency)}</b>
              </span>
            )}
            {activePt.low != null && (
              <span>
                低 <b>{fmtPrice(activePt.low, currency)}</b>
              </span>
            )}
            <span>
              收 <b>{fmtPrice(activePt.price, currency)}</b>
            </span>
            <span
              style={{
                color: toneColor(trendTone(barDiff, colorScheme)),
              }}
            >
              {fmtSigned(barPct, "%")}
            </span>
            {activePt.volume != null && activePt.volume > 0 && (
              <span>
                量 <b>{fmtVolume(activePt.volume)}</b>
              </span>
            )}
            {showMA &&
              range !== "1d" &&
              fullIndicators.mas.map((ma) => {
                const v = ma.values[activeGlobalIdx];
                if (v == null) return null;
                return (
                  <span key={ma.label} style={{ color: ma.color }}>
                    {ma.label} {v >= 100 ? v.toFixed(1) : v.toFixed(2)}
                  </span>
                );
              })}
          </>
        )}
      </div>

      {/* Main SVG Plot + Sub-indicator SVG Plot */}
      <div
        ref={wrapRef}
        className={`markets-chart-wrap${isZoomed ? " pannable" : ""}`}
        onWheel={handleWheel}
        onPointerDown={handlePointerDown}
        onPointerMove={handlePointerMove}
        onPointerUp={handlePointerUp}
        onPointerCancel={handlePointerUp}
        onPointerLeave={() => {
          setHover(null);
          handlePointerUp();
        }}
      >
        <svg
          className="markets-chart-svg"
          width={W}
          height={H}
          viewBox={`0 0 ${W} ${H}`}
          role="img"
          aria-label="标的 K 线与技术指标图"
        >
          {/* Main Y Gridlines */}
          {yTicks.map((val, i) => {
            const y = yOf(val);
            return (
              <g key={`my-${i}`}>
                <line
                  x1={padL}
                  y1={y}
                  x2={W - padR}
                  y2={y}
                  stroke="var(--line)"
                  strokeDasharray="3 3"
                  opacity={0.6}
                />
                <text
                  x={padL - 7}
                  y={y + 3.5}
                  textAnchor="end"
                  fill="var(--muted)"
                  fontSize={10}
                >
                  {showBenchmark && alignedBench
                    ? `${val >= 0 ? "+" : ""}${val.toFixed(1)}%`
                    : val.toFixed(val >= 100 ? 1 : 2)}
                </text>
              </g>
            );
          })}

          {/* Previous close line (1d) or 0% line (benchmark) */}
          {showBenchmark && alignedBench ? (
            <line
              x1={padL}
              y1={yOf(0)}
              x2={W - padR}
              y2={yOf(0)}
              stroke="var(--muted)"
              strokeWidth={1}
              strokeDasharray="4 3"
              opacity={0.7}
            />
          ) : (
            range === "1d" &&
            prevClose != null && (
              <line
                x1={padL}
                y1={yOf(prevClose)}
                x2={W - padR}
                y2={yOf(prevClose)}
                stroke="var(--muted)"
                strokeWidth={1}
                strokeDasharray="4 3"
                opacity={0.7}
              />
            )
          )}

          {/* Optional ATH / ATL / 52W Reference Lines */}
          {refLines.map((rl) => {
            const y = yOf(rl.value);
            return (
              <g key={rl.label}>
                <line
                  x1={padL}
                  y1={y}
                  x2={W - padR}
                  y2={y}
                  stroke={rl.color}
                  strokeWidth={1}
                  strokeDasharray="5 3"
                  opacity={0.7}
                />
                <text
                  x={W - padR - 4}
                  y={y - 3}
                  textAnchor="end"
                  fill={rl.color}
                  fontSize={9.5}
                  fontWeight={600}
                >
                  {rl.label} {fmtPrice(rl.value, currency)}
                </text>
              </g>
            );
          })}

          {/* Main Series: Benchmark Comparison OR Candlestick OR Line */}
          {showBenchmark && alignedBench ? (
            <>
              {buildPolyline(alignedBench.benchPct, yOf).map((pts, i) => (
                <polyline
                  key={`b-${i}`}
                  fill="none"
                  stroke="#f59e0b"
                  strokeWidth={1.6}
                  strokeDasharray="4 3"
                  points={pts}
                />
              ))}
              {buildPolyline(alignedBench.stockPct, yOf).map((pts, i) => (
                <polyline
                  key={`s-${i}`}
                  fill="none"
                  stroke={color}
                  strokeWidth={2}
                  strokeLinejoin="round"
                  points={pts}
                />
              ))}
            </>
          ) : effectiveStyle === "candle" ? (
            <>
              {visiblePoints.map((pt, i) => {
                const o = pt.open ?? pt.price;
                const c = pt.price;
                const h = pt.high ?? Math.max(o, c);
                const l = pt.low ?? Math.min(o, c);
                const isUp = c >= o;
                const cColor = isUp ? upColor : downColor;
                const cx = xOf(i);
                const bodyTop = yOf(Math.max(o, c));
                const bodyBot = yOf(Math.min(o, c));
                const bodyH = Math.max(1.2, bodyBot - bodyTop);
                return (
                  <g key={pt.ts}>
                    <line
                      x1={cx}
                      y1={yOf(h)}
                      x2={cx}
                      y2={yOf(l)}
                      stroke={cColor}
                      strokeWidth={1}
                    />
                    <rect
                      x={cx - candleW / 2}
                      y={bodyTop}
                      width={candleW}
                      height={bodyH}
                      fill={cColor}
                      rx={0.8}
                    />
                  </g>
                );
              })}
            </>
          ) : (
            <>
              <polygon points={areaPts} fill={color} opacity={0.14} />
              <polyline
                fill="none"
                stroke={color}
                strokeWidth={2}
                strokeLinecap="round"
                strokeLinejoin="round"
                points={linePts}
              />
            </>
          )}

          {/* Moving Average Overlays (MA5 / MA10 / MA20 / MA60) */}
          {showMA &&
            !showBenchmark &&
            range !== "1d" &&
            fullIndicators.mas.map((ma) => {
              const sliceVals = ma.values.slice(startIdx, endIdx + 1);
              return buildPolyline(sliceVals, yOf).map((seg, idx) => (
                <polyline
                  key={`${ma.label}-${idx}`}
                  fill="none"
                  stroke={ma.color}
                  strokeWidth={1.15}
                  opacity={0.9}
                  points={seg}
                />
              ));
            })}

          {/* Sub-chart Panel (Volume / MACD / RSI / KDJ) */}
          {subMode !== "none" && (
            <g>
              <line
                x1={padL}
                y1={subTop}
                x2={W - padR}
                y2={subTop}
                stroke="var(--line)"
                opacity={0.75}
              />
              {/* Sub-chart label + active value */}
              <text
                x={padL + 4}
                y={subTop + 11}
                fill="var(--muted)"
                fontSize={9.5}
              >
                {subMode === "vol"
                  ? `VOL ${fmtVolume(activePt.volume)}`
                  : subMode === "macd"
                    ? `MACD(12,26,9)  DIF ${(fullIndicators.macd.dif[activeGlobalIdx] ?? 0).toFixed(2)}  DEA ${(fullIndicators.macd.dea[activeGlobalIdx] ?? 0).toFixed(2)}  柱 ${(fullIndicators.macd.hist[activeGlobalIdx] ?? 0).toFixed(2)}`
                    : subMode === "rsi"
                      ? `RSI  6:${fullIndicators.rsi6[activeGlobalIdx]?.toFixed(1) ?? "—"}  12:${fullIndicators.rsi12[activeGlobalIdx]?.toFixed(1) ?? "—"}  24:${fullIndicators.rsi24[activeGlobalIdx]?.toFixed(1) ?? "—"}`
                      : `KDJ(9,3,3)  K:${(fullIndicators.kdj.k[activeGlobalIdx] ?? 50).toFixed(1)}  D:${(fullIndicators.kdj.d[activeGlobalIdx] ?? 50).toFixed(1)}  J:${(fullIndicators.kdj.j[activeGlobalIdx] ?? 50).toFixed(1)}`}
              </text>

              {subMode === "vol" &&
                visiblePoints.map((pt, i) => {
                  const v = pt.volume ?? 0;
                  if (v <= 0) return null;
                  const o =
                    pt.open ?? (i > 0 ? visiblePoints[i - 1].price : pt.price);
                  const barColor = pt.price >= o ? upColor : downColor;
                  const bh = Math.max(1, (v / volMax) * (subH - 14));
                  return (
                    <rect
                      key={`v-${pt.ts}`}
                      x={xOf(i) - candleW / 2}
                      y={subBottom - bh}
                      width={candleW}
                      height={bh}
                      fill={barColor}
                      opacity={0.62}
                    />
                  );
                })}

              {subMode === "macd" && (
                <>
                  <line
                    x1={padL}
                    y1={subYMacd(0)}
                    x2={W - padR}
                    y2={subYMacd(0)}
                    stroke="var(--line)"
                    strokeDasharray="2 2"
                  />
                  {visiblePoints.map((pt, i) => {
                    const h = fullIndicators.macd.hist[startIdx + i] ?? 0;
                    const y0 = subYMacd(0);
                    const y1 = subYMacd(h);
                    return (
                      <rect
                        key={`mh-${pt.ts}`}
                        x={xOf(i) - Math.max(1, candleW * 0.4)}
                        y={Math.min(y0, y1)}
                        width={Math.max(1.5, candleW * 0.8)}
                        height={Math.max(1, Math.abs(y1 - y0))}
                        fill={h >= 0 ? upColor : downColor}
                        opacity={0.65}
                      />
                    );
                  })}
                  {buildPolyline(
                    fullIndicators.macd.dif.slice(startIdx, endIdx + 1),
                    subYMacd,
                  ).map((seg, idx) => (
                    <polyline
                      key={`dif-${idx}`}
                      fill="none"
                      stroke="#f59e0b"
                      strokeWidth={1.2}
                      points={seg}
                    />
                  ))}
                  {buildPolyline(
                    fullIndicators.macd.dea.slice(startIdx, endIdx + 1),
                    subYMacd,
                  ).map((seg, idx) => (
                    <polyline
                      key={`dea-${idx}`}
                      fill="none"
                      stroke="#3b82f6"
                      strokeWidth={1.2}
                      points={seg}
                    />
                  ))}
                </>
              )}

              {subMode === "rsi" && (
                <>
                  {[70, 30].map((lvl) => (
                    <g key={`rl-${lvl}`}>
                      <line
                        x1={padL}
                        y1={subY0to100(lvl)}
                        x2={W - padR}
                        y2={subY0to100(lvl)}
                        stroke="var(--line)"
                        strokeDasharray="3 3"
                      />
                      <text
                        x={padL - 6}
                        y={subY0to100(lvl) + 3}
                        textAnchor="end"
                        fill="var(--muted)"
                        fontSize={9}
                      >
                        {lvl}
                      </text>
                    </g>
                  ))}
                  {[
                    { arr: fullIndicators.rsi6, color: "#f59e0b" },
                    { arr: fullIndicators.rsi12, color: "#06b6d4" },
                    { arr: fullIndicators.rsi24, color: "#a855f7" },
                  ].map((s, si) =>
                    buildPolyline(
                      s.arr.slice(startIdx, endIdx + 1),
                      subY0to100,
                    ).map((seg, idx) => (
                      <polyline
                        key={`rsi-${si}-${idx}`}
                        fill="none"
                        stroke={s.color}
                        strokeWidth={1.15}
                        points={seg}
                      />
                    )),
                  )}
                </>
              )}

              {subMode === "kdj" && (
                <>
                  {[80, 20].map((lvl) => (
                    <g key={`kl-${lvl}`}>
                      <line
                        x1={padL}
                        y1={subY0to100(lvl)}
                        x2={W - padR}
                        y2={subY0to100(lvl)}
                        stroke="var(--line)"
                        strokeDasharray="3 3"
                      />
                      <text
                        x={padL - 6}
                        y={subY0to100(lvl) + 3}
                        textAnchor="end"
                        fill="var(--muted)"
                        fontSize={9}
                      >
                        {lvl}
                      </text>
                    </g>
                  ))}
                  {[
                    { arr: fullIndicators.kdj.k, color: "#f59e0b" },
                    { arr: fullIndicators.kdj.d, color: "#06b6d4" },
                    { arr: fullIndicators.kdj.j, color: "#a855f7" },
                  ].map((s, si) =>
                    buildPolyline(
                      s.arr.slice(startIdx, endIdx + 1),
                      subY0to100,
                    ).map((seg, idx) => (
                      <polyline
                        key={`kdj-${si}-${idx}`}
                        fill="none"
                        stroke={s.color}
                        strokeWidth={1.15}
                        points={seg}
                      />
                    )),
                  )}
                </>
              )}
            </g>
          )}

          {/* X-axis Ticks */}
          {xTickIndices.map((idx) => (
            <text
              key={`xt-${idx}`}
              x={xOf(idx)}
              y={H - 5}
              textAnchor={
                idx === 0 ? "start" : idx === visCount - 1 ? "end" : "middle"
              }
              fill="var(--muted)"
              fontSize={10}
            >
              {fmtChartTime(visiblePoints[idx].ts, range)}
            </text>
          ))}

          {/* Full Crosshair (spans both Main + Sub-chart) */}
          {clampedHover != null && (
            <g pointerEvents="none">
              <line
                x1={xOf(clampedHover)}
                y1={padT}
                x2={xOf(clampedHover)}
                y2={subMode === "none" ? padT + mainH : subBottom}
                stroke="var(--text)"
                strokeWidth={1}
                strokeDasharray="3 2"
                opacity={0.45}
              />
              <line
                x1={padL}
                y1={yOf(
                  showBenchmark && alignedBench
                    ? alignedBench.stockPct[clampedHover]
                    : visiblePoints[clampedHover].price,
                )}
                x2={W - padR}
                y2={yOf(
                  showBenchmark && alignedBench
                    ? alignedBench.stockPct[clampedHover]
                    : visiblePoints[clampedHover].price,
                )}
                stroke="var(--text)"
                strokeWidth={0.8}
                strokeDasharray="3 3"
                opacity={0.3}
              />
              <circle
                cx={xOf(clampedHover)}
                cy={yOf(
                  showBenchmark && alignedBench
                    ? alignedBench.stockPct[clampedHover]
                    : visiblePoints[clampedHover].price,
                )}
                r={3.8}
                fill={color}
                stroke="var(--surface)"
                strokeWidth={1.5}
              />
              {showBenchmark &&
                alignedBench &&
                alignedBench.benchPct[clampedHover] != null && (
                  <circle
                    cx={xOf(clampedHover)}
                    cy={yOf(alignedBench.benchPct[clampedHover]!)}
                    r={3.4}
                    fill="#f59e0b"
                    stroke="var(--surface)"
                    strokeWidth={1.5}
                  />
                )}
            </g>
          )}
        </svg>

        {clampedHover != null &&
          hover != null &&
          (() => {
            const tipPos = calcFloatingTooltipPos({
              anchorX: (xOf(clampedHover) / W) * hover.wrapW,
              anchorY: hover.y,
              containerW: hover.wrapW,
              containerH: hover.wrapH,
              tipW: hover.tipW,
              tipH: hover.tipH,
            });
            const benchPctVal =
              showBenchmark && alignedBench
                ? alignedBench.benchPct[activeLocalIdx]
                : null;
            const stockPctVal =
              showBenchmark && alignedBench
                ? alignedBench.stockPct[activeLocalIdx]
                : null;
            const maItems =
              !showBenchmark && showMA && range !== "1d"
                ? fullIndicators.mas.flatMap((ma) => {
                    const value = ma.values[activeGlobalIdx];
                    return value != null
                      ? [{ label: ma.label, color: ma.color, value }]
                      : [];
                  })
                : [];
            return (
              <div
                ref={tipRef}
                className="markets-chart-tip"
                style={{ left: tipPos.left, top: tipPos.top }}
              >
                <div className="markets-chart-tip-head">
                  <span className="markets-chart-tip-time">
                    {fmtChartTime(activePt.ts, range)}
                  </span>
                  {!showBenchmark && (
                    <span
                      className="markets-chart-tip-badge"
                      style={{
                        color: toneColor(trendTone(barDiff, colorScheme)),
                      }}
                    >
                      {fmtSigned(barPct, "%")}
                    </span>
                  )}
                </div>

                {showBenchmark && alignedBench && stockPctVal != null ? (
                  <>
                    <div className="markets-chart-tip-row">
                      <span className="markets-chart-tip-label">
                        <i
                          className="markets-chart-tip-dot"
                          style={{ background: color }}
                        />
                        {symbol || "标的"}
                      </span>
                      <span className="markets-chart-tip-val">
                        <strong>{fmtSigned(stockPctVal, "%")}</strong>
                      </span>
                    </div>
                    <div className="markets-chart-tip-row">
                      <span className="markets-chart-tip-label">价格</span>
                      <span className="markets-chart-tip-val">
                        {fmtPrice(activePt.price, currency)}
                      </span>
                    </div>
                    {benchPctVal != null && (
                      <>
                        <div className="markets-chart-tip-row">
                          <span className="markets-chart-tip-label">
                            <i
                              className="markets-chart-tip-dot line"
                              style={{ background: "#f59e0b" }}
                            />
                            {benchmarkName || "基准"}
                          </span>
                          <span className="markets-chart-tip-val">
                            {fmtSigned(benchPctVal, "%")}
                          </span>
                        </div>
                        <div className="markets-chart-tip-sep" />
                        <div className="markets-chart-tip-row">
                          <span className="markets-chart-tip-label">
                            相对超额
                          </span>
                          <span
                            className="markets-chart-tip-val"
                            style={{
                              color: toneColor(
                                trendTone(
                                  stockPctVal - benchPctVal,
                                  colorScheme,
                                ),
                              ),
                            }}
                          >
                            {fmtSigned(stockPctVal - benchPctVal, "%")}
                          </span>
                        </div>
                      </>
                    )}
                  </>
                ) : (
                  <>
                    <div className="markets-chart-tip-row">
                      <span className="markets-chart-tip-label">
                        {activePt.open != null ? "收盘" : "价格"}
                      </span>
                      <span className="markets-chart-tip-val">
                        <strong>{fmtPrice(activePt.price, currency)}</strong>
                      </span>
                    </div>
                    <div className="markets-chart-tip-row">
                      <span className="markets-chart-tip-label">
                        {range === "1d" ? "较昨收" : "涨跌额"}
                      </span>
                      <span
                        className="markets-chart-tip-val"
                        style={{
                          color: toneColor(trendTone(barDiff, colorScheme)),
                        }}
                      >
                        {fmtSigned(barDiff)}
                      </span>
                    </div>
                    {(activePt.open != null ||
                      activePt.high != null ||
                      activePt.low != null) && (
                      <div className="markets-chart-tip-grid">
                        {activePt.open != null && (
                          <div className="markets-chart-tip-cell">
                            <span>开</span>
                            <b>{fmtPrice(activePt.open, currency)}</b>
                          </div>
                        )}
                        {activePt.high != null && (
                          <div className="markets-chart-tip-cell">
                            <span>高</span>
                            <b>{fmtPrice(activePt.high, currency)}</b>
                          </div>
                        )}
                        {activePt.low != null && (
                          <div className="markets-chart-tip-cell">
                            <span>低</span>
                            <b>{fmtPrice(activePt.low, currency)}</b>
                          </div>
                        )}
                        {activePt.volume != null && activePt.volume > 0 && (
                          <div className="markets-chart-tip-cell">
                            <span>量</span>
                            <b>{fmtVolume(activePt.volume)}</b>
                          </div>
                        )}
                      </div>
                    )}
                    {activePt.open == null &&
                      activePt.high == null &&
                      activePt.low == null &&
                      activePt.volume != null &&
                      activePt.volume > 0 && (
                        <div className="markets-chart-tip-row">
                          <span className="markets-chart-tip-label">
                            成交量
                          </span>
                          <span className="markets-chart-tip-val">
                            {fmtVolume(activePt.volume)}
                          </span>
                        </div>
                      )}
                    {maItems.length > 0 && (
                      <>
                        <div className="markets-chart-tip-sep" />
                        <div className="markets-chart-tip-grid">
                          {maItems.map((m) => (
                            <div
                              key={m.label}
                              className="markets-chart-tip-cell"
                            >
                              <span style={{ color: m.color }}>{m.label}</span>
                              <b>
                                {m.value >= 100
                                  ? m.value.toFixed(1)
                                  : m.value.toFixed(2)}
                              </b>
                            </div>
                          ))}
                        </div>
                      </>
                    )}
                    {subMode !== "none" && subMode !== "vol" && (
                      <>
                        <div className="markets-chart-tip-sep" />
                        <div className="markets-chart-tip-sub">
                          {subMode === "macd" ? (
                            <>
                              <span>
                                DIF{" "}
                                {(
                                  fullIndicators.macd.dif[activeGlobalIdx] ?? 0
                                ).toFixed(2)}
                              </span>
                              <span>
                                DEA{" "}
                                {(
                                  fullIndicators.macd.dea[activeGlobalIdx] ?? 0
                                ).toFixed(2)}
                              </span>
                              <span>
                                柱{" "}
                                {(
                                  fullIndicators.macd.hist[activeGlobalIdx] ?? 0
                                ).toFixed(2)}
                              </span>
                            </>
                          ) : subMode === "rsi" ? (
                            <>
                              <span>
                                RSI6{" "}
                                {fullIndicators.rsi6[activeGlobalIdx]?.toFixed(
                                  1,
                                ) ?? "—"}
                              </span>
                              <span>
                                12{" "}
                                {fullIndicators.rsi12[activeGlobalIdx]?.toFixed(
                                  1,
                                ) ?? "—"}
                              </span>
                              <span>
                                24{" "}
                                {fullIndicators.rsi24[activeGlobalIdx]?.toFixed(
                                  1,
                                ) ?? "—"}
                              </span>
                            </>
                          ) : (
                            <>
                              <span>
                                K{" "}
                                {(
                                  fullIndicators.kdj.k[activeGlobalIdx] ?? 50
                                ).toFixed(1)}
                              </span>
                              <span>
                                D{" "}
                                {(
                                  fullIndicators.kdj.d[activeGlobalIdx] ?? 50
                                ).toFixed(1)}
                              </span>
                              <span>
                                J{" "}
                                {(
                                  fullIndicators.kdj.j[activeGlobalIdx] ?? 50
                                ).toFixed(1)}
                              </span>
                            </>
                          )}
                        </div>
                      </>
                    )}
                  </>
                )}
              </div>
            );
          })()}
      </div>

      {totalLen >= 16 && (
        <div className="markets-kline-hint">
          滚轮缩放周期密度{isZoomed ? " · 按住左键左右拖拽查看历史" : ""}
        </div>
      )}
    </div>
  );
}
