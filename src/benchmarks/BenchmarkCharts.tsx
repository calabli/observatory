import React, { useMemo, useState } from "react";
import {
  blendedPrice1m,
  computeParetoFrontier,
  creatorColor,
  effectiveTps,
  effectiveTtftS,
  formatPrice1m,
  formatScore,
  formatSeconds,
  formatTps,
  getDimensionScore,
  parseModelVariant,
  shortModelLabel,
  type BenchmarkDimensionKey,
  type BenchmarkModel,
  type LocalModelMatch,
} from "./benchmarksShared";

export type BenchmarkChartMode = "bar" | "price" | "speed";
export type BarLimitOption = 20 | 35 | 0;

interface BenchmarkChartProps {
  models: BenchmarkModel[];
  dimension: BenchmarkDimensionKey;
  dimensionLabel: string;
  chartMode: BenchmarkChartMode;
  onChangeChartMode: (mode: BenchmarkChartMode) => void;
  selectedId: string | null;
  onSelectModel: (id: string) => void;
  localMatches: Map<string, LocalModelMatch>;
}

interface PlotItem {
  id: string;
  model: BenchmarkModel;
  x: number;
  y: number;
  cx: number;
  cy: number;
  color: string;
  isLocal: boolean;
  isPareto: boolean;
}

const SCATTER_W = 860;
const SCATTER_H = 310;
const PAD_L = 52;
const PAD_R = 24;
const PAD_T = 22;
const PAD_B = 40;
const PLOT_W = SCATTER_W - PAD_L - PAD_R;
const PLOT_H = SCATTER_H - PAD_T - PAD_B;

export function BenchmarkChartsPanel({
  models,
  dimension,
  dimensionLabel,
  chartMode,
  onChangeChartMode,
  selectedId,
  onSelectModel,
  localMatches,
}: BenchmarkChartProps) {
  const [chartLimit, setChartLimit] = useState<BarLimitOption>(20);
  const [hoveredId, setHoveredId] = useState<string | null>(null);

  return (
    <section className="panel bm-chart-panel">
      <div className="bm-chart-head">
        <div>
          <h3 className="bm-chart-title">
            {chartMode === "bar"
              ? `${dimensionLabel}排行柱形图`
              : `${dimensionLabel} × ${chartMode === "price" ? "综合单价（对数轴）" : "输出速度（tok/s）"}`}
          </h3>
          <p className="bm-chart-sub">
            {chartMode === "bar"
              ? "官网同款柱形排行：悬停柱子同步高亮同模型全部等级，绿点代表本机已用"
              : `虚线为帕累托前沿（${chartMode === "price" ? "越靠左上性价比越高" : "越靠右上又快又强"}），悬停自动连线展示同模型各等级`}
          </p>
        </div>
        <div className="bm-chart-controls">
          <div className="markets-pill-group">
            <button
              type="button"
              className={`markets-pill-btn ${chartMode === "bar" ? "active" : ""}`}
              onClick={() => onChangeChartMode("bar")}
            >
              排行柱形图
            </button>
            <button
              type="button"
              className={`markets-pill-btn ${chartMode === "price" ? "active" : ""}`}
              onClick={() => onChangeChartMode("price")}
            >
              性价比散点
            </button>
            <button
              type="button"
              className={`markets-pill-btn ${chartMode === "speed" ? "active" : ""}`}
              onClick={() => onChangeChartMode("speed")}
            >
              速度散点
            </button>
          </div>
          <div className="markets-pill-group">
            {(
              [
                [20, "Top 20"],
                [35, "Top 35"],
                [0, "全部"],
              ] as const
            ).map(([limit, label]) => (
              <button
                key={limit}
                type="button"
                className={`markets-pill-btn ${chartLimit === limit ? "active" : ""}`}
                onClick={() => setChartLimit(limit)}
              >
                {label}
              </button>
            ))}
          </div>
        </div>
      </div>

      {chartMode === "bar" ? (
        <OfficialBarChart
          models={models}
          dimension={dimension}
          dimensionLabel={dimensionLabel}
          limit={chartLimit}
          hoveredId={hoveredId}
          onHover={setHoveredId}
          selectedId={selectedId}
          onSelectModel={onSelectModel}
          localMatches={localMatches}
        />
      ) : (
        <ScatterPlot
          models={models}
          dimension={dimension}
          dimensionLabel={dimensionLabel}
          axisMode={chartMode}
          limit={chartLimit}
          hoveredId={hoveredId}
          onHover={setHoveredId}
          selectedId={selectedId}
          onSelectModel={onSelectModel}
          localMatches={localMatches}
        />
      )}
    </section>
  );
}

function OfficialBarChart({
  models,
  dimension,
  dimensionLabel,
  limit,
  hoveredId,
  onHover,
  selectedId,
  onSelectModel,
  localMatches,
}: {
  models: BenchmarkModel[];
  dimension: BenchmarkDimensionKey;
  dimensionLabel: string;
  limit: BarLimitOption;
  hoveredId: string | null;
  onHover: (id: string | null) => void;
  selectedId: string | null;
  onSelectModel: (id: string) => void;
  localMatches: Map<string, LocalModelMatch>;
}) {
  const { bars, maxY, yTicks, creatorLegend, totalScored } = useMemo(() => {
    const scored = models
      .map((model) => ({
        model,
        score: getDimensionScore(model, dimension),
        parsed: parseModelVariant(model),
      }))
      .filter(
        (
          x,
        ): x is {
          model: BenchmarkModel;
          score: number;
          parsed: ReturnType<typeof parseModelVariant>;
        } => x.score != null && x.score > 0,
      )
      .sort(
        (a, b) => b.score - a.score || a.model.name.localeCompare(b.model.name),
      );

    const total = scored.length;
    const sliced = limit > 0 ? scored.slice(0, limit) : scored;
    if (sliced.length === 0) {
      return {
        bars: [] as Array<{
          model: BenchmarkModel;
          score: number;
          familyKey: string;
          label: string;
          tierLabel: string;
          color: string;
          isLocal: boolean;
        }>,
        maxY: 100,
        yTicks: [] as number[],
        creatorLegend: [] as Array<{ name: string; color: string }>,
        totalScored: 0,
      };
    }

    const peak = Math.max(...sliced.map((s) => s.score));
    const step = peak <= 30 ? 5 : peak <= 80 ? 10 : 20;
    const topY = Math.ceil((peak * 1.1) / step) * step;
    const ticks: number[] = [];
    for (let v = 0; v <= topY; v += step) ticks.push(v);

    const creatorCounts = new Map<string, number>();
    const items = sliced.map((s) => {
      creatorCounts.set(
        s.model.creatorName,
        (creatorCounts.get(s.model.creatorName) ?? 0) + 1,
      );
      return {
        model: s.model,
        score: s.score,
        familyKey: s.parsed.familyKey,
        label: shortModelLabel(s.model),
        tierLabel: s.parsed.tierLabel,
        color: creatorColor(s.model.creatorName),
        isLocal: localMatches.has(s.model.id),
      };
    });

    const legend = [...creatorCounts.entries()]
      .sort((a, b) => b[1] - a[1])
      .slice(0, 8)
      .map(([name]) => ({ name, color: creatorColor(name) }));

    return {
      bars: items,
      maxY: topY,
      yTicks: ticks,
      creatorLegend: legend,
      totalScored: total,
    };
  }, [models, dimension, limit, localMatches]);

  if (bars.length === 0) {
    return (
      <div className="bm-chart-empty">
        当前筛选条件下暂无具备「{dimensionLabel}」评分的模型。
      </div>
    );
  }

  const activeBar =
    bars.find((b) => b.model.id === hoveredId) ??
    bars.find((b) => b.model.id === selectedId) ??
    null;
  const focusFamilyKey = hoveredId
    ? (bars.find((b) => b.model.id === hoveredId)?.familyKey ?? null)
    : null;

  const padL = 104;
  const padR = 24;
  const padT = 28;
  const padB = 142;
  const chartH = 215;
  const totalH = padT + chartH + padB;
  // Keep every bar comfortably wide; scroll horizontally when showing >22 bars
  const slotW = bars.length <= 20 ? 40 : 34;
  const barW = bars.length <= 20 ? 26 : 22;
  const svgW = Math.max(820, padL + padR + bars.length * slotW);
  const actualSlot = (svgW - padL - padR) / bars.length;

  const yPos = (val: number) => padT + (1 - val / maxY) * chartH;

  return (
    <div className="bm-scatter-wrap">
      <div className="bm-bar-scroll">
        <svg
          className="bm-bar-svg"
          width={svgW}
          height={totalH}
          viewBox={`0 0 ${svgW} ${totalH}`}
          role="img"
          aria-label={`${dimensionLabel}柱形图`}
        >
          {/* Horizontal grid lines */}
          {yTicks.map((tick) => {
            const y = yPos(tick);
            return (
              <g key={`yt-${tick}`}>
                <line
                  x1={padL}
                  y1={y}
                  x2={svgW - padR}
                  y2={y}
                  className={tick === 0 ? "bm-bar-baseline" : "bm-grid-line"}
                />
                <text
                  x={padL - 8}
                  y={y + 3.5}
                  textAnchor="end"
                  className="bm-axis-text"
                >
                  {tick}
                </text>
              </g>
            );
          })}

          {/* Vertical Bars */}
          {bars.map((b, idx) => {
            const cx = padL + actualSlot * idx + actualSlot / 2;
            const x = cx - barW / 2;
            const top = yPos(b.score);
            const h = Math.max(2, padT + chartH - top);
            const isSelected = b.model.id === selectedId;
            const isHovered = b.model.id === hoveredId;
            const isSameFamily =
              focusFamilyKey != null && b.familyKey === focusFamilyKey;
            const dimmed =
              focusFamilyKey != null && !isSameFamily && !isSelected;

            return (
              <g
                key={b.model.id}
                className="bm-bar-col"
                style={{ opacity: dimmed ? 0.34 : 1 }}
                onMouseEnter={() => onHover(b.model.id)}
                onMouseLeave={() => onHover(null)}
                onClick={() => onSelectModel(b.model.id)}
              >
                {/* Invisible wider hit target for smooth hover */}
                <rect
                  x={padL + actualSlot * idx}
                  y={padT}
                  width={actualSlot}
                  height={chartH + padB - 6}
                  fill="transparent"
                />

                {/* Bar rectangle with rounded top */}
                <rect
                  x={x}
                  y={top}
                  width={barW}
                  height={h}
                  rx={4}
                  ry={4}
                  fill={b.color}
                  fillOpacity={
                    isSelected || isHovered || isSameFamily ? 1 : 0.88
                  }
                  stroke={
                    isSelected || isHovered
                      ? "var(--text)"
                      : isSameFamily
                        ? b.color
                        : "none"
                  }
                  strokeWidth={isSelected || isHovered ? 1.8 : 0}
                />

                {/* Score value right above bar */}
                <text
                  x={cx}
                  y={top - 6}
                  textAnchor="middle"
                  className={`bm-bar-val ${isSelected || isHovered || isSameFamily ? "active" : ""}`}
                >
                  {formatScore(b.score)}
                </text>

                {/* Local-used emerald indicator dot above score */}
                {b.isLocal && (
                  <circle
                    cx={cx}
                    cy={top - 18}
                    r={3.2}
                    fill="#10b981"
                    stroke="var(--surface)"
                    strokeWidth={1}
                  />
                )}

                {/* Angled X-axis model label */}
                <g
                  transform={`translate(${cx + 3}, ${padT + chartH + 10}) rotate(-42)`}
                >
                  <text
                    x={0}
                    y={0}
                    textAnchor="end"
                    className={`bm-bar-xlabel ${isSelected || isHovered || isSameFamily ? "active" : ""}`}
                  >
                    {b.label}
                  </text>
                </g>
              </g>
            );
          })}
        </svg>
      </div>

      {/* Bottom Legend + Active Bar Readout */}
      <div className="bm-chart-foot">
        <div className="bm-creator-legend">
          {creatorLegend.map((c) => (
            <span key={c.name} className="bm-legend-item">
              <i style={{ background: c.color }} />
              {c.name}
            </span>
          ))}
          <span className="bm-legend-item">
            <i style={{ background: "#10b981" }} />
            本机已用
          </span>
          <span className="bm-legend-count">
            展示 {bars.length} / {totalScored} 个有评分变体
          </span>
        </div>

        {activeBar && (
          <div className="bm-hover-readout">
            <strong>{activeBar.model.name}</strong>
            <span>{activeBar.model.creatorName}</span>
            <span>
              {dimensionLabel} <b>{formatScore(activeBar.score)}</b>
            </span>
            <span>
              价格{" "}
              <b>
                {formatPrice1m(activeBar.model.priceInput1m)} /{" "}
                {formatPrice1m(activeBar.model.priceOutput1m)}
              </b>
            </span>
            <span>
              速度 <b>{formatTps(effectiveTps(activeBar.model))}</b>
            </span>
            <span>
              TTFT <b>{formatSeconds(effectiveTtftS(activeBar.model))}</b>
            </span>
            {localMatches.has(activeBar.model.id) && (
              <span className="bm-readout-local">
                本机 {localMatches.get(activeBar.model.id)!.requests} 次 · $
                {localMatches.get(activeBar.model.id)!.totalCostUsd.toFixed(2)}
              </span>
            )}
          </div>
        )}
      </div>
    </div>
  );
}

function ScatterPlot({
  models,
  dimension,
  dimensionLabel,
  axisMode,
  limit,
  hoveredId,
  onHover,
  selectedId,
  onSelectModel,
  localMatches,
}: {
  models: BenchmarkModel[];
  dimension: BenchmarkDimensionKey;
  dimensionLabel: string;
  axisMode: "price" | "speed";
  limit: BarLimitOption;
  hoveredId: string | null;
  onHover: (id: string | null) => void;
  selectedId: string | null;
  onSelectModel: (id: string) => void;
  localMatches: Map<string, LocalModelMatch>;
}) {
  const {
    points,
    frontierPath,
    xTicks,
    yTicks,
    creatorLegend,
    totalPlottable,
  } = useMemo(() => {
    const allRaw: Array<{ model: BenchmarkModel; x: number; y: number }> = [];
    for (const m of models) {
      const y = getDimensionScore(m, dimension);
      if (y == null) continue;
      const x = axisMode === "price" ? blendedPrice1m(m) : effectiveTps(m);
      if (x == null || x <= 0) continue;
      allRaw.push({ model: m, x, y });
    }
    allRaw.sort(
      (a, b) => b.y - a.y || a.model.name.localeCompare(b.model.name),
    );
    const total = allRaw.length;
    const raw = limit > 0 ? allRaw.slice(0, limit) : allRaw;
    if (raw.length === 0) {
      return {
        points: [] as PlotItem[],
        frontierPath: "",
        xTicks: [] as Array<{ pos: number; label: string }>,
        yTicks: [] as Array<{ pos: number; label: string }>,
        creatorLegend: [] as Array<{ name: string; color: string }>,
        totalPlottable: 0,
      };
    }

    const xs = raw.map((r) => r.x);
    const ys = raw.map((r) => r.y);
    const minY = Math.max(0, Math.floor((Math.min(...ys) - 3) / 5) * 5);
    const maxY = Math.ceil((Math.max(...ys) + 3) / 5) * 5;
    const spanY = Math.max(5, maxY - minY);

    let mapX: (x: number) => number;
    let builtXTicks: Array<{ pos: number; label: string }> = [];

    if (axisMode === "price") {
      const minLog = Math.log10(Math.max(0.05, Math.min(...xs) * 0.8));
      const maxLog = Math.log10(Math.max(1, Math.max(...xs) * 1.25));
      const spanLog = Math.max(0.5, maxLog - minLog);
      mapX = (x) =>
        PAD_L + ((Math.log10(Math.max(0.05, x)) - minLog) / spanLog) * PLOT_W;
      const candidates = [0.1, 0.25, 0.5, 1, 2, 5, 10, 20, 50, 100];
      builtXTicks = candidates
        .filter(
          (v) => Math.log10(v) >= minLog - 0.02 && Math.log10(v) <= maxLog,
        )
        .map((v) => ({
          pos: mapX(v),
          label: `$${v}`,
        }));
    } else {
      const maxX = Math.ceil((Math.max(...xs) * 1.08) / 50) * 50;
      const spanX = Math.max(50, maxX);
      mapX = (x) => PAD_L + (Math.min(x, spanX) / spanX) * PLOT_W;
      const step =
        spanX <= 150 ? 25 : spanX <= 350 ? 50 : spanX <= 700 ? 100 : 200;
      for (let v = 0; v <= spanX; v += step) {
        builtXTicks.push({ pos: mapX(v), label: `${v}` });
      }
    }

    const mapY = (y: number) => PAD_T + (1 - (y - minY) / spanY) * PLOT_H;

    const yStep = spanY <= 25 ? 5 : spanY <= 60 ? 10 : 20;
    const builtYTicks: Array<{ pos: number; label: string }> = [];
    for (let v = minY; v <= maxY; v += yStep) {
      builtYTicks.push({ pos: mapY(v), label: `${v}` });
    }

    const frontier = computeParetoFrontier(
      raw.map((r) => ({ id: r.model.id, x: r.x, y: r.y })),
      axisMode,
    );
    const frontierIds = new Set(frontier.map((f) => f.id));

    const plotted: PlotItem[] = raw.map((r) => ({
      id: r.model.id,
      model: r.model,
      x: r.x,
      y: r.y,
      cx: mapX(r.x),
      cy: mapY(r.y),
      color: creatorColor(r.model.creatorName),
      isLocal: localMatches.has(r.model.id),
      isPareto: frontierIds.has(r.model.id),
    }));

    const pathD = frontier
      .map(
        (pt, i) =>
          `${i === 0 ? "M" : "L"}${mapX(pt.x).toFixed(1)},${mapY(pt.y).toFixed(1)}`,
      )
      .join(" ");

    const creatorCounts = new Map<string, number>();
    for (const p of plotted) {
      creatorCounts.set(
        p.model.creatorName,
        (creatorCounts.get(p.model.creatorName) ?? 0) + 1,
      );
    }
    const legend = [...creatorCounts.entries()]
      .sort((a, b) => b[1] - a[1])
      .slice(0, 7)
      .map(([name]) => ({ name, color: creatorColor(name) }));

    return {
      points: plotted,
      frontierPath: pathD,
      xTicks: builtXTicks,
      yTicks: builtYTicks,
      creatorLegend: legend,
      totalPlottable: total,
    };
  }, [models, dimension, axisMode, limit, localMatches]);

  const activePoint = useMemo(
    () =>
      points.find((p) => p.id === hoveredId) ??
      points.find((p) => p.id === selectedId) ??
      null,
    [points, hoveredId, selectedId],
  );

  const siblingCurve = useMemo(() => {
    if (!activePoint)
      return {
        path: "",
        siblings: [] as Array<PlotItem & { tierLabel: string }>,
      };
    const activeFamily = parseModelVariant(activePoint.model).familyKey;
    const siblings = points
      .map((p) => ({ p, parsed: parseModelVariant(p.model) }))
      .filter((x) => x.parsed.familyKey === activeFamily)
      .sort((a, b) => b.parsed.tierOrder - a.parsed.tierOrder)
      .map((x) => ({ ...x.p, tierLabel: x.parsed.tierLabel }));
    if (siblings.length < 2) {
      return {
        path: "",
        siblings: [] as Array<PlotItem & { tierLabel: string }>,
      };
    }
    const path = siblings
      .map(
        (s, i) => `${i === 0 ? "M" : "L"}${s.cx.toFixed(1)},${s.cy.toFixed(1)}`,
      )
      .join(" ");
    return { path, siblings };
  }, [activePoint, points]);

  const paretoLabels = useMemo(() => {
    const candidates = points.filter((p) => p.isPareto);
    const kept: PlotItem[] = [];
    for (const p of candidates) {
      const collides = kept.some(
        (k) => Math.abs(k.cx - p.cx) < 96 && Math.abs(k.cy - p.cy) < 18,
      );
      if (!collides) kept.push(p);
    }
    return kept;
  }, [points]);

  if (points.length === 0) {
    return (
      <div className="bm-chart-empty">
        当前筛选条件下暂无可绘制散点的数据（需要同时具备评分与
        {axisMode === "price" ? "价格" : "速度"}指标）。
      </div>
    );
  }

  return (
    <div className="bm-scatter-wrap">
      <svg
        className="bm-scatter-svg"
        viewBox={`0 0 ${SCATTER_W} ${SCATTER_H}`}
        role="img"
        aria-label={`${dimensionLabel}散点图`}
      >
        {yTicks.map((t) => (
          <g key={`y-${t.label}`}>
            <line
              x1={PAD_L}
              y1={t.pos}
              x2={SCATTER_W - PAD_R}
              y2={t.pos}
              className="bm-grid-line"
            />
            <text
              x={PAD_L - 8}
              y={t.pos + 4}
              textAnchor="end"
              className="bm-axis-text"
            >
              {t.label}
            </text>
          </g>
        ))}

        {xTicks.map((t) => (
          <g key={`x-${t.label}`}>
            <line
              x1={t.pos}
              y1={PAD_T}
              x2={t.pos}
              y2={SCATTER_H - PAD_B}
              className="bm-grid-line"
            />
            <text
              x={t.pos}
              y={SCATTER_H - PAD_B + 16}
              textAnchor="middle"
              className="bm-axis-text"
            >
              {t.label}
            </text>
          </g>
        ))}

        <text
          x={SCATTER_W - PAD_R}
          y={SCATTER_H - 6}
          textAnchor="end"
          className="bm-axis-caption"
        >
          {axisMode === "price"
            ? "综合价格 ($ / 1M tokens，3:1 输入输出)"
            : "平均输出速度 (tokens / s)"}
        </text>

        {frontierPath && <path d={frontierPath} className="bm-pareto-line" />}

        {siblingCurve.path && activePoint && (
          <path
            d={siblingCurve.path}
            fill="none"
            stroke={activePoint.color}
            strokeWidth={2}
            strokeOpacity={0.85}
            pointerEvents="none"
          />
        )}

        {points.map((p) => {
          const isSelected = p.id === selectedId;
          const isHovered = p.id === hoveredId;
          const isSibling = siblingCurve.siblings.some((s) => s.id === p.id);
          const r =
            isSelected || isHovered
              ? 7
              : isSibling
                ? 5.8
                : p.isPareto || p.isLocal
                  ? 5.2
                  : 3.8;
          return (
            <g
              key={p.id}
              className="bm-dot-group"
              onMouseEnter={() => onHover(p.id)}
              onMouseLeave={() => onHover(null)}
              onClick={() => onSelectModel(p.id)}
            >
              {p.isLocal && (
                <circle
                  cx={p.cx}
                  cy={p.cy}
                  r={r + 4}
                  fill="none"
                  stroke={p.color}
                  strokeWidth={1.6}
                  strokeOpacity={0.65}
                />
              )}
              <circle
                cx={p.cx}
                cy={p.cy}
                r={r}
                fill={p.color}
                fillOpacity={
                  isSelected || isHovered || isSibling
                    ? 1
                    : p.isPareto
                      ? 0.92
                      : 0.68
                }
                stroke={
                  isSelected || isHovered || isSibling
                    ? "var(--text)"
                    : "var(--surface)"
                }
                strokeWidth={
                  isSelected || isHovered ? 2 : isSibling ? 1.6 : 1.1
                }
              />
            </g>
          );
        })}

        {siblingCurve.siblings.length >= 2
          ? siblingCurve.siblings.map((s) => {
              const anchor = s.cx > SCATTER_W - 140 ? "end" : "start";
              const dx = anchor === "end" ? -9 : 9;
              return (
                <text
                  key={`sib-${s.id}`}
                  x={s.cx + dx}
                  y={s.cy + 4}
                  textAnchor={anchor}
                  className="bm-pareto-label bm-sibling-label"
                  pointerEvents="none"
                >
                  {s.tierLabel} ({formatScore(s.y)})
                </text>
              );
            })
          : paretoLabels.map((p) => {
              const shortName = shortModelLabel(p.model);
              const anchor = p.cx > SCATTER_W - 190 ? "end" : "start";
              const dx = anchor === "end" ? -9 : 9;
              return (
                <text
                  key={`lbl-${p.id}`}
                  x={p.cx + dx}
                  y={p.cy + 4}
                  textAnchor={anchor}
                  className="bm-pareto-label"
                  pointerEvents="none"
                >
                  {shortName}
                </text>
              );
            })}
      </svg>

      <div className="bm-chart-foot">
        <div className="bm-creator-legend">
          {creatorLegend.map((c) => (
            <span key={c.name} className="bm-legend-item">
              <i style={{ background: c.color }} />
              {c.name}
            </span>
          ))}
          <span className="bm-legend-item bm-legend-local">
            <i />
            本机已用
          </span>
          <span className="bm-legend-count">
            展示 {points.length} / {totalPlottable} 个变体
          </span>
        </div>

        {activePoint && (
          <div className="bm-hover-readout">
            <strong>{activePoint.model.name}</strong>
            <span>{activePoint.model.creatorName}</span>
            <span>
              {dimensionLabel} <b>{formatScore(activePoint.y)}</b>
            </span>
            <span>
              价格{" "}
              <b>
                {formatPrice1m(activePoint.model.priceInput1m)} /{" "}
                {formatPrice1m(activePoint.model.priceOutput1m)}
              </b>
            </span>
            <span>
              速度 <b>{formatTps(effectiveTps(activePoint.model))}</b>
            </span>
            <span>
              TTFT <b>{formatSeconds(effectiveTtftS(activePoint.model))}</b>
            </span>
            {localMatches.has(activePoint.id) && (
              <span className="bm-readout-local">
                本机 {localMatches.get(activePoint.id)!.requests} 次 · $
                {localMatches.get(activePoint.id)!.totalCostUsd.toFixed(2)}
              </span>
            )}
          </div>
        )}
      </div>
    </div>
  );
}
