import {
  type ChartPoint,
  type ChartRange,
  fmtChartTime,
  fmtPrice,
  fmtSigned,
} from "./marketsShared";
import { useState } from "react";

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
      <svg width={width} height={height} className="markets-mini-svg">
        <line
          x1={2}
          y1={height / 2}
          x2={width - 2}
          y2={height / 2}
          stroke="var(--line)"
          strokeDasharray="2 2"
        />
      </svg>
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

export function InteractivePriceChart({
  points,
  prevClose,
  currency,
  range,
  color,
  loading,
  error,
}: {
  points: ChartPoint[];
  prevClose: number | null;
  currency: string;
  range: ChartRange;
  color: string;
  loading: boolean;
  error: string | null;
}) {
  const [hoverIdx, setHoverIdx] = useState<number | null>(null);

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

  const W = 620;
  const H = 230;
  const padL = 62;
  const padR = 14;
  const padT = 16;
  const padB = 26;
  const plotW = W - padL - padR;
  const plotH = H - padT - padB;

  const prices = points.map((p) => p.price);
  const rawMin = Math.min(...prices, ...(prevClose ? [prevClose] : []));
  const rawMax = Math.max(...prices, ...(prevClose ? [prevClose] : []));
  const padVal = Math.max((rawMax - rawMin) * 0.08, rawMax * 0.001, 1e-4);
  const minP = rawMin - padVal;
  const maxP = rawMax + padVal;

  const xOf = (i: number) => padL + (i / (points.length - 1)) * plotW;
  const yOf = (v: number) => padT + (1 - (v - minP) / (maxP - minP)) * plotH;

  const linePts = points
    .map((p, i) => `${xOf(i).toFixed(1)},${yOf(p.price).toFixed(1)}`)
    .join(" ");
  const areaPts = `${padL},${padT + plotH} ${linePts} ${padL + plotW},${padT + plotH}`;
  const yTicks = [0, 0.33, 0.67, 1].map((r) => minP + (maxP - minP) * r);
  const xTickIndices = Array.from(
    new Set([
      0,
      Math.floor((points.length - 1) * 0.33),
      Math.floor((points.length - 1) * 0.67),
      points.length - 1,
    ]),
  );

  const hoverPt = hoverIdx != null ? points[hoverIdx] : null;
  const baseRef = prevClose ?? points[0].price;
  const hoverDiff = hoverPt ? hoverPt.price - baseRef : 0;
  const hoverPct = baseRef > 0 ? (hoverDiff / baseRef) * 100 : 0;

  return (
    <div
      className="markets-chart-wrap"
      onPointerMove={(e) => {
        const rect = e.currentTarget.getBoundingClientRect();
        const relX = ((e.clientX - rect.left) / Math.max(1, rect.width)) * W;
        const ratio = Math.max(0, Math.min(1, (relX - padL) / plotW));
        const idx = Math.round(ratio * (points.length - 1));
        setHoverIdx((prev) => (prev === idx ? prev : idx));
      }}
      onPointerLeave={() => setHoverIdx(null)}
    >
      <svg
        className="markets-chart-svg"
        viewBox={`0 0 ${W} ${H}`}
        role="img"
        aria-label="标的走势图"
      >
        {yTicks.map((val, i) => {
          const y = yOf(val);
          return (
            <g key={i}>
              <line
                x1={padL}
                y1={y}
                x2={W - padR}
                y2={y}
                stroke="var(--line)"
                strokeDasharray="3 3"
                opacity={0.65}
              />
              <text
                x={padL - 8}
                y={y + 4}
                textAnchor="end"
                fill="var(--muted)"
                fontSize={10.5}
              >
                {val.toFixed(val >= 100 ? 1 : 2)}
              </text>
            </g>
          );
        })}

        {prevClose != null && (
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
        )}

        <polygon points={areaPts} fill={color} opacity={0.14} />
        <polyline
          fill="none"
          stroke={color}
          strokeWidth={2}
          strokeLinecap="round"
          strokeLinejoin="round"
          points={linePts}
        />

        {xTickIndices.map((idx) => (
          <text
            key={idx}
            x={xOf(idx)}
            y={H - 6}
            textAnchor={
              idx === 0 ? "start" : idx === points.length - 1 ? "end" : "middle"
            }
            fill="var(--muted)"
            fontSize={10.5}
          >
            {fmtChartTime(points[idx].ts, range)}
          </text>
        ))}

        {hoverIdx != null && hoverPt && (
          <g>
            <line
              x1={xOf(hoverIdx)}
              y1={padT}
              x2={xOf(hoverIdx)}
              y2={padT + plotH}
              stroke="var(--text)"
              strokeWidth={1}
              strokeDasharray="3 2"
              opacity={0.45}
            />
            <circle
              cx={xOf(hoverIdx)}
              cy={yOf(hoverPt.price)}
              r={4.2}
              fill={color}
              stroke="var(--surface)"
              strokeWidth={1.6}
            />
          </g>
        )}
      </svg>

      {hoverIdx != null && hoverPt && (
        <div
          className="markets-chart-tip"
          style={{
            left: `clamp(10px, calc(${((xOf(hoverIdx) / W) * 100).toFixed(
              1,
            )}% - 78px), calc(100% - 180px))`,
          }}
        >
          <small>{fmtChartTime(hoverPt.ts, range)}</small>
          <strong>{fmtPrice(hoverPt.price, currency)}</strong>
          <span>
            {fmtSigned(hoverDiff)} ({fmtSigned(hoverPct, "%")})
          </span>
        </div>
      )}
    </div>
  );
}
