import { useMemo, useState } from "react";
import { Plus, Trash2 } from "lucide-react";
import { UsageDialog } from "./ui";
import {
  computeCycleWindow,
  formatCountdown,
  formatCycleRange,
  formatCycleRule,
  normalizeCyclePlan,
  WEEKDAY_LABELS,
  type CycleType,
  type QuotaMetric,
  type UsageCyclePlan,
} from "./cycles";

const CYCLE_TYPES: { key: CycleType; label: string }[] = [
  { key: "weekly", label: "每周固定重置" },
  { key: "rolling", label: "滑动窗口" },
  { key: "monthly", label: "每月固定日" },
  { key: "custom_days", label: "自定义天数" },
];

const ROLLING_PRESETS = [
  { hours: 5, label: "5 小时" },
  { hours: 24, label: "24 小时" },
  { hours: 168, label: "滚动 7 天" },
];

function createBlankDraft(): UsageCyclePlan {
  return normalizeCyclePlan({
    id: `cycle-${crypto.randomUUID()}`,
    name: "",
    type: "weekly",
    weekDay: 1,
    time: "08:00",
    rollingHours: 5,
    monthDay: 1,
    customDays: 14,
    appType: "",
    quotaMetric: "none",
    quotaLimit: 0,
  });
}

export function CycleConfigModal({
  plans,
  initialPlanId,
  sources,
  saving,
  saveError,
  onSave,
  onDelete,
  onClose,
}: {
  plans: UsageCyclePlan[];
  /** Pass a plan id to edit an existing plan, or null to start adding a new one. */
  initialPlanId: string | null;
  sources: { id: string; name: string }[];
  saving: boolean;
  saveError?: string;
  onSave: (plan: UsageCyclePlan) => void;
  onDelete: (id: string) => void;
  onClose: () => void;
}) {
  const [editingId, setEditingId] = useState<string | null>(initialPlanId);
  const [draft, setDraft] = useState<UsageCyclePlan>(() => {
    const existing = initialPlanId
      ? plans.find((p) => p.id === initialPlanId)
      : undefined;
    return existing ? { ...existing } : createBlankDraft();
  });
  const [error, setError] = useState("");

  const isEditingExisting =
    editingId !== null && plans.some((p) => p.id === editingId);
  const canDelete = isEditingExisting && plans.length > 1;

  const previewWindow = useMemo(
    () => computeCycleWindow(draft, new Date(), 0),
    [draft],
  );

  const selectExisting = (plan: UsageCyclePlan) => {
    setEditingId(plan.id);
    setDraft({ ...plan });
    setError("");
  };

  const startNew = () => {
    setEditingId(null);
    setDraft(createBlankDraft());
    setError("");
  };

  const update = (patch: Partial<UsageCyclePlan>) => {
    setError("");
    setDraft((prev) => ({ ...prev, ...patch }));
  };

  const handleSubmit = (e: React.FormEvent) => {
    e.preventDefault();
    const trimmed = draft.name.trim();
    if (!trimmed) {
      setError("请输入周期名称");
      return;
    }
    if (
      draft.quotaMetric !== "none" &&
      (!Number.isFinite(draft.quotaLimit) || draft.quotaLimit <= 0)
    ) {
      setError("请填写大于 0 的参考额度上限，或选择「不设参考额度」");
      return;
    }
    onSave(normalizeCyclePlan({ ...draft, name: trimmed }));
  };

  return (
    <UsageDialog
      title={isEditingExisting ? "编辑查询周期" : "添加查询周期"}
      close={onClose}
      wide
    >
      <div className="usage-cycle-manager">
        {/* Saved cycles switcher bar: switch any saved cycle to edit, or add a new one */}
        <div className="usage-cycle-saved-box">
          <div className="usage-cycle-saved-head">
            <strong>已保存周期 ({plans.length})</strong>
            <button
              type="button"
              className={
                "button usage-cycle-btn" +
                (!isEditingExisting ? " primary" : "")
              }
              onClick={startNew}
            >
              <Plus size={12} />
              新建周期
            </button>
          </div>
          <div className="usage-cycle-saved-list">
            {plans.map((item) => {
              const selected = editingId === item.id;
              return (
                <button
                  key={item.id}
                  type="button"
                  className={
                    "usage-cycle-saved-chip" + (selected ? " active" : "")
                  }
                  onClick={() => selectExisting(item)}
                >
                  <span>{item.name}</span>
                  <small>{formatCycleRule(item)}</small>
                </button>
              );
            })}
          </div>
        </div>

        <form className="usage-cycle-form" onSubmit={handleSubmit}>
          <div className="usage-cycle-grid">
            <label className="usage-cycle-field">
              <span>周期名称</span>
              <input
                type="text"
                value={draft.name}
                placeholder="例如：Claude Max 周额度、Codex 周周期"
                onChange={(e) => update({ name: e.target.value })}
              />
            </label>
            <label className="usage-cycle-field">
              <span>联动应用筛选（可选）</span>
              <select
                value={draft.appType}
                onChange={(e) => update({ appType: e.target.value })}
              >
                <option value="">不联动（统计全部应用）</option>
                {sources.map((src) => (
                  <option key={src.id} value={src.id}>
                    自动筛 {src.name}
                  </option>
                ))}
              </select>
            </label>
          </div>

          <div className="usage-cycle-field">
            <span>周期类型</span>
            <div className="segmented usage-cycle-type-tabs" role="radiogroup">
              {CYCLE_TYPES.map((item) => (
                <button
                  key={item.key}
                  type="button"
                  aria-pressed={draft.type === item.key}
                  onClick={() => update({ type: item.key })}
                >
                  {item.label}
                </button>
              ))}
            </div>
          </div>

          {draft.type === "weekly" && (
            <div className="usage-cycle-rule-box usage-cycle-grid">
              <label className="usage-cycle-field">
                <span>每周重置日</span>
                <select
                  value={draft.weekDay}
                  onChange={(e) => update({ weekDay: Number(e.target.value) })}
                >
                  {[1, 2, 3, 4, 5, 6, 0].map((d) => (
                    <option key={d} value={d}>
                      每{WEEKDAY_LABELS[d]}
                    </option>
                  ))}
                </select>
              </label>
              <label className="usage-cycle-field">
                <span>重置时刻（本机时区）</span>
                <input
                  type="time"
                  value={draft.time}
                  onChange={(e) => update({ time: e.target.value || "00:00" })}
                />
              </label>
            </div>
          )}

          {draft.type === "rolling" && (
            <div className="usage-cycle-rule-box usage-cycle-grid">
              <label className="usage-cycle-field">
                <span>滑动窗口长度（小时）</span>
                <input
                  type="number"
                  min={1}
                  max={720}
                  value={draft.rollingHours}
                  onChange={(e) =>
                    update({
                      rollingHours: Math.max(1, Number(e.target.value)),
                    })
                  }
                />
              </label>
              <div className="usage-cycle-field">
                <span>常用快速选择</span>
                <div className="usage-cycle-presets">
                  {ROLLING_PRESETS.map((p) => (
                    <button
                      key={p.hours}
                      type="button"
                      className={
                        "button" +
                        (draft.rollingHours === p.hours ? " primary" : "")
                      }
                      onClick={() => update({ rollingHours: p.hours })}
                    >
                      {p.label}
                    </button>
                  ))}
                </div>
              </div>
            </div>
          )}

          {draft.type === "monthly" && (
            <div className="usage-cycle-rule-box usage-cycle-grid">
              <label className="usage-cycle-field">
                <span>每月重置日（1 ~ 31）</span>
                <input
                  type="number"
                  min={1}
                  max={31}
                  value={draft.monthDay}
                  onChange={(e) =>
                    update({
                      monthDay: Math.min(
                        31,
                        Math.max(1, Number(e.target.value)),
                      ),
                    })
                  }
                />
              </label>
              <label className="usage-cycle-field">
                <span>重置时刻</span>
                <input
                  type="time"
                  value={draft.time}
                  onChange={(e) => update({ time: e.target.value || "00:00" })}
                />
              </label>
            </div>
          )}

          {draft.type === "custom_days" && (
            <div className="usage-cycle-rule-box usage-cycle-grid three">
              <label className="usage-cycle-field">
                <span>周期天数</span>
                <input
                  type="number"
                  min={1}
                  max={365}
                  value={draft.customDays}
                  onChange={(e) =>
                    update({
                      customDays: Math.min(
                        365,
                        Math.max(1, Number(e.target.value)),
                      ),
                    })
                  }
                />
              </label>
              <label className="usage-cycle-field">
                <span>起始锚点日期</span>
                <input
                  type="date"
                  value={draft.anchorDate}
                  onChange={(e) => update({ anchorDate: e.target.value })}
                />
              </label>
              <label className="usage-cycle-field">
                <span>重置时刻</span>
                <input
                  type="time"
                  value={draft.time}
                  onChange={(e) => update({ time: e.target.value || "00:00" })}
                />
              </label>
            </div>
          )}

          <div className="usage-cycle-grid">
            <label className="usage-cycle-field">
              <span>周期参考额度（可选，用于进度提醒）</span>
              <select
                value={draft.quotaMetric}
                onChange={(e) =>
                  update({ quotaMetric: e.target.value as QuotaMetric })
                }
              >
                <option value="none">不设参考额度（仅按周期查询）</option>
                <option value="cost">按估算费用 (USD)</option>
                <option value="tokens">按总 Token (M)</option>
                <option value="requests">按请求次数</option>
              </select>
            </label>
            {draft.quotaMetric !== "none" && (
              <label className="usage-cycle-field">
                <span>
                  {draft.quotaMetric === "cost"
                    ? "参考额度上限 (USD)"
                    : draft.quotaMetric === "tokens"
                      ? "参考额度上限 (M Token)"
                      : "参考额度上限 (次)"}
                </span>
                <input
                  type="number"
                  min={0.01}
                  step="any"
                  value={draft.quotaLimit || ""}
                  placeholder={
                    draft.quotaMetric === "cost"
                      ? "例如 200"
                      : draft.quotaMetric === "tokens"
                        ? "例如 150"
                        : "例如 1000"
                  }
                  onChange={(e) =>
                    update({ quotaLimit: Number(e.target.value) || 0 })
                  }
                />
              </label>
            )}
          </div>

          <div className="usage-cycle-preview">
            <span>当前周期预览</span>
            <strong className="tabular-nums">
              {formatCycleRange(previewWindow.start, previewWindow.end)}
            </strong>
            {draft.type !== "rolling" && (
              <small className="tabular-nums">
                距重置 {formatCountdown(previewWindow.remainingMs)}
              </small>
            )}
          </div>

          {(error || saveError) && (
            <div className="usage-error">{error || saveError}</div>
          )}

          <div className="usage-cycle-modal-foot">
            {canDelete && editingId ? (
              <button
                type="button"
                className="button usage-cycle-danger"
                disabled={saving}
                onClick={() => onDelete(editingId)}
              >
                <Trash2 size={13} />
                删除此周期
              </button>
            ) : (
              <span />
            )}
            <div className="filters">
              <button type="button" className="button" onClick={onClose}>
                取消
              </button>
              <button
                type="submit"
                className="button primary"
                disabled={saving}
              >
                {saving
                  ? "保存中…"
                  : isEditingExisting
                    ? "保存修改"
                    : "添加并应用"}
              </button>
            </div>
          </div>
        </form>
      </div>
    </UsageDialog>
  );
}
