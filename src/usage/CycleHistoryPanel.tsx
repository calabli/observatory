import type { UsageSummary } from "../types/usage";
import type { RankingMetric } from "./appRanking";
import { formatMoney, formatTokens } from "./ui";
import { formatCycleShortDate, type CycleWindow } from "./cycles";

export interface CycleHistoryEntry {
  offset: number;
  window: CycleWindow;
  summary?: UsageSummary;
}

function metricNumber(
  summary: UsageSummary | undefined,
  metric: RankingMetric,
) {
  if (!summary) return 0;
  if (metric === "cost") return Number(summary.totalCost || 0);
  if (metric === "requests") return summary.totalRequests || 0;
  if (metric === "hitRate") return (summary.cacheHitRate || 0) * 100;
  return summary.realTotalTokens || 0;
}

function formatMetricValue(value: number, metric: RankingMetric) {
  if (metric === "cost") return formatMoney(value);
  if (metric === "requests") return `${Math.round(value).toLocaleString()} 次`;
  if (metric === "hitRate") return `${value.toFixed(1)}%`;
  return formatTokens(Math.round(value));
}

export function CycleHistoryPanel({
  entries,
  activeOffset,
  metric,
  loading,
  onSelectOffset,
}: {
  entries: CycleHistoryEntry[];
  activeOffset: number;
  metric: RankingMetric;
  loading: boolean;
  onSelectOffset: (offset: number) => void;
}) {
  const values = entries.map((e) => metricNumber(e.summary, metric));
  const peak = metric === "hitRate" ? 100 : Math.max(1e-6, ...values);

  return (
    <section
      className="panel section-gap usage-cycle-history"
      aria-label="近 4 个周期对比"
    >
      <div className="panel-head">
        <h2>近 4 个周期对比</h2>
        <span className="muted">
          {loading ? "读取中…" : "点击任意周期快速跳转"}
        </span>
      </div>
      <div className="usage-cycle-history-grid">
        {entries.map((entry, idx) => {
          const val = values[idx];
          const pct = Math.min(100, Math.max(0, (val / peak) * 100));
          const active = entry.offset === activeOffset;
          const tag =
            entry.offset === 0
              ? "当前周期"
              : `${Math.abs(entry.offset)} 周期前`;
          return (
            <button
              key={entry.offset}
              type="button"
              className={"usage-cycle-history-card" + (active ? " active" : "")}
              aria-pressed={active}
              onClick={() => onSelectOffset(entry.offset)}
            >
              <div className="usage-cycle-history-top">
                <span className="usage-cycle-history-tag">{tag}</span>
                <strong className="tabular-nums">
                  {entry.summary ? formatMetricValue(val, metric) : "—"}
                </strong>
              </div>
              <div className="usage-cycle-history-dates tabular-nums muted">
                {formatCycleShortDate(entry.window.start)} →{" "}
                {formatCycleShortDate(entry.window.end)}
              </div>
              <div className="usage-cycle-history-bar" aria-hidden="true">
                <span style={{ width: `${pct}%` }} />
              </div>
              <div className="usage-cycle-history-sub muted tabular-nums">
                {entry.summary ? (
                  <>
                    <span>{formatMoney(entry.summary.totalCost)}</span>
                    <span>
                      {entry.summary.totalRequests.toLocaleString()} 次
                    </span>
                    <span>{formatTokens(entry.summary.realTotalTokens)}</span>
                  </>
                ) : (
                  <span>加载中…</span>
                )}
              </div>
            </button>
          );
        })}
      </div>
    </section>
  );
}
