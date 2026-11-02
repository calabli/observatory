import { useEffect, useMemo, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import {
  CalendarClock,
  Plus,
  RefreshCw,
  Settings2,
  Trash2,
} from "lucide-react";
import { parseUsageError, usageCall } from "../native/usageClient";
import { AppGlyph } from "../usage/AppGlyph";
import {
  UsageDialog,
  formatMoney,
  formatTokens,
  friendlyError,
} from "../usage/ui";
import type { NativeStatus } from "../usage/types";
import {
  activeSubscription,
  coveringLocalDates,
  currentBillingPeriod,
  billingQuotaWindows,
  clampCycleToWindow,
  cycleOffsetRange,
  cycleOverlapsWindow,
  localDate,
  weeklyCycle,
  type WeeklyRule,
} from "./cycles";
import { formatSubscriptionPrice } from "./format";
import { aggregateIndependently } from "./aggregate";
import { filterAndSort } from "./list";
import "./subscriptions.css";

type Subscription = {
  id: string;
  name: string;
  appType: string;
  price: number;
  currency: string;
  billing: string;
  startDate: string;
  endDate: string;
  enabled: boolean;
  rule: WeeklyRule;
  /** 自定义账期天数，仅 `billing === "期"` 时有意义。 */
  customIntervalDays?: number | null;
};
type Summary = {
  requests: number;
  totalTokens: number;
  inputTokens: number;
  outputTokens: number;
  cacheReadTokens: number;
  cacheWriteTokens: number;
  costUsd: number;
  lastScanAt: number | null;
  daily: {
    date: string;
    totalTokens: number;
    costUsd: number;
    requests: number;
  }[];
};
const weekdays = ["周日", "周一", "周二", "周三", "周四", "周五", "周六"];
const defaultRule: WeeklyRule = {
  startDay: 4,
  startTime: "19:00:00",
  endDay: 4,
  endTime: "19:00:00",
};
/** 趋势图最多展示的日期数。请求时也按这个数量裁剪，避免多传 366 天。 */
const CHART_DAYS = 30;
/** 界面偏好保存在本机，重载后回到上次的位置。 */
const UI_KEY = "observatory.subscriptions.ui";
type UiState = {
  selected?: string;
  tab?: "usage" | "manage";
  filter?: "active" | "all";
  mode?: "week" | "days" | "billing";
  days?: number;
  dayPreset?: string;
  customDays?: string;
  chartMetric?: "tokens" | "cost" | "requests";
};
function readUi(): UiState {
  try {
    const raw = localStorage.getItem(UI_KEY);
    return raw ? (JSON.parse(raw) as UiState) : {};
  } catch {
    return {};
  }
}
function writeUi(state: UiState) {
  try {
    localStorage.setItem(UI_KEY, JSON.stringify(state));
  } catch {
    // 隐私模式或存储写满时写不进去，不影响使用。
  }
}
const display = (d: Date) =>
  d.toLocaleString("zh-CN", {
    year: "numeric",
    month: "2-digit",
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit",
    hour12: false,
  });
const iso = (d: Date) =>
  new Date(Math.floor(d.getTime() / 1000) * 1000)
    .toISOString()
    .replace(".000Z", "Z");
const blank = (): Subscription => ({
  id: crypto.randomUUID(),
  name: "",
  appType: "claude",
  price: 0,
  currency: "USD",
  billing: "月",
  startDate: localDate(),
  endDate: "",
  enabled: true,
  rule: { ...defaultRule },
  customIntervalDays: null,
});
function ClockSelect({
  value,
  onChange,
  label,
}: {
  value: string;
  onChange: (v: string) => void;
  label: string;
}) {
  const parts = value.split(":");
  return (
    <div className="sp-clock" aria-label={label}>
      {[24, 60, 60].map((count, index) => (
        <select
          key={index}
          aria-label={`${label}${["小时", "分钟", "秒"][index]}`}
          value={parts[index] ?? "00"}
          onChange={(e) => {
            const next = [parts[0] ?? "00", parts[1] ?? "00", parts[2] ?? "00"];
            next[index] = e.target.value;
            onChange(next.join(":"));
          }}
        >
          {Array.from({ length: count }, (_, i) =>
            String(i).padStart(2, "0"),
          ).map((v) => (
            <option key={v}>{v}</option>
          ))}
        </select>
      ))}
    </div>
  );
}
export function SubscriptionWorkspace() {
  const qc = useQueryClient();
  const initial = useMemo(() => readUi(), []);
  const records = useQuery({
    queryKey: ["subscriptions"],
    queryFn: () => usageCall<Subscription[]>("subscriptions_get"),
  });
  const status = useQuery({
    queryKey: ["usage", "status"],
    queryFn: () => usageCall<NativeStatus>("status"),
  });
  const revision = useQuery({
    queryKey: ["usage", "scan-revision"],
    queryFn: () => usageCall<{ revision: number }>("scan_revision"),
    refetchInterval: 2000,
    refetchIntervalInBackground: true,
    retry: false,
  });
  const save = useMutation({
    mutationFn: (items: Subscription[]) =>
      usageCall("subscriptions_save", { subscriptions: items }),
    onSuccess: () => qc.invalidateQueries({ queryKey: ["subscriptions"] }),
  });
  const [selected, setSelected] = useState(initial.selected ?? "");
  const [tab, setTab] = useState<"usage" | "manage">(initial.tab ?? "usage");
  const [filter, setFilter] = useState<"active" | "all">(
    initial.filter ?? "active",
  );
  const [form, setForm] = useState<Subscription | null>(null);
  const [formError, setFormError] = useState("");
  const [mode, setMode] = useState<"week" | "days" | "billing">(initial.mode ?? "week");
  const [chartMetric, setChartMetric] = useState<
    "tokens" | "cost" | "requests"
  >(initial.chartMetric ?? "tokens");
  const [days, setDays] = useState(initial.days ?? 7);
  const [dayPreset, setDayPreset] = useState(initial.dayPreset ?? "7");
  const [customDays, setCustomDays] = useState(initial.customDays ?? "14");
  const validDays =
    dayPreset !== "custom" ||
    (/^\d+$/.test(customDays) &&
      Number(customDays) >= 1 &&
      Number(customDays) <= 366);
  const [offset, setOffset] = useState(0);
  const [deviceScope, setDeviceScope] = useState<"local" | "all">("local");
  const [now, setNow] = useState(() => Date.now());
  // 订阅管理列表的检索与排序。
  const [query, setQuery] = useState("");
  const [sort, setSort] = useState<"start" | "name" | "price">("start");
  // 精度不足时改用完整日期范围：把被截断的日期整日计入。
  const [dateOverride, setDateOverride] = useState<{
    startDate: string;
    endDate: string;
  } | null>(null);
  useEffect(() => {
    const timer = setInterval(() => setNow(Date.now()), 30000);
    return () => clearInterval(timer);
  }, []);
  useEffect(() => {
    writeUi({
      selected,
      tab,
      filter,
      mode,
      days,
      dayPreset,
      customDays,
      chartMetric,
    });
  }, [selected, tab, filter, mode, days, dayPreset, customDays, chartMetric]);
  const items = useMemo(() => records.data ?? [], [records.data]);
  const active = useMemo(
    () => items.filter((s) => activeSubscription(s)),
    [items],
  );
  const item = items.find((s) => s.id === selected) ?? active[0] ?? null;
  // Keep active subscriptions first, while allowing disabled and expired plans
  // to be selected directly for historical usage.
  const selectable = useMemo(() => [...active, ...items.filter(s => !active.some(a => a.id === s.id))], [active, items]);
  // 选中的订阅只要还在列表里就保留：未生效或已停用的订阅也能查看历史用量。
  useEffect(() => {
    if (selected && items.some((s) => s.id === selected)) return;
    setSelected(active[0]?.id ?? items[0]?.id ?? "");
  }, [items, active, selected]);
  // 切换订阅、周期或统计方式后，完整日期范围的覆盖就失效了。
  useEffect(() => {
    setDateOverride(null);
  }, [selected, offset, mode]);
  const computed = useMemo(() => {
    if (!item) return null;
    let period;
    try {
      period = weeklyCycle(item.rule, new Date(now), offset);
    } catch (e) {
      return {
        period: null,
        next: null,
        error: friendlyError(e),
        nextError: "",
      };
    }
    try {
      return {
        period,
        next: weeklyCycle(item.rule, new Date(now), 1),
        error: "",
        nextError: "",
      };
    } catch (e) {
      return { period, next: null, error: "", nextError: friendlyError(e) };
    }
  }, [item, offset, now]);
  const period = computed?.period;
  // 周期导航限制在订阅有效期内：与有效期没有交集的周期不覆盖任何额度。
  const cycleBounds = useMemo(() => {
    if (!item) return { min: 0, max: 0 };
    return cycleOffsetRange(
      item.rule,
      { startDate: item.startDate, endDate: item.endDate },
      new Date(now),
    );
  }, [item, now]);
  // 下拉里只列有效期内的周期，另外保留当前周期——它可能是正在进行的跨界周期。
  const cycleOptions = useMemo(() => {
    const list: number[] = [];
    const seen = new Set<number>();
    const push = (value: number) => {
      if (!seen.has(value)) {
        seen.add(value);
        list.push(value);
      }
    };
    for (
      let value = cycleBounds.max;
      value >= cycleBounds.min && list.length < 13;
      value--
    )
      push(value);
    push(0);
    push(offset);
    return list.sort((a, b) => b - a);
  }, [cycleBounds.min, cycleBounds.max, offset]);
  const billingPeriod = useMemo(() => {
    if (!item) return null;
    try { return currentBillingPeriod(item, new Date(now)); } catch { return null; }
  }, [item, now]);
  // 周期收敛到订阅有效期内：开头取较晚者，结尾取较早者。
  const validityWindow = useMemo(
    () =>
      item
        ? { startDate: item.startDate, endDate: item.endDate }
        : { startDate: "", endDate: "" },
    [item],
  );
  const effectivePeriod = useMemo(
    () =>
      period && cycleOverlapsWindow(period, validityWindow)
        ? clampCycleToWindow(period, validityWindow)
        : null,
    [period, validityWindow],
  );
  const effectiveNext = useMemo(
    () =>
      computed?.next && cycleOverlapsWindow(computed.next, validityWindow)
        ? clampCycleToWindow(computed.next, validityWindow)
        : null,
    [computed?.next, validityWindow],
  );
  const periodClamped =
    !!period &&
    !!effectivePeriod &&
    (period.start.getTime() !== effectivePeriod.start.getTime() ||
      period.end.getTime() !== effectivePeriod.end.getTime());
  // 收敛后的周期还没开始，就不用去查了。
  const future =
    mode === "week" &&
    !!effectivePeriod &&
    effectivePeriod.start.getTime() >= Math.floor(now / 1000) * 1000;
  const queryPeriod = mode === "billing" ? billingPeriod : effectivePeriod;
  const params = useMemo(() => {
    if (
      !item ||
      (mode === "week" && computed?.error) ||
      future ||
      (mode === "days" && !validDays)
    )
      return null;
    if (mode === "days")
      return { days, appType: item.appType, devices: deviceScope, dailyLimit: CHART_DAYS };
    if (!queryPeriod) return null;
    // 精度不足时改查完整日期：把被截断的那几天整日计入。
    if (dateOverride)
      return {
        startDate: dateOverride.startDate,
        endDate: dateOverride.endDate,
        appType: item.appType,
        devices: deviceScope,
        dailyLimit: CHART_DAYS,
      };
    return {
      startTime: iso(queryPeriod.start),
      endTime: iso(new Date(Math.min(queryPeriod.end.getTime(), now))),
      appType: item.appType,
      devices: deviceScope,
      dailyLimit: CHART_DAYS,
    };
  }, [
    item,
    mode,
    days,
    queryPeriod,
    deviceScope,
    computed?.error,
    future,
    now,
    validDays,
    dateOverride,
  ]);
  const data = useQuery({
    queryKey: ["subscriptions", "usage", params, revision.data?.revision],
    queryFn: () => usageCall<Summary>("subscription_usage", params),
    enabled: !!params,
    retry: false,
    refetchOnWindowFocus: true,
  });
  const billingWeeks = useMemo(() => {
    if (mode !== "billing" || !item || !billingPeriod) return { windows: [], error: "" };
    try { return { windows: billingQuotaWindows(item.rule, billingPeriod, new Date(now)), error: "" }; }
    catch (error) { return { windows: [], error: friendlyError(error) }; }
  }, [mode, item, billingPeriod, now]);
  const weeklyRows = useQuery({
    queryKey: ["subscriptions", "billing-weeks", item?.appType, deviceScope, billingWeeks.windows.map(w => [iso(w.start), iso(w.end)]), revision.data?.revision],
    enabled: mode === "billing" && !!item && !billingWeeks.error && billingWeeks.windows.length > 0,
    queryFn: async () => {
      if (!item) return [];
      return aggregateIndependently(billingWeeks.windows, (window) =>
        usageCall<Summary>("subscription_usage", {
          startTime: iso(window.start), endTime: iso(window.end), appType: item.appType,
          devices: deviceScope, dailyLimit: 0,
        }),
      );
    },
    retry: false,
  });
  // 精度错误里带着被截断的日期，界面要把它显示出来并给一条补救路径。
  const usageError = useMemo(
    () => (data.error ? parseUsageError(data.error) : null),
    [data.error],
  );
  const precisionDates = useMemo(() => {
    const details = usageError?.details;
    if (!details || !Array.isArray(details.dates)) return [] as string[];
    return details.dates
      .filter((value): value is string => typeof value === "string")
      .sort();
  }, [usageError]);
  function openNew() {
    setForm(blank());
    setFormError("");
  }
  function openEdit(s: Subscription) {
    setForm({ ...s, rule: { ...s.rule } });
    setFormError("");
  }
  function persist() {
    if (!form) return;
    const f = { ...form, name: form.name.trim() };
    const interval = f.customIntervalDays ?? null;
    const wantsCustom = f.billing === "期";
    if (
      !f.name ||
      f.name.length > 120 ||
      !f.startDate ||
      !f.endDate ||
      f.endDate <= f.startDate ||
      !Number.isFinite(f.price) ||
      f.price < 0
    ) {
      setFormError(
        "请填写订阅名称、非负金额及有效起止日期；结束日期须晚于开始日期。",
      );
      return;
    }
    if (wantsCustom && (!interval || interval < 1 || interval > 366)) {
      setFormError("自定义账期需要填写 1–366 之间的天数。");
      return;
    }
    // 没选自定义账期就不带这个字段，省得后端校验时对不上。
    const normalized = wantsCustom
      ? { ...f, customIntervalDays: interval }
      : { ...f, customIntervalDays: null };
    try {
      weeklyCycle(f.rule);
    } catch (e) {
      setFormError(friendlyError(e));
      return;
    }
    const updated = items.some((s) => s.id === f.id)
      ? items.map((s) => (s.id === f.id ? normalized : s))
      : [...items, normalized];
    save.mutate(updated, {
      onSuccess: () => {
        setSelected(f.id);
        setOffset(0);
        setForm(null);
      },
    });
  }
  function remove(s: Subscription) {
    if (window.confirm(`删除 ${s.name} 的订阅记录？本机用量不会被删除。`))
      save.mutate(items.filter((x) => x.id !== s.id));
  }
  /** 列表里直接启停，不必打开表单。 */
  function toggleEnabled(s: Subscription) {
    save.mutate(
      items.map((x) => (x.id === s.id ? { ...x, enabled: !x.enabled } : x)),
    );
  }
  const patch = (p: Partial<Subscription>) =>
    setForm((v) => (v ? { ...v, ...p } : v));
  const rule = (p: Partial<WeeklyRule>) =>
    setForm((v) => (v ? { ...v, rule: { ...v.rule, ...p } } : v));
  const shown = useMemo(
    () =>
      filterAndSort(items, new Set(active.map((s) => s.id)), {
        filter,
        query,
        sort,
      }),
    [items, active, filter, query, sort],
  );
  const chart = (data.data?.daily ?? []).slice(-CHART_DAYS);
  const chartValue = (day: Summary["daily"][number]) =>
    chartMetric === "tokens"
      ? day.totalTokens
      : chartMetric === "cost"
        ? day.costUsd
        : day.requests;
  const max = Math.max(0.000001, ...chart.map(chartValue));
  const chartText = (value: number) =>
    chartMetric === "tokens"
      ? `${value.toLocaleString()} Token`
      : chartMetric === "cost"
        ? formatMoney(value, 4)
        : `${value.toLocaleString()} 次`;
  const firstDay = new Date(now);
  firstDay.setHours(0, 0, 0, 0);
  firstDay.setDate(firstDay.getDate() - days + 1);
  return (
    <div className="sp-page">
      {(records.error || status.error || save.error) && (
        <div className="usage-error" role="alert">
          {friendlyError(records.error || status.error || save.error)}
        </div>
      )}
      <div className="sp-tabs">
        <button aria-pressed={tab === "usage"} onClick={() => setTab("usage")}>
          周期用量
        </button>
        <button
          aria-pressed={tab === "manage"}
          onClick={() => setTab("manage")}
        >
          订阅管理 <span>{items.length}</span>
        </button>
        <button className="button primary sp-add" onClick={openNew}>
          <Plus size={14} />
          添加订阅
        </button>
      </div>
      {tab === "usage" ? (
        <>
          {records.isLoading ? (
            <div className="empty">正在读取订阅…</div>
          ) : !selectable.length ? (
            <div className="empty">
              <CalendarClock size={24} />
              <p>暂无生效中的订阅</p>
              <button className="button primary" onClick={openNew}>
                添加订阅
              </button>
              <button
                className="text-button"
                onClick={() => {
                  setFilter("all");
                  setTab("manage");
                }}
              >
                查看全部订阅
              </button>
            </div>
          ) : (
            <div className="sp-layout">
              <main>
                <div className="sp-section-title">
                  <div>
                    <h2>周期用量</h2>
                    <p>按周额度、本账期或最近天数查看所选 Agent 的用量。</p>
                  </div>
                  <button
                    className="button"
                    disabled={!params || data.isFetching}
                    onClick={() => void data.refetch()}
                  >
                    <RefreshCw size={14} />
                    {data.isFetching ? "查询中…" : "查询用量"}
                  </button>
                </div>
                <div
                  className="sp-agents"
                  role="group"
                  aria-label="选择当前订阅"
                >
                  {selectable.map((s) => (
                    <button
                      key={s.id}
                      className={
                        "sp-agent " + (item?.id === s.id ? "selected" : "")
                      }
                      onClick={() => {
                        setSelected(s.id);
                        setOffset(0);
                      }}
                    >
                      <AppGlyph app={s.appType} />
                      <span>
                        <strong>{s.name}</strong>
                        <small>
                          {status.data?.sources.find((a) => a.id === s.appType)
                            ?.name ?? s.appType}
                          {!s.enabled ? " · 已停用" : !activeSubscription(s) ? " · 未生效或已结束" : ""}
                        </small>
                      </span>
                    </button>
                  ))}
                </div>
                <div className="sp-controls">
                  <span className="muted">统计方式</span>
                  <div className="sp-mode">
                    <button
                      aria-pressed={mode === "week"}
                      onClick={() => setMode("week")}
                    >
                      周额度周期
                    </button>
                    <button
                      aria-pressed={mode === "billing"}
                      onClick={() => setMode("billing")}
                    >
                      本账期
                    </button>
                    <button
                      aria-pressed={mode === "days"}
                      onClick={() => setMode("days")}
                    >
                      最近天数
                    </button>
                  </div>
                  <select aria-label="统计设备" value={deviceScope} onChange={(e) => setDeviceScope(e.target.value as "local" | "all")}>
                    <option value="local">本机设备</option>
                    <option value="all">全部已配对设备</option>
                  </select>
                  {mode === "days" && (
                    <>
                      <select
                        aria-label="查询天数"
                        value={dayPreset}
                        onChange={(e) => {
                          const value = e.target.value;
                          setDayPreset(value);
                          if (value === "custom") {
                            const n = Number(customDays);
                            if (Number.isInteger(n) && n >= 1 && n <= 366)
                              setDays(n);
                          } else setDays(Number(value));
                        }}
                      >
                        <option value="1">今天</option>
                        <option value="7">最近 7 天</option>
                        <option value="30">最近 30 天</option>
                        <option value="90">最近 90 天</option>
                        <option value="custom">自定义天数</option>
                      </select>
                      {dayPreset === "custom" && (
                        <input
                          aria-label="自定义天数"
                          type="number"
                          min="1"
                          max="366"
                          value={customDays}
                          onChange={(e) => {
                            setCustomDays(e.target.value);
                            const n = Number(e.target.value);
                            if (Number.isInteger(n) && n >= 1 && n <= 366)
                              setDays(n);
                          }}
                        />
                      )}
                    </>
                  )}
                </div>
                {mode === "billing" && (
                  <div className="usage-note">
                    {billingPeriod ? `${now >= billingPeriod.end.getTime() ? "最后账期" : "本账期"}：${display(billingPeriod.start)} — ${display(billingPeriod.end)}（结束不含${now < billingPeriod.end.getTime() ? "；统计截至现在" : ""}）` : "订阅尚未开始或账期设置无效，无法查询账期。"}
                  </div>
                )}
                {mode === "days" && !validDays && (
                  <div className="usage-error" role="alert">
                    请输入 1–366 的整数天数。
                  </div>
                )}
                {mode === "days" && validDays && (
                  <div className="usage-note">
                    {localDate(firstDay)} 至 {localDate(new Date(now))} ·
                    本机时区，包含今天
                  </div>
                )}
                {mode === "week" && (
                  <>
                    <div className="sp-period">
                      <div>
                        <small>
                          {offset === 0
                            ? "当前周期"
                            : offset < 0
                              ? "历史周期"
                              : "下一周期"}
                        </small>
                        <strong>
                          {effectivePeriod
                            ? `${display(effectivePeriod.start)} — ${display(
                                effectivePeriod.end,
                              )}`
                            : "—"}
                        </strong>
                        <small>
                          本机时区 · 开始包含，结束不包含
                          {offset === 0 && period && now >= period.end.getTime()
                            ? " · 当前不在周期内"
                            : offset === 0 &&
                                period &&
                                now >= period.start.getTime()
                              ? " · 已统计至当前时间"
                              : ""}
                        </small>
                        {periodClamped && period && (
                          <small>
                            周周期 {display(period.start)} —{" "}
                            {display(period.end)}，已按订阅有效期收敛
                          </small>
                        )}
                      </div>
                      <button
                        className="text-button"
                        onClick={() => item && openEdit(item)}
                      >
                        编辑周期
                      </button>
                    </div>
                    <div className="sp-cycle-nav">
                      <button
                        className="button"
                        disabled={offset - 1 < cycleBounds.min}
                        title={offset - 1 < cycleBounds.min ? "已是最早周期" : "上一周期"}
                        onClick={() => setOffset((n) => n - 1)}
                      >
                        ← 上一周期
                      </button>
                      <select
                        aria-label="选择查询周期"
                        value={offset}
                        onChange={(e) => setOffset(Number(e.target.value))}
                      >
                        {cycleOptions.map((n) => {
                          let label =
                            n === 0
                              ? "本周期"
                              : n === 1
                                ? "下一周期"
                                : n > 0
                                  ? `第 ${n} 周期`
                                  : `历史周期 ${-n}`;
                          try {
                            if (item) {
                              const candidate = weeklyCycle(
                                item.rule,
                                new Date(now),
                                n,
                              );
                              if (
                                !cycleOverlapsWindow(candidate, validityWindow)
                              )
                                label += " · 不在订阅有效期内";
                              else {
                                const range = clampCycleToWindow(
                                  candidate,
                                  validityWindow,
                                );
                                label += ` · ${display(range.start)} — ${display(range.end)}`;
                              }
                            }
                          } catch {
                            label += " · 该时间在本机时区不存在";
                          }
                          return (
                            <option value={n} key={n}>
                              {label}
                            </option>
                          );
                        })}
                      </select>
                      <button
                        className="button"
                        disabled={offset + 1 > cycleBounds.max}
                        title={offset + 1 > cycleBounds.max ? "已是可查询的最后周期" : "下一周期"}
                        onClick={() => setOffset((n) => n + 1)}
                      >
                        下一周期 →
                      </button>
                      {effectiveNext && offset + 1 <= cycleBounds.max && (
                        <small>
                          下一周期：{display(effectiveNext.start)} —{" "}
                          {display(effectiveNext.end)}
                        </small>
                      )}
                      {computed?.nextError && (
                        <small>下一周期无法计算：{computed.nextError}</small>
                      )}
                    </div>
                  </>
                )}
                {mode === "week" && periodClamped && item && (
                  <div className="usage-note">
                    周期已按订阅有效期（{item.startDate} 至 {item.endDate}
                    ，不含结束日）收敛，只统计订阅覆盖的那一段。
                  </div>
                )}
                {mode === "week" && computed?.error && (
                  <div className="usage-error" role="alert">
                    {computed.error}
                  </div>
                )}
                {future && (
                  <div className="usage-note">
                    {period && period.start.getTime() > now
                      ? "该周期尚未开始，暂无用量。"
                      : "周期刚开始，请稍后查询。"}
                  </div>
                )}
                {mode === "week" && period && !effectivePeriod && (
                  <div className="usage-note">
                    该周期不在订阅有效期内，请选择其他周期。
                  </div>
                )}
                {dateOverride && (
                  <div className="usage-note">
                    已改用完整日期范围 {dateOverride.startDate} 至{" "}
                    {dateOverride.endDate}
                    （含首尾两天）查询，结果可能包含原周期外的用量。{" "}
                    <button
                      className="text-button"
                      onClick={() => setDateOverride(null)}
                    >
                      回到周期查询
                    </button>
                  </div>
                )}
                {data.error && (
                  <div className="usage-error" role="alert">
                    <p>
                      查询失败：
                      {usageError?.message || friendlyError(data.error)}
                    </p>
                    {precisionDates.length > 0 && (
                      <>
                        <p>
                          受影响的日期：
                          {precisionDates.map((date) => (
                            <code key={date}>{date}</code>
                          ))}
                        </p>
                        <button
                          className="button"
                          onClick={() => {
                            if (!queryPeriod) return;
                            const end = new Date(
                              Math.floor(
                                Math.min(queryPeriod.end.getTime(), now) /
                                  1000,
                              ) * 1000,
                            );
                            setDateOverride(
                              coveringLocalDates(queryPeriod.start, end),
                            );
                          }}
                        >
                          改用完整日期范围查询
                        </button>
                      </>
                    )}
                  </div>
                )}
                <div className="sp-metrics">
                  {[
                    [
                      "总 Token",
                      data.data ? formatTokens(data.data.totalTokens) : "—",
                      "输入、输出与缓存合计",
                    ],
                    [
                      "估算 API 费用",
                      data.data ? formatMoney(data.data.costUsd) : "—",
                      "按已记录费用估算",
                    ],
                    [
                      "请求次数",
                      data.data ? data.data.requests.toLocaleString() : "—",
                      "所选范围内",
                    ],
                    [
                      "订阅费用",
                      item
                        ? formatSubscriptionPrice(
                            item.price,
                            item.currency,
                            item.billing,
                            item.customIntervalDays,
                          )
                        : "—",
                      "手动记录，不与 API 费用相加",
                    ],
                  ].map(([label, value, note]) => (
                    <div key={label}>
                      <small>{label}</small>
                      <strong>{value}</strong>
                      <small>{note}</small>
                    </div>
                  ))}
                </div>
                {mode === "billing" && (
                  <section className="panel sp-billing-weeks">
                    <div className="sp-section-title"><div><h2>账期内周额度</h2><p>按额度周期精确统计，跨账期的周期仅计入本账期部分</p></div></div>
                    {billingWeeks.error || weeklyRows.error ? (
                      <div className="usage-error" role="alert">周额度聚合失败：{billingWeeks.error || friendlyError(weeklyRows.error)}</div>
                    ) : weeklyRows.isFetching ? <p className="muted">正在汇总周额度…</p> : !weeklyRows.data?.length ? <p className="muted">当前账期暂无已开始的额度周期</p> : (
                      <div className="sp-billing-week-list">{weeklyRows.data.map(({ window: row, value: usage, error }) => {
                        const parsed = error ? parseUsageError(error) : null;
                        const dates = Array.isArray(parsed?.details?.dates) ? parsed.details.dates.filter((date): date is string => typeof date === "string") : [];
                        const reason = parsed?.code === "RANGE_PRECISION_UNAVAILABLE"
                          ? `历史数据仅有日汇总，无法精确拆分${dates.length ? `（${dates.join("、")}）` : ""}`
                          : parsed?.message || "该周期查询失败";
                        return <div className="sp-billing-week-row" key={row.cycleStart.toISOString()}>
                          <div><strong>{display(row.start)} — {display(row.end)}</strong><small>{row.start.getTime() !== row.cycleStart.getTime() || row.end.getTime() !== row.cycleEnd.getTime() ? "账期内部分周期" : "完整额度周期"}</small>{error != null && <small role="alert">{reason}</small>}</div>
                          <div><strong>{usage ? formatTokens(usage.totalTokens) : "—"}</strong><small>Token</small></div>
                          <div><strong>{usage ? formatMoney(usage.costUsd) : "—"}</strong><small>估算费用</small></div>
                          <div><strong>{usage ? usage.requests.toLocaleString() : "—"}</strong><small>请求</small></div>
                        </div>;
                      })}</div>
                    )}
                    <p className="hint">按当前订阅的额度规则统计；无法精确拆分的周期显示为“—”，其他周期仍可查看。规则存在空档时，周周期之和可能小于账期总量。远端数据以最近同步快照为准。</p>
                  </section>
                )}
                <section className="panel sp-chart">
                  <div className="sp-section-title">
                    <div>
                      <h2>用量趋势</h2>
                      <p>按天统计 · 最多显示最近 30 个日期</p>
                    </div>
                    <div className="sp-mode" role="group" aria-label="趋势指标">
                      {(
                        [
                          ["tokens", "Token"],
                          ["cost", "费用"],
                          ["requests", "请求"],
                        ] as const
                      ).map(([value, label]) => (
                        <button
                          key={value}
                          aria-pressed={chartMetric === value}
                          onClick={() => setChartMetric(value)}
                        >
                          {label}
                        </button>
                      ))}
                    </div>
                  </div>
                  {chart.length ? (
                    <div className="sp-chart-body">
                      <div className="sp-yaxis" aria-hidden="true">
                        {[1, 0.75, 0.5, 0.25, 0].map((ratio) => (
                          <span key={ratio}>{chartText(max * ratio)}</span>
                        ))}
                      </div>
                      <div className="sp-bars">
                        {chart.map((d, index) => (
                          <div
                            key={d.date}
                            className="sp-bar-col"
                            role="img"
                            aria-label={`${d.date}：${chartText(chartValue(d))}`}
                          >
                            <div
                              className="sp-bar-fill"
                              style={{
                                height: `${Math.max(0, (chartValue(d) / max) * 100)}%`,
                              }}
                            />
                            <span className="sp-bar-tip">
                              {d.date} · {chartText(chartValue(d))}
                            </span>
                            {(chart.length <= 10 ||
                              index % Math.ceil(chart.length / 7) === 0 ||
                              index === chart.length - 1) && (
                              <small>{d.date.slice(5)}</small>
                            )}
                          </div>
                        ))}
                      </div>
                    </div>
                  ) : (
                    <div className="empty">
                      {data.isFetching ? "正在查询…" : "暂无趋势数据"}
                    </div>
                  )}
                </section>
                <p className="hint">
                  {data.data?.lastScanAt
                    ? `最近本机扫描：${new Date(data.data.lastScanAt).toLocaleString("zh-CN", { hour12: false })}。`
                    : "本次启动后尚无扫描时间。"}
                  统计所选 Agent 的{deviceScope === "all" ? "本机及已配对设备" : "本机"}用量；{mode === "week" ? "周周期按订阅有效期裁剪。" : mode === "billing" ? "账期按订阅设置及有效期计算。" : "最近天数不按订阅有效期裁剪。"}同一 Agent
                  的多个订阅暂不能按账号区分；官方剩余额度和百分比请以服务商页面为准。
                </p>
              </main>
              <aside>
                <section className="panel sp-side">
                  <h2>周期设置</h2>
                  <dl>
                    <div>
                      <dt>关联 Agent</dt>
                      <dd>
                        {status.data?.sources.find(
                          (a) => a.id === item?.appType,
                        )?.name ?? item?.appType}
                      </dd>
                    </div>
                    <div>
                      <dt>周期开始</dt>
                      <dd>
                        {item && weekdays[item.rule.startDay]}{" "}
                        {item?.rule.startTime}
                      </dd>
                    </div>
                    <div>
                      <dt>周期结束</dt>
                      <dd>
                        {item && weekdays[item.rule.endDay]}{" "}
                        {item?.rule.endTime}
                      </dd>
                    </div>
                    <div>
                      <dt>时区</dt>
                      <dd>本机时区</dd>
                    </div>
                  </dl>
                  <button
                    className="button"
                    onClick={() => item && openEdit(item)}
                  >
                    <Settings2 size={14} />
                    编辑周期
                  </button>
                </section>
                <section className="panel sp-side">
                  <h2>当前订阅</h2>
                  <strong>{item?.name}</strong>
                  <p>
                    有效期 {item?.startDate} 至 {item?.endDate}（不含结束日）
                  </p>
                  <p>
                    {item
                      ? formatSubscriptionPrice(
                          item.price,
                          item.currency,
                          item.billing,
                          item.customIntervalDays,
                        )
                      : "—"}
                  </p>
                  <button
                    className="text-button"
                    onClick={() => setTab("manage")}
                  >
                    管理订阅 →
                  </button>
                </section>
              </aside>
            </div>
          )}
        </>
      ) : (
        <>
          <div className="sp-section-title">
            <div>
              <h2>订阅管理</h2>
              <p>查看当前生效的订阅，维护计划、费用和有效期。</p>
            </div>
            <button className="button" onClick={openNew}>
              <Plus size={14} />
              添加订阅
            </button>
          </div>
          <div className="sp-controls">
            <button
              className="button"
              aria-pressed={filter === "active"}
              onClick={() => setFilter("active")}
            >
              当前生效
            </button>
            <button
              className="button"
              aria-pressed={filter === "all"}
              onClick={() => setFilter("all")}
            >
              全部订阅
            </button>
            <input
              type="search"
              aria-label="搜索订阅"
              placeholder="搜索名称或 Agent"
              value={query}
              onChange={(e) => setQuery(e.target.value)}
            />
            <select
              aria-label="排序方式"
              value={sort}
              onChange={(e) => setSort(e.target.value as typeof sort)}
            >
              <option value="start">按开始日期</option>
              <option value="name">按名称</option>
              <option value="price">按金额从高到低</option>
            </select>
            <span className="muted">
              当前生效 {active.length} 项 · 共 {items.length} 项
            </span>
          </div>
          <div className="panel sp-list">
            {shown.length ? (
              shown.map((s) => (
                <div className="sp-list-row" key={s.id}>
                  <AppGlyph app={s.appType} />
                  <div>
                    <strong>{s.name}</strong>
                    <small>
                      {status.data?.sources.find((a) => a.id === s.appType)
                        ?.name ?? s.appType}{" "}
                      · {s.startDate} — {s.endDate}
                    </small>
                  </div>
                  <span>
                    {formatSubscriptionPrice(
                      s.price,
                      s.currency,
                      s.billing,
                      s.customIntervalDays,
                    )}
                  </span>
                  <span>
                    {activeSubscription(s)
                      ? "生效中"
                      : s.enabled
                        ? "未生效或已结束"
                        : "已停用"}
                  </span>
                  <button
                    className="text-button"
                    aria-pressed={s.enabled}
                    disabled={save.isPending}
                    onClick={() => toggleEnabled(s)}
                  >
                    {s.enabled ? "停用" : "启用"}
                  </button>
                  <button
                    className="text-button"
                    onClick={() => {
                      setSelected(s.id);
                      setOffset(0);
                      setTab("usage");
                    }}
                  >
                    查看用量
                  </button>
                  <button className="text-button" onClick={() => openEdit(s)}>
                    编辑
                  </button>
                  <button
                    className="icon-button"
                    aria-label={`删除 ${s.name}`}
                    disabled={save.isPending}
                    onClick={() => remove(s)}
                  >
                    <Trash2 size={14} />
                  </button>
                </div>
              ))
            ) : (
              <div className="empty">没有订阅记录</div>
            )}
          </div>
        </>
      )}
      {form && (
        <UsageDialog
          title={items.some((s) => s.id === form.id) ? "编辑订阅" : "添加订阅"}
          close={() => setForm(null)}
          wide
        >
          <div className="sp-form">
            <label>
              订阅名称
              <input
                value={form.name}
                maxLength={120}
                onChange={(e) => patch({ name: e.target.value })}
                placeholder="例如 Claude Max"
              />
            </label>
            <label>
              关联 Agent
              <select
                value={form.appType}
                onChange={(e) => patch({ appType: e.target.value })}
              >
                {status.data?.sources.map((a) => (
                  <option key={a.id} value={a.id}>
                    {a.name}
                  </option>
                ))}
              </select>
            </label>
            <label>
              订阅金额
              <input
                type="number"
                min="0"
                step="0.01"
                value={Number.isFinite(form.price) ? form.price : ""}
                onChange={(e) => patch({ price: e.target.valueAsNumber })}
              />
            </label>
            <label>
              币种
              <select
                value={form.currency}
                onChange={(e) => patch({ currency: e.target.value })}
              >
                <option>USD</option>
                <option>CNY</option>
                <option>EUR</option>
              </select>
            </label>
            <label>
              账期
              <select
                value={form.billing}
                onChange={(e) =>
                  patch({
                    billing: e.target.value,
                    // 选回固定账期时清掉自定义天数，别留着无意义的值。
                    customIntervalDays:
                      e.target.value === "期"
                        ? (form.customIntervalDays ?? 30)
                        : null,
                  })
                }
              >
                <option value="月">每月</option>
                <option value="年">每年</option>
                <option value="期">自定义</option>
              </select>
            </label>
            {form.billing === "期" && (
              <label>
                账期天数
                <input
                  type="number"
                  min={1}
                  max={366}
                  value={form.customIntervalDays ?? 30}
                  onChange={(e) =>
                    patch({ customIntervalDays: e.target.valueAsNumber })
                  }
                />
              </label>
            )}
            <label>
              状态
              <select
                value={String(form.enabled)}
                onChange={(e) => patch({ enabled: e.target.value === "true" })}
              >
                <option value="true">启用</option>
                <option value="false">停用</option>
              </select>
            </label>
            <label>
              开始日期
              <input
                type="date"
                value={form.startDate}
                onChange={(e) => patch({ startDate: e.target.value })}
              />
            </label>
            <label>
              结束日期（不含当日）
              <input
                type="date"
                value={form.endDate}
                onChange={(e) => patch({ endDate: e.target.value })}
              />
            </label>
            <label>
              每周开始
              <select
                value={form.rule.startDay}
                onChange={(e) => rule({ startDay: Number(e.target.value) })}
              >
                {weekdays.map((day, i) => (
                  <option key={day} value={i}>
                    {day}
                  </option>
                ))}
              </select>
            </label>
            <label>
              开始时间
              <ClockSelect
                label="开始时间"
                value={form.rule.startTime}
                onChange={(v) => rule({ startTime: v })}
              />
            </label>
            <label>
              每周结束
              <select
                value={form.rule.endDay}
                onChange={(e) => rule({ endDay: Number(e.target.value) })}
              >
                {weekdays.map((day, i) => (
                  <option key={day} value={i}>
                    {day}
                  </option>
                ))}
              </select>
            </label>
            <label>
              结束时间
              <ClockSelect
                label="结束时间"
                value={form.rule.endTime}
                onChange={(v) => rule({ endTime: v })}
              />
            </label>
          </div>
          <p className="hint">
            周期按本机时区计算。订阅日期和金额由你手动维护；不会自动查询账单或官方额度。
          </p>
          {formError && (
            <p className="usage-error" role="alert">
              {formError}
            </p>
          )}
          {save.error && (
            <p className="usage-error" role="alert">
              保存失败：{friendlyError(save.error)}
            </p>
          )}
          <div className="sp-actions">
            <button className="button" onClick={() => setForm(null)}>
              取消
            </button>
            <button
              className="button primary"
              disabled={save.isPending || !status.data}
              onClick={persist}
            >
              保存
            </button>
          </div>
        </UsageDialog>
      )}
    </div>
  );
}
