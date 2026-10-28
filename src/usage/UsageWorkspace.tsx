import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import {
  Activity,
  ArrowLeft,
  ArrowRight,
  Boxes,
  Check,
  Database,
  Download,
  FileDown,
  Info,
  Laptop,
  Layers3,
  Monitor,
  RefreshCw,
  Search,
  Settings2,
  Tags,
  TriangleAlert,
} from "lucide-react";
import { usageCall } from "../native/usageClient";
import type { RequestLog, ModelStats, ProviderStats } from "../types/usage";
import { getFreshInputTokens, isUnpricedUsage } from "../types/usage";
import type { Dashboard, LanStatus, NativeStatus, UsageFilters } from "./types";
import {
  UsageDialog,
  formatMoney,
  formatTokens,
  friendlyError,
  timeLabel,
} from "./ui";
import { PricingManager } from "./PricingManager";
import { DeviceManager } from "./DeviceManager";
import { syncModelsDevPricingOnStartup } from "../lib/modelsDevAutoSync";
import {
  isHourlyTrend,
  trendLabel,
  trendTokens,
  trendHitRate,
} from "./chartData";
import { AppGlyph } from "./AppGlyph";
import { AppSelect } from "./AppSelect";
import { formatRequestSpeed } from "./requestSpeed";

type Props = {
  interval: string;
  onRefreshReady?: (fn: () => Promise<void>) => void;
  onBusy?: (busy: boolean) => void;
  onStatus?: (status: NativeStatus) => void;
};
type Range = "today" | "week" | "month" | "all" | "custom";
const blankSummary = {
  totalRequests: 0,
  totalCost: "0",
  totalInputTokens: 0,
  totalOutputTokens: 0,
  totalCacheReadTokens: 0,
  totalCacheCreationTokens: 0,
  realTotalTokens: 0,
  cacheHitRate: 0,
  successRate: 0,
};
const saveView = (value: unknown) => {
  try {
    localStorage.setItem("observatory.usage.view", JSON.stringify(value));
  } catch {}
};
function loadView(): {
  scope?: string;
  range?: Range;
  app?: string;
  model?: string;
  provider?: string;
} {
  try {
    return JSON.parse(localStorage.getItem("observatory.usage.view") || "{}");
  } catch {
    return {};
  }
}
function bounds(range: Range, start: string, end: string) {
  if (range === "all") return {};
  const day = new Date();
  day.setHours(0, 0, 0, 0);
  const last = new Date(day);
  last.setDate(last.getDate() + 1);
  if (range === "week") day.setDate(day.getDate() - 6);
  if (range === "month") day.setDate(day.getDate() - 29);
  if (range === "custom") {
    const first = new Date(start + "T00:00:00");
    const final = new Date(end + "T23:59:59");
    return {
      startDate: Math.floor(first.getTime() / 1000),
      endDate: Math.floor(final.getTime() / 1000),
    };
  }
  return {
    startDate: Math.floor(day.getTime() / 1000),
    endDate: Math.floor(last.getTime() / 1000) - 1,
  };
}
const dateInput = (date = new Date()) =>
  [
    date.getFullYear(),
    String(date.getMonth() + 1).padStart(2, "0"),
    String(date.getDate()).padStart(2, "0"),
  ].join("-");
function metricValue(log: RequestLog) {
  const fresh = getFreshInputTokens(log);
  return (
    fresh + log.outputTokens + log.cacheReadTokens + log.cacheCreationTokens
  );
}
function timeText(value: number) {
  return new Date(value * 1000).toLocaleString("zh-CN", {
    hour12: false,
    month: "2-digit",
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit",
  });
}
function exportPage(rows: RequestLog[]) {
  const fields = [
    "时间",
    "应用",
    "供应商",
    "模型",
    "新增输入 Token",
    "输出 Token",
    "缓存读取 Token",
    "缓存写入 Token",
    "费用 USD",
    "状态码",
    "数据来源",
  ];
  const quote = (v: unknown) => {
    let text = String(v ?? "");
    if (/^[=+@\t\r-]/.test(text)) text = "'" + text;
    return '"' + text.replace(/"/g, '""') + '"';
  };
  const lines = [
    fields,
    ...rows.map((r) => [
      new Date(r.createdAt * 1000).toISOString(),
      r.appType,
      r.providerName,
      r.model,
      getFreshInputTokens(r),
      r.outputTokens,
      r.cacheReadTokens,
      r.cacheCreationTokens,
      r.totalCostUsd,
      r.statusCode,
      r.dataSource,
    ]),
  ];
  const url = URL.createObjectURL(
    new Blob(
      ["\uFEFF" + lines.map((row) => row.map(quote).join(",")).join("\r\n")],
      { type: "text/csv;charset=utf-8" },
    ),
  );
  const a = document.createElement("a");
  a.href = url;
  a.download = `observatory-usage-${dateInput()}.csv`;
  a.click();
  setTimeout(() => URL.revokeObjectURL(url), 1000);
}

export function UsageWorkspace({
  interval,
  onRefreshReady,
  onBusy,
  onStatus,
}: Props) {
  const saved = useMemo(loadView, []);
  const qc = useQueryClient();
  const [scope, setScope] = useState(
    saved.scope === "multi" ? "multi" : "local",
  );
  const [range, setRange] = useState<Range>(saved.range ?? "today");
  const [app, setApp] = useState(saved.app ?? "");
  const [model, setModel] = useState(saved.model ?? "");
  const [provider, setProvider] = useState(saved.provider ?? "");
  const [customStart, setCustomStart] = useState(dateInput());
  const [customEnd, setCustomEnd] = useState(dateInput());
  const [page, setPage] = useState(1);
  const [size, setSize] = useState(20);
  const [statusCode, setStatusCode] = useState("");
  const [tab, setTab] = useState("logs");
  const [metric, setMetric] = useState("tokens");
  const [chartType, setChartType] = useState<"bar" | "area">("bar");
  const [modal, setModal] = useState<"pricing" | "sources" | "devices" | null>(
    null,
  );
  const [detail, setDetail] = useState<string | null>(null);
  const [notice, setNotice] = useState("");
  const [scanBusy, setScanBusy] = useState(false);
  const [scanErrors, setScanErrors] = useState<string[]>([]);
  const [importPath, setImportPath] = useState("");
  const scanRef = useRef<Promise<void> | null>(null);
  const mounted = useRef(true);
  const startup = useRef(false);
  const status = useQuery({
    queryKey: ["usage", "status"],
    queryFn: () => usageCall<NativeStatus>("status"),
    retry: false,
    staleTime: 10000,
  });
  const lan = useQuery({
    queryKey: ["usage", "lan"],
    queryFn: () => usageCall<LanStatus>("lan_status"),
    enabled: !!status.data,
    staleTime: 10000,
  });
  const period = useMemo(
    () => bounds(range, customStart, customEnd),
    [range, customStart, customEnd],
  );
  const validPeriod =
    range !== "custom" ||
    (customStart && customEnd && customStart <= customEnd);
  const filters: UsageFilters = {
    ...period,
    appType: app || undefined,
    model: model || undefined,
    providerName: provider || undefined,
    statusCode: statusCode ? Number(statusCode) : undefined,
    multiDevice: scope === "multi",
    page,
    pageSize: size,
  };
  const dashboard = useQuery({
    queryKey: ["usage", "dashboard", filters],
    queryFn: () => usageCall<Dashboard>("dashboard", filters),
    enabled: !!status.data && !!validPeriod,
    staleTime: 5000,
    refetchOnWindowFocus: false,
    retry: false,
  });
  const refresh = useCallback(
    async (automatic = false) => {
      if (scanRef.current) return scanRef.current;
      const task = (async () => {
        setScanBusy(true);
        onBusy?.(true);
        try {
          const r = await usageCall<{
            scan: { imported: number; errors: string[] };
            peers: { error?: string }[];
          }>("sync", { automatic });
          if (mounted.current) {
            setScanErrors([
              ...r.scan.errors,
              ...r.peers.flatMap((p) => (p.error ? [p.error] : [])),
            ]);
            setNotice(`本机扫描完成，新增或更新 ${r.scan.imported} 条记录。`);
          }
          await qc.invalidateQueries({ queryKey: ["usage"] });
        } catch (e) {
          if (mounted.current) setNotice("扫描失败：" + friendlyError(e));
          throw e;
        } finally {
          scanRef.current = null;
          if (mounted.current) setScanBusy(false);
          onBusy?.(false);
        }
      })();
      scanRef.current = task;
      return task;
    },
    [qc, onBusy],
  );
  useEffect(() => {
    mounted.current = true;
    onRefreshReady?.(refresh);
    return () => {
      mounted.current = false;
    };
  }, [onRefreshReady, refresh]);
  useEffect(() => {
    if (status.data) {
      onStatus?.(status.data);
      if (!importPath) setImportPath(status.data.ccSwitchPath);
    }
  }, [status.data, onStatus]);
  useEffect(() => {
    if (!status.data || startup.current) return;
    startup.current = true;
    if (status.data.initialImportNotice)
      setNotice(status.data.initialImportNotice);
    if (
      !status.data.initialImportError &&
      status.data.refreshInterval !== "manual"
    )
      void refresh(true).catch(() => {});
    void syncModelsDevPricingOnStartup()
      .then((r) => {
        if (!r.skipped) return qc.invalidateQueries({ queryKey: ["usage"] });
      })
      .catch((e) => {
        if (mounted.current)
          setNotice("价格自动同步未完成：" + friendlyError(e));
      });
  }, [status.data, refresh, qc]);
  useEffect(() => {
    if (!status.data || status.data.initialImportError || interval === "manual")
      return;
    const focus = () => {
      if (document.visibilityState === "visible")
        void refresh(true).catch(() => {});
    };
    window.addEventListener("focus", focus);
    const seconds = Number(interval);
    const timer =
      Number.isFinite(seconds) && seconds > 0
        ? setInterval(() => {
            if (document.visibilityState === "visible")
              void refresh(true).catch(() => {});
          }, seconds * 1000)
        : null;
    return () => {
      window.removeEventListener("focus", focus);
      if (timer) clearInterval(timer);
    };
  }, [
    interval,
    status.data?.device.deviceId,
    status.data?.initialImportError,
    refresh,
  ]);
  useEffect(
    () => saveView({ scope, range, app, provider, model }),
    [scope, range, app, provider, model],
  );
  const change = (fn: () => void) => {
    setPage(1);
    setDetail(null);
    fn();
  };
  const importHistory = useMutation({
    mutationFn: () =>
      usageCall<{
        requests: number;
        rollups: number;
        prices: number;
        warnings: string[];
      }>("import_cc_switch", { path: importPath }),
    onSuccess: async (r) => {
      setNotice(
        `已导入 ${r.requests} 条新明细，更新 ${r.rollups} 条日汇总。${r.warnings.join("；")}`,
      );
      await qc.invalidateQueries({ queryKey: ["usage"] });
    },
  });
  const s = dashboard.data?.summary;
  const logs = dashboard.data?.logs;
  const sources = status.data?.sources ?? [];
  const appName = (id: string) => sources.find((a) => a.id === id)?.name ?? id;
  return (
    <div className="native-usage" data-native-usage>
      <div className="tabs">
        <button
          aria-pressed={scope === "local"}
          onClick={() => change(() => setScope("local"))}
        >
          <Laptop size={14} />
          本机用量
        </button>
        <button
          aria-pressed={scope === "multi"}
          onClick={() => change(() => setScope("multi"))}
        >
          <Monitor size={14} />
          多设备汇总
          <span className="badge">
            {1 + (lan.data?.devices.filter((d) => d.enabled).length ?? 0)}
          </span>
        </button>
        <button
          className="usage-manage-device"
          onClick={() => setModal("devices")}
        >
          <Settings2 size={13} />
          管理设备
        </button>
      </div>
      <div className="filter-row">
        <div className="filters">
          <select
            aria-label="统计时间"
            value={range}
            onChange={(e) => change(() => setRange(e.target.value as Range))}
          >
            <option value="today">今天</option>
            <option value="week">最近 7 天</option>
            <option value="month">最近 30 天</option>
            <option value="all">全部时间</option>
            <option value="custom">自定义日期</option>
          </select>
          <AppSelect
            sources={sources}
            value={app}
            onChange={(value) =>
              change(() => {
                setApp(value);
                setProvider("");
                setModel("");
              })
            }
          />
          <select
            aria-label="统计供应商"
            value={provider}
            onChange={(e) => change(() => setProvider(e.target.value))}
          >
            <option value="">全部供应商</option>
            {dashboard.data?.options.providers.map((p) => (
              <option key={p} value={p}>
                {p}
              </option>
            ))}
          </select>
          <select
            aria-label="统计模型"
            value={model}
            onChange={(e) => change(() => setModel(e.target.value))}
          >
            <option value="">全部模型</option>
            {dashboard.data?.options.models.map((m) => (
              <option key={m} value={m}>
                {m}
              </option>
            ))}
          </select>
        </div>
        <div className="filters">
          <button className="button" onClick={() => setModal("pricing")}>
            <Tags size={13} />
            价格管理
          </button>
          <button className="button" onClick={() => setModal("sources")}>
            <Layers3 size={13} />
            数据来源
          </button>
          <button
            className="button primary"
            disabled={scanBusy || !status.data}
            onClick={() => void refresh().catch(() => {})}
          >
            <RefreshCw size={13} />
            {scanBusy ? "扫描中…" : "扫描更新"}
          </button>
        </div>
      </div>
      {range === "custom" && (
        <div className="usage-date-range">
          <label>
            开始日期
            <input
              type="date"
              aria-label="开始日期"
              value={customStart}
              onChange={(e) => change(() => setCustomStart(e.target.value))}
            />
          </label>
          <span>至</span>
          <label>
            结束日期
            <input
              type="date"
              aria-label="结束日期"
              value={customEnd}
              onChange={(e) => change(() => setCustomEnd(e.target.value))}
            />
          </label>
          {!validPeriod && (
            <span className="usage-error">开始日期不能晚于结束日期。</span>
          )}
        </div>
      )}
      <div className="usage-meta">
        <span className="inline">
          <span className="status-dot" />
          {status.data?.device.deviceName ?? "正在连接统计引擎"}
          {scope === "multi"
            ? " · 本机 + 已启用设备的最近成功快照"
            : " · 真实本地记录"}
        </span>
        <span>
          {scanBusy
            ? "正在读取本机日志…"
            : status.data?.lastScanAt
              ? "最近扫描 " +
                new Date(status.data.lastScanAt).toLocaleTimeString("zh-CN", {
                  hour12: false,
                })
              : "尚未完成本机扫描"}
        </span>
      </div>
      {(status.error || dashboard.error || importHistory.error) && (
        <div className="usage-error" role="alert">
          <TriangleAlert size={15} />
          {friendlyError(
            status.error || dashboard.error || importHistory.error,
          )}
          <button
            className="text-button"
            onClick={() => {
              void status.refetch();
              void dashboard.refetch();
            }}
          >
            重试
          </button>
        </div>
      )}
      {status.data?.initialImportError && (
        <div className="usage-error" role="alert">
          历史迁移未完成：{status.data.initialImportError}
        </div>
      )}
      {notice && (
        <div className="usage-note" role="status">
          {notice}
        </div>
      )}
      {scanErrors.length > 0 && (
        <details className="usage-error">
          <summary>
            {scanErrors.length} 项来源暂未完成扫描或同步，已有记录保留
          </summary>
          {scanErrors.map((e, i) => (
            <p key={i}>{e}</p>
          ))}
        </details>
      )}
      {status.data?.ccSwitchAvailable && status.data.detailRecords === 0 && (
        <div className="usage-import-callout">
          <FileDown size={22} />
          <div>
            <strong>发现 CC Switch 历史统计</strong>
            <p>
              只读导入明细、日汇总和去重游标，保留原始记录与费用，不带入密钥配置。
            </p>
          </div>
          <button
            className="button primary"
            disabled={importHistory.isPending || scanBusy}
            onClick={() => importHistory.mutate()}
          >
            {importHistory.isPending ? "导入中…" : "导入历史用量"}
          </button>
        </div>
      )}
      <div
        className={
          "stats-grid usage-metric-strip " +
          (dashboard.isFetching ? "usage-updating" : "")
        }
        aria-busy={dashboard.isFetching}
      >
        <Metric
          label="估算费用"
          value={s ? formatMoney(s.totalCost) : "—"}
          detail="USD · 不按接收端价格重算远端记录"
          icon={Tags}
          selected={metric === "cost"}
          onSelect={() => setMetric("cost")}
        />
        <Metric
          label="请求次数"
          value={s ? s.totalRequests.toLocaleString() : "—"}
          detail={s ? "成功率 " + s.successRate.toFixed(1) + "%" : "读取统计中"}
          icon={Activity}
          selected={metric === "requests"}
          onSelect={() => setMetric("requests")}
        />
        <Metric
          label="总 Token"
          value={s ? formatTokens(s.realTotalTokens) : "—"}
          detail={
            s
              ? "新增输入 " +
                formatTokens(s.totalInputTokens) +
                " · 输出 " +
                formatTokens(s.totalOutputTokens)
              : "含缓存读取与缓存写入"
          }
          icon={Layers3}
          selected={metric === "tokens"}
          onSelect={() => setMetric("tokens")}
        />
        <Metric
          label="缓存命中率"
          value={s ? (s.cacheHitRate * 100).toFixed(1) + "%" : "—"}
          detail={
            s
              ? "缓存读取 " +
                formatTokens(s.totalCacheReadTokens) +
                " · 写入 " +
                formatTokens(s.totalCacheCreationTokens)
              : "缓存读取 / 可缓存输入"
          }
          icon={Database}
          selected={metric === "hitRate"}
          onSelect={() => setMetric("hitRate")}
        />
      </div>
      <div className="usage-token-parts" aria-label="Token 构成">
        {[
          ["新增输入", s?.totalInputTokens, "var(--chart-usage)"],
          ["输出", s?.totalOutputTokens, "var(--chart-output)"],
          ["缓存写入", s?.totalCacheCreationTokens, "var(--agent-color)"],
          ["缓存读取", s?.totalCacheReadTokens, "var(--teal)"],
        ].map(([label, value, color]) => (
          <span key={String(label)}>
            <i style={{ background: String(color) }} />
            {label}
            <b>{typeof value === "number" ? formatTokens(value) : "—"}</b>
          </span>
        ))}
      </div>
      {dashboard.data && s && s.totalRequests === 0 && !scanBusy && (
        <div className="usage-empty">
          <Boxes size={27} />
          <strong>当前筛选下没有用量记录</strong>
          <p>
            更换时间或应用筛选，也可以扫描本机日志或导入已有 CC Switch 统计。
          </p>
          <div className="filters">
            <button
              className="button"
              onClick={() =>
                change(() => {
                  setRange("all");
                  setApp("");
                  setProvider("");
                  setModel("");
                })
              }
            >
              查看全部时间
            </button>
            <button className="button" onClick={() => setModal("sources")}>
              配置数据来源
            </button>
          </div>
        </div>
      )}
      <div className="usage-charts">
        <section className="panel">
          <div className="panel-head">
            <h2>用量趋势</h2>
            <div className="panel-actions usage-chart-controls">
              <div className="segmented" aria-label="用量趋势指标">
                {[
                  ["tokens", "Token"],
                  ["cost", "费用"],
                  ["requests", "请求"],
                  ["hitRate", "命中率"],
                ].map(([key, label]) => (
                  <button
                    key={key}
                    aria-pressed={metric === key}
                    onClick={() => setMetric(key)}
                  >
                    {label}
                  </button>
                ))}
              </div>
              <div className="segmented" aria-label="图表类型">
                <button
                  aria-pressed={chartType === "bar"}
                  onClick={() => setChartType("bar")}
                >
                  柱状
                </button>
                <button
                  aria-pressed={chartType === "area"}
                  onClick={() => setChartType("area")}
                >
                  折线
                </button>
              </div>
            </div>
          </div>
          <UsageChart
            data={dashboard.data?.trends ?? []}
            metric={metric}
            chartType={chartType}
          />
        </section>
        <section className="panel">
          <div className="panel-head">
            <h2>应用费用分布</h2>
            <span className="muted">USD</span>
          </div>
          <div className="native-app-costs">
            {dashboard.data?.byApp
              .filter((a) => !app || a.appType === app)
              .sort(
                (a, b) =>
                  Number(b.summary.totalCost) - Number(a.summary.totalCost),
              )
              .slice(0, 7)
              .map((a) => (
                <button
                  key={a.appType}
                  className="usage-app-cost"
                  onClick={() => change(() => setApp(a.appType))}
                >
                  <span>
                    <span className="usage-app-label">
                      <AppGlyph app={a.appType} />
                      {appName(a.appType)}
                    </span>
                    <b>{formatMoney(a.summary.totalCost)}</b>
                  </span>
                  <span className="meter">
                    <span
                      style={{
                        width:
                          Math.max(
                            1,
                            (Number(a.summary.totalCost) /
                              Math.max(0.000001, Number(s?.totalCost))) *
                              100,
                          ) + "%",
                      }}
                    />
                  </span>
                </button>
              ))}
          </div>
        </section>
      </div>
      <ActivityGrid
        data={dashboard.data?.trends ?? []}
        start={period.startDate}
        end={period.endDate}
      />
      <section className="panel section-gap">
        <div className="usage-section-title">
          <div className="tabs usage-detail-tabs">
            {[
              ["logs", "请求明细"],
              ["providers", "供应商"],
              ["models", "模型"],
            ].map(([key, label]) => (
              <button
                key={key}
                aria-pressed={tab === key}
                onClick={() => setTab(key)}
              >
                {label}
                <span className="badge">
                  {key === "logs"
                    ? (logs?.total ?? 0)
                    : key === "providers"
                      ? (dashboard.data?.providers.length ?? 0)
                      : (dashboard.data?.models.length ?? 0)}
                </span>
              </button>
            ))}
          </div>
          {tab === "logs" && (
            <div className="filters">
              <select
                aria-label="状态码筛选"
                value={statusCode}
                onChange={(e) => change(() => setStatusCode(e.target.value))}
              >
                <option value="">全部状态</option>
                {[200, 400, 401, 403, 429, 500, 502, 503].map((s) => (
                  <option key={s} value={s}>
                    {s}
                  </option>
                ))}
              </select>
              <button
                className="button"
                disabled={!logs?.data.length}
                onClick={() => exportPage(logs?.data ?? [])}
              >
                <Download size={13} />
                导出本页
              </button>
            </div>
          )}
        </div>
        {tab === "logs" ? (
          <>
            <div className="table-wrap">
              <table className="usage-request-table">
                <thead>
                  <tr>
                    <th>时间</th>
                    <th>应用 / 模型</th>
                    <th>供应商</th>
                    <th className="numeric">新增输入</th>
                    <th className="numeric">输出</th>
                    <th className="numeric">缓存读取</th>
                    <th className="numeric">费用 USD</th>
                    <th>耗时</th>
                    <th>输出速度</th>
                    <th>状态</th>
                    <th />
                  </tr>
                </thead>
                <tbody>
                  {logs?.data.map((r) => (
                    <tr
                      key={r.requestId}
                      onClick={() => setDetail(r.requestId)}
                    >
                      <td className="muted">{timeText(r.createdAt)}</td>
                      <td>
                        <span className="usage-log-identity">
                          <AppGlyph app={r.appType} />
                          <span>
                            {appName(r.appType)}
                            <small className="cell-sub">
                              {r.model || "unknown"}
                            </small>
                          </span>
                        </span>
                      </td>
                      <td>{r.providerName ?? r.providerId}</td>
                      <td className="numeric">
                        {formatTokens(getFreshInputTokens(r))}
                      </td>
                      <td className="numeric">
                        {formatTokens(r.outputTokens)}
                      </td>
                      <td className="numeric">
                        {formatTokens(r.cacheReadTokens)}
                      </td>
                      <td className="numeric">
                        {isUnpricedUsage(r) ? (
                          <span className="usage-unpriced">未计价</span>
                        ) : (
                          formatMoney(r.totalCostUsd, 4)
                        )}
                      </td>
                      <td>
                        {r.latencyMs
                          ? (r.dataSource !== "proxy" ? "≈ " : "") +
                            (r.latencyMs / 1000).toFixed(2) +
                            "s"
                          : "—"}
                      </td>
                      <td title="精确速度为输出 Token 除以生成耗时；≈ 表示按日志耗时估算，包含首字等待。">
                        {formatRequestSpeed(r)}
                      </td>
                      <td>
                        <span
                          className={
                            r.statusCode >= 200 && r.statusCode < 300
                              ? "positive"
                              : ""
                          }
                        >
                          {r.statusCode}
                        </span>
                      </td>
                      <td>
                        <button
                          className="icon-button"
                          aria-label={"查看请求 " + r.requestId}
                          onClick={(e) => {
                            e.stopPropagation();
                            setDetail(r.requestId);
                          }}
                        >
                          <ArrowRight size={12} />
                        </button>
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
            {!logs?.data.length && (
              <p className="empty">
                {dashboard.isLoading
                  ? "正在读取请求明细…"
                  : "没有匹配的明细记录；较早记录可能已按日汇总。"}
              </p>
            )}
            <div className="usage-pagination">
              <span>共 {logs?.total ?? 0} 条明细 · 费用以记录时的计价为准</span>
              <div className="filters">
                <select
                  aria-label="每页记录数"
                  value={size}
                  onChange={(e) =>
                    change(() => setSize(Number(e.target.value)))
                  }
                >
                  {[20, 50, 100, 200].map((n) => (
                    <option key={n} value={n}>
                      {n} 条 / 页
                    </option>
                  ))}
                </select>
                <button
                  className="icon-button"
                  disabled={page <= 1}
                  aria-label="上一页"
                  onClick={() => setPage(page - 1)}
                >
                  <ArrowLeft size={13} />
                </button>
                <span>
                  {page} / {Math.max(1, Math.ceil((logs?.total ?? 0) / size))}
                </span>
                <button
                  className="icon-button"
                  disabled={page * size >= (logs?.total ?? 0)}
                  aria-label="下一页"
                  onClick={() => setPage(page + 1)}
                >
                  <ArrowRight size={13} />
                </button>
              </div>
            </div>
          </>
        ) : (
          <StatsTable
            rows={
              tab === "models"
                ? (dashboard.data?.models ?? [])
                : (dashboard.data?.providers ?? [])
            }
            model={tab === "models"}
          />
        )}
      </section>
      {modal === "pricing" && <PricingManager close={() => setModal(null)} />}
      {modal === "devices" && (
        <UsageDialog title="多设备用量同步" close={() => setModal(null)} wide>
          <DeviceManager />
        </UsageDialog>
      )}
      {modal === "sources" && (
        <SourcesDialog
          close={() => setModal(null)}
          status={status.data}
          importPath={importPath}
          setImportPath={setImportPath}
          importBusy={importHistory.isPending}
          importHistory={() => importHistory.mutate()}
          importError={importHistory.error}
          importResult={
            importHistory.data
              ? `已导入 ${importHistory.data.requests} 条新明细，更新 ${importHistory.data.rollups} 条日汇总。${importHistory.data.warnings.join("；")}`
              : undefined
          }
        />
      )}
      {detail && (
        <RequestDetail
          id={detail}
          multi={scope === "multi"}
          close={() => setDetail(null)}
        />
      )}
    </div>
  );
}

function Metric({
  label,
  value,
  detail,
  icon: Icon,
  selected,
  onSelect,
}: {
  label: string;
  value: string;
  detail: string;
  icon: typeof Tags;
  selected: boolean;
  onSelect: () => void;
}) {
  return (
    <button
      type="button"
      className="stat"
      aria-pressed={selected}
      onClick={onSelect}
    >
      <div className="stat-label">
        {label}
        <Icon size={15} />
      </div>
      <div className="stat-value">{value}</div>
      <div className="stat-detail" title={detail}>
        {detail}
      </div>
    </button>
  );
}
function UsageChart({
  data,
  metric,
  chartType,
}: {
  data: Dashboard["trends"];
  metric: string;
  chartType: "bar" | "area";
}) {
  const [hover, setHover] = useState<number | null>(null);
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
            if (data.length)
              setHover(
                Math.max(
                  0,
                  Math.min(
                    data.length - 1,
                    chartType === "bar"
                      ? Math.floor(at * data.length)
                      : Math.round(at * (data.length - 1)),
                  ),
                ),
              );
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
                const width = Math.min(36, band * 0.7);
                const height = v > 0 ? Math.max(1, (v / max) * 150) : 0;
                return (
                  <rect
                    key={i}
                    x={(i + 0.5) * band - width / 2}
                    y={168 - height}
                    width={width}
                    height={height}
                    rx={2}
                    fill="var(--chart-usage)"
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
          </svg>
          {hover !== null && data[hover] && (
            <div
              className="chart-tooltip"
              style={{ left: "min(50%, calc(100% - 150px))" }}
            >
              <small>{data[hover].date}</small>
              {metric === "hitRate" && trendHitRate(data[hover]) === null
                ? "无可缓存输入"
                : label(values[hover])}
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
function ActivityGrid({
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
function StatsTable({
  rows,
  model,
}: {
  rows: (ModelStats | ProviderStats)[];
  model: boolean;
}) {
  return (
    <div className="table-wrap">
      <table>
        <thead>
          <tr>
            <th>{model ? "模型" : "供应商 / 应用"}</th>
            <th className="numeric">请求</th>
            <th className="numeric">Token</th>
            <th className="numeric">费用</th>
            <th className="numeric">平均费用</th>
            <th className="numeric">首字时间</th>
            <th className="numeric">成功率</th>
          </tr>
        </thead>
        <tbody>
          {rows.map((r, i) => (
            <tr key={i}>
              <td>
                {"model" in r ? r.model : r.providerName}
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
function RequestDetail({
  id,
  multi,
  close,
}: {
  id: string;
  multi: boolean;
  close: () => void;
}) {
  const data = useQuery({
    queryKey: ["usage", "detail", multi, id],
    queryFn: () =>
      usageCall<RequestLog | null>("request_detail", {
        requestId: id,
        multiDevice: multi,
      }),
    retry: false,
  });
  const r = data.data;
  return (
    <UsageDialog title="请求详情" close={close} wide>
      {data.error ? (
        <p className="usage-error">{friendlyError(data.error)}</p>
      ) : !r ? (
        <p className="hint">
          {data.isLoading ? "正在读取详情…" : "明细不存在或已按日汇总"}
        </p>
      ) : (
        <>
          <div className="usage-detail-grid">
            {[
              ["应用", r.appType],
              ["供应商", r.providerName ?? r.providerId],
              ["响应模型", r.model],
              ["请求模型", r.requestModel ?? "—"],
              ["计价模型", r.pricingModel ?? "未记录"],
              ["计费倍数", r.costMultiplier],
              ["状态码", r.statusCode],
              ["来源", r.dataSource ?? "proxy"],
              ["时间", timeText(r.createdAt)],
              [
                "请求耗时",
                r.latencyMs
                  ? (r.dataSource !== "proxy" ? "≈ " : "") +
                    (r.latencyMs / 1000).toFixed(3) +
                    "s"
                  : "—",
              ],
              [
                "首字时间",
                r.firstTokenMs != null
                  ? (r.dataSource !== "proxy" ? "≈ " : "") +
                    r.firstTokenMs +
                    "ms"
                  : "—",
              ],
              ["是否流式", r.isStreaming ? "是" : "否"],
              ["输出速度", formatRequestSpeed(r)],
            ].map(([label, value]) => (
              <div key={label}>
                <span>{label}</span>
                <strong>{value}</strong>
              </div>
            ))}
          </div>
          <div className="table-wrap">
            <table>
              <thead>
                <tr>
                  <th>计费项</th>
                  <th className="numeric">Token</th>
                  <th className="numeric">金额 USD</th>
                </tr>
              </thead>
              <tbody>
                {[
                  ["新增输入", getFreshInputTokens(r), r.inputCostUsd],
                  ["输出", r.outputTokens, r.outputCostUsd],
                  ["缓存读取", r.cacheReadTokens, r.cacheReadCostUsd],
                  ["缓存写入", r.cacheCreationTokens, r.cacheCreationCostUsd],
                  ["合计", metricValue(r), r.totalCostUsd],
                ].map(([name, t, c]) => (
                  <tr key={name}>
                    <td>{name}</td>
                    <td className="numeric">{Number(t).toLocaleString()}</td>
                    <td className="numeric">{formatMoney(c, 6)}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
          {r.errorMessage && <p className="usage-error">{r.errorMessage}</p>}
          <p className="hint usage-request-id">请求标识：{r.requestId}</p>
          <p className="hint">
            ≈ 表示按日志时间戳估算的耗时，与代理记录的精确计时分开解释。
          </p>
        </>
      )}
    </UsageDialog>
  );
}
function SourcesDialog({
  close,
  status,
  importPath,
  setImportPath,
  importBusy,
  importHistory,
  importError,
  importResult,
}: {
  importError?: unknown;
  importResult?: string;
  close: () => void;
  status?: NativeStatus;
  importPath: string;
  setImportPath: (v: string) => void;
  importBusy: boolean;
  importHistory: () => void;
}) {
  const qc = useQueryClient();
  const enabled =
    status?.sources.filter((a) => a.enabled).map((a) => a.id) ?? [];
  const save = useMutation({
    mutationFn: (selected: string[]) =>
      usageCall("sources_save", { enabled: selected }),
    onSuccess: async () => {
      await qc.invalidateQueries({ queryKey: ["usage", "status"] });
    },
  });
  return (
    <UsageDialog title="数据来源与历史迁移" close={close} wide>
      <p className="hint">
        原生日志以只读方式读取，开关控制后续扫描，不删除已有记录。独立数据库：
        {status?.databasePath}
      </p>
      {save.error && <p className="usage-error">{friendlyError(save.error)}</p>}
      <div className="usage-section-title">
        <h3>应用采集范围</h3>
        <button
          className="text-button"
          disabled={save.isPending}
          onClick={() => save.mutate(status?.sources.map((a) => a.id) ?? [])}
        >
          全部开启
        </button>
      </div>
      <div className="usage-sources">
        {status?.sources.map((a) => (
          <label key={a.id}>
            <span>
              <strong>{a.name}</strong>
              <small>
                {a.nativeSupported
                  ? "原生日志采集"
                  : "兼容 CC Switch 历史与代理统计导入"}{" "}
                · {a.requests.toLocaleString()} 次历史请求
              </small>
            </span>
            <input
              className="toggle"
              type="checkbox"
              aria-label={"采集 " + a.name}
              disabled={save.isPending}
              checked={a.enabled}
              onChange={(e) =>
                save.mutate(
                  e.target.checked
                    ? [...enabled, a.id]
                    : enabled.filter((id) => id !== a.id),
                )
              }
            />
          </label>
        ))}
      </div>
      <div className="usage-editor">
        <h3>只读导入 CC Switch 历史用量</h3>
        {!!importError && (
          <p role="alert" className="usage-error">
            {friendlyError(importError)}
          </p>
        )}
        {importResult && (
          <p className="usage-success" role="status">
            {importResult}
          </p>
        )}
        <p className="hint">
          带入统计明细、日汇总、扫描游标和去重记录；供应商仅保留名称，不复制配置和密钥。重复导入不会重复累计。
        </p>
        <label className="field">
          CC Switch 数据库路径
          <input
            aria-label="CC Switch 数据库路径"
            value={importPath}
            onChange={(e) => setImportPath(e.target.value)}
          />
        </label>
        <div className="dialog-actions">
          <button
            className="button primary"
            disabled={importBusy}
            onClick={importHistory}
          >
            <FileDown size={13} />
            {importBusy ? "导入中…" : "导入历史用量"}
          </button>
        </div>
      </div>
      <p className="hint">
        Hermes、OpenClaw、Claude Desktop
        当前通过已有统计导入或对端共享接入；尚未提供独立日志采集器。
      </p>
    </UsageDialog>
  );
}
