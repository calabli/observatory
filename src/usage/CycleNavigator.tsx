import {
  CalendarClock,
  ChevronLeft,
  ChevronRight,
  Pencil,
  Plus,
  RotateCcw,
} from "lucide-react";
import type { UsageSummary } from "../types/usage";
import {
  computeCyclePace,
  formatCountdown,
  formatCycleRange,
  formatCycleRule,
  formatQuotaValue,
  type CycleWindow,
  type UsageCyclePlan,
} from "./cycles";

export function CycleNavigator({
  plans,
  activePlan,
  window,
  offset,
  summary,
  linkedAppName,
  onSelectPlan,
  onPrev,
  onNext,
  onResetOffset,
  onEditPlan,
  onNewPlan,
  onUnlinkApp,
}: {
  plans: UsageCyclePlan[];
  activePlan: UsageCyclePlan;
  window: CycleWindow;
  offset: number;
  summary: UsageSummary | undefined;
  linkedAppName?: string;
  onSelectPlan: (id: string) => void;
  onPrev: () => void;
  onNext: () => void;
  onResetOffset: () => void;
  onEditPlan: (planId: string) => void;
  onNewPlan: () => void;
  onUnlinkApp?: () => void;
}) {
  const pace = computeCyclePace(activePlan, summary, window, offset);
  const statusLabel =
    pace.status === "over"
      ? "已超参考额度"
      : pace.status === "fast"
        ? `消耗偏快 (+${pace.diffPct.toFixed(1)}%)`
        : pace.status === "healthy"
          ? `节奏健康 (${pace.diffPct >= 0 ? "+" : ""}${pace.diffPct.toFixed(1)}%)`
          : pace.status === "completed"
            ? "周期已结束"
            : "时间进度追踪";

  const barPct = pace.hasQuota
    ? Math.min(100, Math.max(0, pace.usagePct))
    : Math.min(100, Math.max(0, pace.timePct));

  const timeMarkerPct = Math.min(99, Math.max(1, pace.timePct));

  return (
    <section
      className="usage-cycle-bar"
      aria-label="AI Plan 周期查询导航"
      data-cycle-navigator
    >
      {/* Row 1: Plan pills + Edit/Add */}
      <div className="usage-cycle-row usage-cycle-plans-row">
        <div className="usage-cycle-pills" role="tablist" aria-label="周期方案">
          {plans.map((p) => {
            const active = p.id === activePlan.id;
            return (
              <button
                key={p.id}
                type="button"
                role="tab"
                aria-selected={active}
                className={"usage-cycle-pill" + (active ? " active" : "")}
                onClick={() => onSelectPlan(p.id)}
              >
                <span>{p.name}</span>
              </button>
            );
          })}
        </div>
        <div className="usage-cycle-actions">
          {linkedAppName && onUnlinkApp && (
            <span className="usage-cycle-linked">
              已联动 {linkedAppName}
              <button type="button" onClick={onUnlinkApp}>
                查看全部
              </button>
            </span>
          )}
          <button
            type="button"
            className="button usage-cycle-btn"
            onClick={() => onEditPlan(activePlan.id)}
          >
            <Pencil size={12} />
            编辑周期
          </button>
          <button
            type="button"
            className="button primary usage-cycle-btn"
            onClick={onNewPlan}
          >
            <Plus size={13} />
            添加周期
          </button>
        </div>
      </div>

      {/* Row 2: Stepper + Boundary + Rule & Countdown */}
      <div className="usage-cycle-row usage-cycle-nav-row">
        <div className="usage-cycle-stepper-wrap">
          <div className="usage-cycle-stepper">
            <button
              type="button"
              onClick={onPrev}
              aria-label="上一周期"
              title="上一周期"
            >
              <ChevronLeft size={14} />
              上一周期
            </button>
            <span className="usage-cycle-window">
              <CalendarClock size={13} />
              <span className="tabular-nums">
                {formatCycleRange(window.start, window.end)}
              </span>
              <span
                className={
                  "usage-cycle-badge" + (offset === 0 ? " current" : "")
                }
              >
                {offset === 0 ? "当前周期" : `${Math.abs(offset)} 周期前`}
              </span>
            </span>
            <button
              type="button"
              disabled={offset >= 0}
              onClick={onNext}
              aria-label="下一周期"
              title="下一周期"
            >
              下一周期
              <ChevronRight size={14} />
            </button>
          </div>
          {offset !== 0 && (
            <button
              type="button"
              className="text-button usage-cycle-back"
              onClick={onResetOffset}
            >
              <RotateCcw size={12} />
              回到当前
            </button>
          )}
        </div>

        <div className="usage-cycle-meta-tags">
          <span className="usage-cycle-tag">{formatCycleRule(activePlan)}</span>
          <span className="usage-cycle-tag accent tabular-nums">
            {offset === 0
              ? activePlan.type === "rolling"
                ? `实时滑动 · 近 ${activePlan.rollingHours} 小时`
                : `距下次重置 ${formatCountdown(window.remainingMs)}`
              : "历史已完成周期"}
          </span>
        </div>
      </div>

      {/* Row 3: Pace & Quota Progress Strip */}
      <div className="usage-cycle-pace">
        <div className="usage-cycle-pace-head">
          <div className="usage-cycle-pace-left">
            <span className="muted">
              {pace.hasQuota ? "周期额度进度" : "周期时间进度"}
            </span>
            <strong className="tabular-nums">
              {pace.hasQuota
                ? `${formatQuotaValue(pace.usedValue, activePlan.quotaMetric)} / ${formatQuotaValue(pace.limitValue, activePlan.quotaMetric)} (${pace.usagePct.toFixed(1)}%)`
                : `本周期已过 ${pace.timePct.toFixed(1)}%`}
            </strong>
            <span className={`usage-cycle-verdict tone-${pace.status}`}>
              {statusLabel}
            </span>
          </div>
          <div className="usage-cycle-pace-right muted tabular-nums">
            {pace.hasQuota ? (
              pace.projectedValue !== null ? (
                <>
                  按当前速率预计本周期{" "}
                  <b>
                    {formatQuotaValue(
                      pace.projectedValue,
                      activePlan.quotaMetric,
                    )}
                  </b>
                  {pace.projectedValue > pace.limitValue ? "（超额风险）" : ""}
                </>
              ) : offset < 0 ? (
                <>
                  周期日均{" "}
                  {formatQuotaValue(pace.dailyAvgUsed, activePlan.quotaMetric)}
                </>
              ) : (
                <>周期刚启动，稍后生成速率预测</>
              )
            ) : (
              <>点击「编辑周期」可配置费用 / Token / 请求参考额度</>
            )}
          </div>
        </div>

        <div className="usage-cycle-track" aria-hidden="true">
          <div
            className={`usage-cycle-fill tone-${pace.status}`}
            style={{ width: `${barPct}%` }}
          />
          {offset === 0 && activePlan.type !== "rolling" && (
            <span
              className="usage-cycle-marker"
              style={{ left: `${timeMarkerPct}%` }}
              title={`当前时间已过 ${pace.timePct.toFixed(1)}%`}
            />
          )}
        </div>

        <div className="usage-cycle-pace-foot muted tabular-nums">
          <div className="usage-cycle-pace-legend">
            {pace.hasQuota && (
              <span>
                <i className={`dot tone-${pace.status}`} />
                用量已用 {pace.usagePct.toFixed(1)}%
              </span>
            )}
            {offset === 0 && activePlan.type !== "rolling" && (
              <span>
                <i className="tick" />
                时间已过 {pace.timePct.toFixed(1)}%
              </span>
            )}
          </div>
          {pace.hasQuota && offset === 0 && activePlan.type !== "rolling" && (
            <span>
              本周期日均{" "}
              {formatQuotaValue(pace.dailyAvgUsed, activePlan.quotaMetric)}
              {pace.dailyAvgRemaining !== null
                ? ` · 剩余日均可用 ${formatQuotaValue(pace.dailyAvgRemaining, activePlan.quotaMetric)}`
                : ""}
            </span>
          )}
        </div>
      </div>
    </section>
  );
}
