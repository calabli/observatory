import { memo, useMemo, useState } from "react";
import {
  getFreshInputTokens,
  isNotBillableUsage,
  isUnpricedUsage,
  type ModelStats,
  type ProviderStats,
  type RequestLog,
} from "../types/usage";
import { dateInput, timeText } from "./usageShared";
import { AppGlyph } from "./AppGlyph";
import { modelDisplayName } from "./modelDisplay";
import { formatMoney, formatTokens } from "./ui";
import { formatRequestSpeed } from "./requestSpeed";
import { ArrowRight, Tags } from "lucide-react";
import { modelSpeeds, type ModelSpeedSort } from "./modelSpeed";
import { waveColor } from "../theme/wave";
import { type Dashboard } from "./types";
import { sortStats, type StatsSortKey } from "./statsSort";

/** A single detail record row. Polls such as lan_status every 10 seconds drag
 *  the whole page into a re-render; the row data references stay stable under
 *  react-query structural sharing, and memo blocks row-level rebuilds of the
 *  whole table caused by unrelated updates. */
export const LogRow = memo(function LogRow({
  log,
  appNameText,
  onOpen,
}: {
  log: RequestLog;
  appNameText: string;
  onOpen: (requestId: string) => void;
}) {
  return (
    <tr onClick={() => onOpen(log.requestId)}>
      <td className="muted">{timeText(log.createdAt)}</td>
      <td>
        <span className="usage-log-identity">
          <AppGlyph app={log.appType} />
          <span>
            {appNameText}
            <small className="cell-sub">
              {modelDisplayName(log.model || "unknown")}
            </small>
            {log.model && modelDisplayName(log.model) !== log.model && (
              <small className="cell-sub">{log.model}</small>
            )}
          </span>
        </span>
      </td>
      <td>{log.providerName ?? log.providerId}</td>
      <td className="numeric">{formatTokens(getFreshInputTokens(log))}</td>
      <td className="numeric">{formatTokens(log.outputTokens)}</td>
      <td className="numeric">{formatTokens(log.cacheReadTokens)}</td>
      <td className="numeric">
        {isNotBillableUsage(log) ? (
          <span
            className="usage-not-billable"
            title="该模型没有公开单价，用量计入套餐，本身不产生费用"
          >
            不计费
          </span>
        ) : isUnpricedUsage(log) ? (
          <span
            className="usage-unpriced"
            title="价格表里还没有这个模型的价格，可以到「价格管理」里补上"
          >
            未计价
          </span>
        ) : (
          formatMoney(log.totalCostUsd, 4)
        )}
      </td>
      <td>
        {log.latencyMs
          ? (log.dataSource !== "proxy" ? "≈ " : "") +
            (log.latencyMs / 1000).toFixed(2) +
            "s"
          : "—"}
      </td>
      <td title="精确速度为输出 Token 除以生成耗时；≈ 表示按日志耗时估算，包含首字等待。">
        {formatRequestSpeed(log)}
      </td>
      <td>
        <span
          className={
            log.statusCode >= 200 && log.statusCode < 300 ? "positive" : ""
          }
        >
          {log.statusCode}
        </span>
      </td>
      <td>
        <button
          className="icon-button"
          aria-label={"查看请求 " + log.requestId}
          onClick={(e) => {
            e.stopPropagation();
            onOpen(log.requestId);
          }}
        >
          <ArrowRight size={12} />
        </button>
      </td>
    </tr>
  );
});

export function Metric({
  label,
  value,
  detail,
  icon: Icon,
  selected,
  onSelect,
  delta,
}: {
  label: string;
  value: string;
  detail: string;
  icon: typeof Tags;
  selected: boolean;
  onSelect: () => void;
  delta?: { text: string; tone?: "good" | "warn" | "muted" };
}) {
  return (
    <button
      type="button"
      className="stat"
      aria-pressed={selected}
      onClick={onSelect}
    >
      <div className="stat-label">
        <span>{label}</span>
        <span className="stat-label-right">
          {delta && (
            <span className={`stat-delta tone-${delta.tone ?? "muted"}`}>
              {delta.text}
            </span>
          )}
          <Icon size={15} />
        </span>
      </div>
      <div className="stat-value">{value}</div>
      <div className="stat-detail" title={detail}>
        {detail}
      </div>
    </button>
  );
}

/** Horizontal bars of output speed per model, fastest first. */
/** Rows shown before "show all"; the rest stay one click away. */
export const SPEED_ROWS_COLLAPSED = 8;

export function ModelSpeedChart({ models }: { models: ModelStats[] }) {
  const [sort, setSort] = useState<ModelSpeedSort>("usage");
  const [expanded, setExpanded] = useState(false);
  const all = useMemo(() => modelSpeeds(models, sort), [models, sort]);
  const rows = expanded ? all : all.slice(0, SPEED_ROWS_COLLAPSED);
  const hidden = all.length - rows.length;
  const peak = Math.max(1, ...all.map((r) => r.speed));
  const seconds = (ms: number) =>
    ms >= 1000 ? (ms / 1000).toFixed(1) + "s" : Math.round(ms) + "ms";
  return (
    <section className="panel section-gap model-speed">
      <div className="panel-head">
        <h2>模型输出速度</h2>
        <span className="muted">
          输出 Token / 秒 · 所选范围 · ≈ 为按日志耗时估算，含首字等待 ·
          灰色为样本少
        </span>
        <div className="tabs" role="group" aria-label="速度排序">
          {(
            [
              ["usage", "按使用量"],
              ["speed", "按速度"],
            ] as const
          ).map(([key, label]) => (
            <button
              key={key}
              type="button"
              aria-pressed={sort === key}
              onClick={() => setSort(key)}
            >
              {label}
            </button>
          ))}
        </div>
      </div>
      {rows.length ? (
        <div className="model-speed-list">
          {rows.map((r, i) => (
            <div
              className={`model-speed-row${r.lowSample ? " low-sample" : ""}`}
              key={r.model}
              title={`${r.model} · ${r.requests.toLocaleString()} 次请求 · 测速输出 ${r.basisTokens.toLocaleString()} Token${r.lowSample ? "（样本少，仅供参考）" : ""}`}
            >
              <span className="model-speed-name">
                {modelDisplayName(r.model)}
              </span>
              <span className="model-speed-track">
                <i
                  style={{
                    width: `${Math.max(2, (r.speed / peak) * 100)}%`,
                    background: waveColor(i, rows.length),
                  }}
                />
              </span>
              <b className="model-speed-value">
                {r.estimated ? "≈ " : ""}
                {r.speed.toFixed(1)}
                <small> tok/s</small>
              </b>
              <span className="model-speed-first">
                {r.firstTokenMs === null
                  ? "首字 —"
                  : `首字 ${r.firstTokenEstimated ? "≈ " : ""}${seconds(r.firstTokenMs)}`}
              </span>
            </div>
          ))}
          {(hidden > 0 || expanded) && all.length > SPEED_ROWS_COLLAPSED && (
            <button
              type="button"
              className="button model-speed-more"
              onClick={() => setExpanded((v) => !v)}
            >
              {expanded ? "收起" : `展开全部 ${all.length} 个模型`}
            </button>
          )}
        </div>
      ) : (
        <p className="hint">
          所选范围内没有可计算速度的请求（需要至少 100 个输出 Token
          且有耗时记录；较早记录已按日汇总，不含速度）。
        </p>
      )}
    </section>
  );
}

export function ActivityGrid({
  data,
  start,
  end,
}: {
  data: Dashboard["trends"];
  start?: number;
  end?: number;
}) {
  const days = useMemo(() => {
    const endDate = end ? new Date(end * 1000) : new Date();
    endDate.setHours(0, 0, 0, 0);
    const first = new Date(endDate);
    first.setDate(first.getDate() - 125);
    if (start && start * 1000 > first.getTime()) first.setTime(start * 1000);
    first.setHours(0, 0, 0, 0);
    const map = new Map<string, number>();
    for (const d of data) {
      const key = /^\d{4}-\d{2}-\d{2}/.test(d.date)
        ? d.date.slice(0, 10)
        : dateInput(endDate);
      map.set(key, (map.get(key) ?? 0) + d.requestCount);
    }
    const list = [];
    for (let d = new Date(first); d <= endDate; d.setDate(d.getDate() + 1)) {
      const key = dateInput(d);
      list.push({ date: key, count: map.get(key) ?? 0 });
    }
    return list;
  }, [data, start, end]);
  const max = Math.max(1, ...days.map((d) => d.count));
  return (
    <section className="panel section-gap">
      <div className="panel-head">
        <h2>调用活动</h2>
        <span className="muted">
          所选范围内最近 {days.length} 天 · 按日请求数
        </span>
      </div>
      <div className="usage-heatmap">
        {days.map((d) => (
          <div
            key={d.date}
            title={`${d.date} · ${d.count.toLocaleString()} 次请求`}
            aria-label={`${d.date}：${d.count} 次请求`}
            style={{
              background: d.count
                ? `var(--heat-${Math.min(4, Math.max(1, Math.ceil(Math.sqrt(d.count / max) * 4)))})`
                : "var(--soft)",
            }}
          />
        ))}
      </div>
      <div className="usage-meta">
        <span>{days[0]?.date}</span>
        <span>
          少{" "}
          <i className="usage-heat-legend">
            {[1, 2, 3, 4].map((level) => (
              <b key={level} style={{ background: `var(--heat-${level})` }} />
            ))}
          </i>{" "}
          多 · {days.at(-1)?.date}
        </span>
      </div>
    </section>
  );
}

export function StatsTable({
  rows,
  model,
}: {
  rows: (ModelStats | ProviderStats)[];
  model: boolean;
}) {
  const [sort, setSort] = useState<{
    key: StatsSortKey;
    direction: "asc" | "desc";
  }>({ key: "totalCost", direction: "desc" });
  const ordered = useMemo(
    () => sortStats(rows, sort.key, sort.direction),
    [rows, sort],
  );
  const columns: [StatsSortKey, string][] = [
    ["requestCount", "请求"],
    ["totalTokens", "Token"],
    ["totalCost", "费用"],
    ["avgCost", "平均费用"],
    ["firstToken", "首字时间"],
    ["successRate", "成功率"],
  ];
  return (
    <div className="table-wrap">
      <table>
        <thead>
          <tr>
            <th>{model ? "模型" : "供应商 / 应用"}</th>
            {columns.map(([key, label]) => (
              <th
                key={key}
                className="numeric"
                aria-sort={
                  sort.key === key
                    ? sort.direction === "asc"
                      ? "ascending"
                      : "descending"
                    : undefined
                }
              >
                <button
                  type="button"
                  className="usage-sort"
                  onClick={() =>
                    setSort((previous) => ({
                      key,
                      direction:
                        previous.key === key && previous.direction === "desc"
                          ? "asc"
                          : "desc",
                    }))
                  }
                >
                  {label}
                  <span aria-hidden="true">
                    {sort.key === key
                      ? sort.direction === "asc"
                        ? "↑"
                        : "↓"
                      : "↕"}
                  </span>
                </button>
              </th>
            ))}
          </tr>
        </thead>
        <tbody>
          {ordered.map((r, i) => (
            <tr key={i}>
              <td>
                {"model" in r ? modelDisplayName(r.model) : r.providerName}
                {"model" in r && modelDisplayName(r.model) !== r.model && (
                  <small className="cell-sub">{r.model}</small>
                )}
                {"appType" in r && (
                  <small className="cell-sub">{r.appType}</small>
                )}
              </td>
              <td className="numeric">{r.requestCount.toLocaleString()}</td>
              <td className="numeric">{formatTokens(r.totalTokens)}</td>
              <td className="numeric">{formatMoney(r.totalCost)}</td>
              <td className="numeric">
                {formatMoney(
                  "avgCostPerRequest" in r
                    ? r.avgCostPerRequest
                    : Number(r.totalCost) / Math.max(1, r.requestCount),
                  4,
                )}
              </td>
              <td className="numeric">
                {r.firstTokenMedianMs != null
                  ? Math.round(r.firstTokenMedianMs) + " ms"
                  : r.estFirstTokenMedianMs != null
                    ? "≈ " + Math.round(r.estFirstTokenMedianMs) + " ms"
                    : "—"}
              </td>
              <td className="numeric">{r.successRate.toFixed(1)}%</td>
            </tr>
          ))}
        </tbody>
      </table>
      {rows.length === 0 && <p className="empty">暂无匹配记录</p>}
    </div>
  );
}
