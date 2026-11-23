import { type Dashboard } from "./types";
import { useMemo, useState } from "react";
import {
  isHourlyTrend,
  trendHitRate,
  trendLabel,
  trendTokens,
} from "./chartData";
import { formatMoney, formatTokens } from "./ui";
import { waveColor } from "../theme/wave";

export function UsageChart({
  data,
  metric,
  chartType,
}: {
  data: Dashboard["trends"];
  metric: string;
  chartType: "bar" | "area" | "blocks";
}) {
  const [hover, setHover] = useState<number | null>(null);
  // The derived values do not depend on hover: a hover change should not
  // trigger a recomputation.
  const { values, max, hourly, runs } = useMemo(() => {
    const values = data.map((d) =>
      metric === "cost"
        ? Number(d.totalCost)
        : metric === "requests"
          ? d.requestCount
          : metric === "hitRate"
            ? (trendHitRate(d) ?? 0) * 100
            : trendTokens(d),
    );
    const max = metric === "hitRate" ? 100 : Math.max(1, ...values) * 1.08;
    const hourly = isHourlyTrend(data);
    const runs: number[][] = [];
    let run: number[] = [];
    for (let i = 0; i < values.length; i++) {
      if (metric === "hitRate" && trendHitRate(data[i]) === null) {
        if (run.length) runs.push(run);
        run = [];
      } else run.push(i);
    }
    if (run.length) runs.push(run);
    return { values, max, hourly, runs };
  }, [data, metric]);
  const x = (i: number) => (i * 1000) / Math.max(1, values.length - 1);
  const y = (i: number) => 168 - (values[i] / max) * 150;
  const label = (v: number) =>
    metric === "cost"
      ? formatMoney(v)
      : metric === "hitRate"
        ? v.toFixed(1) + "%"
        : metric === "requests"
          ? Math.round(v).toLocaleString()
          : formatTokens(Math.round(v));
  if (chartType === "blocks") {
    const peak = metric === "hitRate" ? 100 : Math.max(0, ...values);
    const firstDate = data[0]?.date.slice(0, 10);
    const [year, month, day] = (firstDate ?? "").split("-").map(Number);
    const padding =
      !hourly && year && month && day
        ? new Date(year, month - 1, day).getDay()
        : 0;
    const rows = hourly ? 4 : 7;
    const columns = Math.max(1, Math.ceil((padding + data.length) / rows));
    const monthLabels = hourly
      ? []
      : data
          .flatMap((d, i) => {
            const current = d.date.slice(0, 7);
            if (
              !/^\d{4}-\d{2}$/.test(current) ||
              (i > 0 && data[i - 1].date.slice(0, 7) === current)
            )
              return [];
            return [
              {
                label: `${Number(current.slice(5, 7))}月`,
                column: Math.floor((padding + i) / rows) + 1,
              },
            ];
          })
          .filter(
            (item, index, all) =>
              index === 0 || item.column !== all[index - 1].column,
          );
    const name =
      metric === "cost"
        ? "估算费用"
        : metric === "requests"
          ? "请求次数"
          : metric === "hitRate"
            ? "缓存命中率"
            : "总 Token";
    return (
      <div className="usage-block-chart">
        <div className="legend">
          <span>{name}</span>
          <span>
            {hourly ? "按小时" : "按日"} ·{" "}
            {metric === "hitRate"
              ? "颜色以 100% 为上限"
              : "颜色按当前范围相对深浅显示"}
          </span>
        </div>
        {data.length ? (
          <div className="usage-block-scroll">
            <div
              className={`usage-block-calendar${columns >= 8 ? " centered" : ""}`}
              style={
                {
                  "--block-columns": columns,
                  "--block-rows": rows,
                } as React.CSSProperties
              }
            >
              {!hourly && (
                <div className="usage-block-months" aria-hidden="true">
                  {monthLabels.map((m) => (
                    <span
                      key={m.label + m.column}
                      style={{ gridColumnStart: m.column }}
                    >
                      {m.label}
                    </span>
                  ))}
                </div>
              )}
              <div
                className="usage-block-grid"
                aria-label={`${name}方块图`}
                // Event delegation: the hover listener sits on the container
                // instead of four closures per block, so sweeping the mouse
                // over hundreds of blocks no longer rebuilds the event
                // attributes one by one.
                onMouseOver={(e) => {
                  const index = (e.target as HTMLElement).dataset?.index;
                  if (index !== undefined) setHover(Number(index));
                }}
                onMouseLeave={() => setHover(null)}
                onFocus={(e) => {
                  const index = (e.target as HTMLElement).dataset?.index;
                  if (index !== undefined) setHover(Number(index));
                }}
                onBlur={(e) => {
                  const next = e.relatedTarget as Node | null;
                  if (!next || !e.currentTarget.contains(next)) setHover(null);
                }}
              >
                {Array.from({ length: padding }, (_, i) => (
                  <span
                    className="usage-block-placeholder"
                    aria-hidden="true"
                    key={`pad-${i}`}
                  />
                ))}
                {data.map((d, i) => {
                  const unavailable =
                    metric === "hitRate" && trendHitRate(d) === null;
                  const level =
                    unavailable || values[i] <= 0 || peak <= 0
                      ? 0
                      : Math.min(
                          4,
                          Math.max(1, Math.ceil((values[i] / peak) * 4)),
                        );
                  const description = `${d.date} · ${name}：${unavailable ? "无可缓存输入" : label(values[i])}`;
                  return (
                    <span
                      key={`${d.date}-${i}`}
                      data-index={i}
                      role="img"
                      aria-label={description}
                      title={description}
                      tabIndex={0}
                      className={`usage-block level-${level}${unavailable ? " unavailable" : ""}`}
                    />
                  );
                })}
              </div>
            </div>
          </div>
        ) : (
          <p className="usage-block-empty">所选范围暂无趋势数据</p>
        )}
        <div className="usage-block-footer">
          <span>
            {hover !== null && data[hover] ? (
              `${data[hover].date} · ${name}：${metric === "hitRate" && trendHitRate(data[hover]) === null ? "无可缓存输入" : label(values[hover])}`
            ) : (
              <>
                {data[0] ? trendLabel(data[0].date, hourly) : ""}{" "}
                {data.length > 1
                  ? `— ${trendLabel(data[data.length - 1].date, hourly)}`
                  : ""}
              </>
            )}
          </span>
          <span>
            少{" "}
            <i className="usage-block-legend">
              {[0, 1, 2, 3, 4].map((n) => (
                <b key={n} className={`level-${n}`} />
              ))}
            </i>{" "}
            多
          </span>
        </div>
      </div>
    );
  }
  const indexLabels = Array.from(
    new Set([
      0,
      Math.floor((data.length - 1) / 4),
      Math.floor((data.length - 1) / 2),
      Math.floor(((data.length - 1) * 3) / 4),
      data.length - 1,
    ]),
  ).filter((i) => i >= 0);
  return (
    <div className="trend">
      <div className="legend">
        <span>
          <i style={{ background: "var(--chart-usage)" }} />
          {metric === "cost"
            ? "估算费用（USD）"
            : metric === "requests"
              ? "请求次数"
              : metric === "hitRate"
                ? "缓存命中率"
                : "总 Token（含缓存）"}
        </span>
        <span>{hourly ? "按小时" : "按日"}</span>
      </div>
      <div className="plot-row">
        <div className="yaxis" style={{ height: 170 }}>
          {[1, 0.75, 0.5, 0.25, 0].map((v) => (
            <span key={v}>{label(max * v)}</span>
          ))}
        </div>
        <div
          className="plot"
          style={{ height: 170 }}
          onPointerMove={(e) => {
            const r = e.currentTarget.getBoundingClientRect();
            const at = (e.clientX - r.left) / r.width;
            if (!data.length) return;
            const next = Math.max(
              0,
              Math.min(
                data.length - 1,
                chartType === "bar"
                  ? Math.floor(at * data.length)
                  : Math.round(at * (data.length - 1)),
              ),
            );
            // Moving inside the same block does not change the value:
            // return the original reference so React skips the re-render.
            setHover((prev) => (prev === next ? prev : next));
          }}
          onPointerLeave={() => setHover(null)}
        >
          <svg
            viewBox="0 0 1000 180"
            preserveAspectRatio="none"
            role="img"
            aria-label="所选范围的用量趋势"
          >
            {[8, 48, 88, 128, 168].map((y) => (
              <line
                key={y}
                x1="0"
                x2="1000"
                y1={y}
                y2={y}
                className="chart-grid"
              />
            ))}
            {values.length > 0 &&
              chartType === "bar" &&
              values.map((v, i) => {
                const band = 1000 / values.length;
                const width = Math.min(28, band * 0.62);
                const height = v > 0 ? Math.max(1, (v / max) * 150) : 0;
                const isHover = hover === i;
                return (
                  <rect
                    key={i}
                    x={(i + 0.5) * band - width / 2}
                    y={168 - height}
                    width={width}
                    height={height}
                    rx={Math.min(4, width / 2)}
                    style={{
                      fill: waveColor(i, values.length),
                      opacity: hover === null || isHover ? 1 : 0.45,
                    }}
                  />
                );
              })}
            {chartType === "area" &&
              runs.map((indices, key) => {
                const points = indices.map((i) => `${x(i)},${y(i)}`).join(" ");
                return (
                  <g key={key}>
                    <polygon
                      points={`${x(indices[0])},168 ${points} ${x(indices[indices.length - 1])},168`}
                      fill="var(--chart-usage)"
                      opacity=".16"
                    />
                    <polyline
                      points={points}
                      fill="none"
                      stroke="var(--chart-usage)"
                      strokeWidth="2"
                      vectorEffect="non-scaling-stroke"
                    />
                    {indices.length === 1 && (
                      <circle
                        cx={x(indices[0])}
                        cy={y(indices[0])}
                        r="3"
                        fill="var(--chart-usage)"
                      />
                    )}
                  </g>
                );
              })}
            {hover !== null && values[hover] !== undefined && (
              <g>
                <line
                  x1={
                    chartType === "bar"
                      ? ((hover + 0.5) * 1000) / Math.max(1, values.length)
                      : x(hover)
                  }
                  x2={
                    chartType === "bar"
                      ? ((hover + 0.5) * 1000) / Math.max(1, values.length)
                      : x(hover)
                  }
                  y1={8}
                  y2={168}
                  stroke="var(--text)"
                  strokeWidth="1"
                  strokeDasharray="3 3"
                  opacity="0.35"
                  vectorEffect="non-scaling-stroke"
                />
                {chartType === "area" && (
                  <circle
                    cx={x(hover)}
                    cy={y(hover)}
                    r="4"
                    fill="var(--chart-usage)"
                    stroke="var(--surface)"
                    strokeWidth="1.5"
                    vectorEffect="non-scaling-stroke"
                  />
                )}
              </g>
            )}
          </svg>
          {hover !== null && data[hover] && (
            <div
              className="chart-tooltip"
              style={{
                left: `clamp(8px, calc(${(
                  (chartType === "bar"
                    ? (hover + 0.5) / Math.max(1, data.length)
                    : hover / Math.max(1, data.length - 1)) * 100
                ).toFixed(1)}% - 85px), calc(100% - 190px))`,
              }}
            >
              <small>{data[hover].date}</small>
              <div>
                <strong>
                  {metric === "hitRate" && trendHitRate(data[hover]) === null
                    ? "无可缓存输入"
                    : label(values[hover])}
                </strong>
              </div>
              <small>
                {formatMoney(Number(data[hover].totalCost))} ·{" "}
                {data[hover].requestCount.toLocaleString()} 次 ·{" "}
                {formatTokens(trendTokens(data[hover]))}
              </small>
            </div>
          )}
          {values.length === 0 && (
            <span className="usage-chart-empty">所选范围暂无趋势数据</span>
          )}
        </div>
      </div>
      <div className="xaxis">
        {indexLabels.map((i) => (
          <span key={i}>{data[i] ? trendLabel(data[i].date, hourly) : ""}</span>
        ))}
      </div>
    </div>
  );
}
