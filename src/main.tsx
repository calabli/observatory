import React, {
  lazy,
  Suspense,
  useCallback,
  useEffect,
  useRef,
  useState,
  type ReactNode,
} from "react";
import { createRoot } from "react-dom/client";
import { getCurrentWindow } from "@tauri-apps/api/window";
import {
  ArrowRight,
  ArrowUpRight,
  Boxes,
  Check,
  Clock3,
  Info,
  Laptop,
  Monitor,
  Moon,
  Network,
  PanelLeftClose,
  PanelLeftOpen,
  Radio,
  Radar,
  RefreshCw,
  ScrollText,
  ShieldCheck,
  Sparkles,
  Sun,
  TrendingUp,
  Trophy,
  Wifi,
  X,
  type LucideIcon,
} from "lucide-react";
const IS_WINDOWS = /Windows/i.test(navigator.userAgent);
import codex from "./assets/codex.svg";
import observatoryMark from "./assets/observatory-mark.svg";
import claude from "./assets/claude.svg";
import deepseek from "./assets/deepseek.svg";
import zcode from "./assets/zcode.png";
import "./styles.css";
import "./usage/usage.css";
import "./theme/ccSwitchTheme.css";
import "./theme/redesign.css";
import {
  QueryClient,
  QueryClientProvider,
  useQuery,
  useQueryClient,
} from "@tanstack/react-query";
// Page stylesheets are imported here, not in the lazily loaded pages, so they stay
// in the main bundle in this order and widget.css (imported last) still overrides them.
import "./usage/multiDeviceScan.css";
import { useLocalDayKey } from "./usage/useLocalDayKey";
import "./insights/insights.css";
import "./markets/markets.css";
import "./benchmarks/benchmarks.css";
import "./subscriptions/subscriptions.css";
import { DeviceManager } from "./usage/DeviceManager";
import { TrayPanel } from "./tray/TrayPanel";
import "./network/network.css";
import { UpdateControl } from "./UpdateControl";
import packageInfo from "../package.json";
import { usageCall } from "./native/usageClient";
import "./activity/activity.css";
import { waveColor } from "./theme/wave";
import { formatTokens } from "./usage/ui";
// Loaded last so it overrides every page stylesheet imported above.
import "./theme/widget.css";
import type { NativeStatus, Dashboard, LanStatus } from "./usage/types";

// Pages other than the overview load on first visit, keeping their code out of the
// startup bundle (and out of the tray panel, which shares this entry).
const UsageWorkspace = lazy(() =>
  import("./usage/UsageWorkspace").then((m) => ({ default: m.UsageWorkspace })),
);
const InsightsWorkspace = lazy(() =>
  import("./insights/InsightsWorkspace").then((m) => ({
    default: m.InsightsWorkspace,
  })),
);
const MarketsWorkspace = lazy(() =>
  import("./markets/MarketsWorkspace").then((m) => ({
    default: m.MarketsWorkspace,
  })),
);
const BenchmarksWorkspace = lazy(() =>
  import("./benchmarks/BenchmarksWorkspace").then((m) => ({
    default: m.BenchmarksWorkspace,
  })),
);
const SubscriptionWorkspace = lazy(() =>
  import("./subscriptions/SubscriptionWorkspace").then((m) => ({
    default: m.SubscriptionWorkspace,
  })),
);
const NetworkOverview = lazy(() =>
  import("./network/NetworkOverview").then((m) => ({
    default: m.NetworkOverview,
  })),
);
const ActivityLog = lazy(() =>
  import("./activity/ActivityLog").then((m) => ({ default: m.ActivityLog })),
);
const queryClient = new QueryClient({
  defaultOptions: { queries: { retry: false, refetchOnWindowFocus: false } },
});

/** Return body of `tray_dashboard`: the dashboard summary, trends and the per-app grouping subset. */
type OverviewUsage = Pick<Dashboard, "summary" | "trends" | "byApp">;
type Page =
  | "overview"
  | "usage"
  | "insights"
  | "subscriptions"
  | "benchmarks"
  | "markets"
  | "network"
  | "devices"
  | "activity";

const brandIcons: Record<string, string> = {
  codex,
  claude,
  dsh: deepseek,
  zcode,
};
type NavItem = {
  id: Page;
  label: string;
  icon: LucideIcon;
};
const navGroups: { label: string; items: NavItem[] }[] = [
  {
    label: "AI 观测",
    items: [
      { id: "overview", label: "总览", icon: Radar },
      { id: "usage", label: "AI 用量", icon: Boxes },
      { id: "insights", label: "洞察", icon: Sparkles },
      { id: "subscriptions", label: "订阅与额度", icon: Clock3 },
    ],
  },
  {
    label: "情报与行情",
    items: [
      { id: "benchmarks", label: "模型榜单", icon: Trophy },
      { id: "markets", label: "市场行情", icon: TrendingUp },
    ],
  },
  {
    label: "系统与设备",
    items: [
      { id: "devices", label: "多设备", icon: Monitor },
      { id: "network", label: "网络诊断", icon: Network },
      { id: "activity", label: "活动日志", icon: ScrollText },
    ],
  },
];
const SIDEBAR_KEY = "observatory.sidebar-collapsed";
function readSidebarCollapsed() {
  try {
    return localStorage.getItem(SIDEBAR_KEY) === "1";
  } catch {
    return false;
  }
}
const titles: Record<Page, [string, string]> = {
  overview: ["运行总览", "把 AI 调用、设备同步与网络状态放在一起看。"],
  usage: ["AI 用量", "从一次调用到多设备汇总，费用与用量清晰可查。"],
  insights: [
    "洞察",
    "缓存省钱、30 天预测、7×24 节律、会话膨胀与模型长尾稳定性。",
  ],
  subscriptions: [
    "订阅与额度",
    "管理订阅和额度周期，查看所选时间范围内的用量。",
  ],
  benchmarks: [
    "模型榜单",
    "Artificial Analysis 综合智力、工程、行业分项评比，结合吞吐速度、定价与本机实测对照。",
  ],
  markets: [
    "市场行情",
    "美股、台股、A 股、港股与全球核心指数免费实时盯盘（分市场直连最高时效免费接口）。",
  ],
  activity: [
    "活动日志",
    "扫描读取了哪些文件、谁调用了本机接口、设备同步与关键操作的记录。",
  ],
  network: ["网络诊断", "从地址、DNS 到连接，逐层查看网络状态。"],
  devices: ["多设备", "选择观测设备，管理用量与指标的共享范围。"],
};
const money = (n: number) => "$" + n.toFixed(2);
function BrandIcon({ id }: { id: string }) {
  return (
    <span className="brand-icon">
      <img src={brandIcons[id]} alt="" />
    </span>
  );
}
function Badge({
  children,
  tone = "neutral",
}: {
  children: ReactNode;
  tone?: "neutral" | "good" | "blue" | "warning";
}) {
  return <span className={"badge " + tone}>{children}</span>;
}
function Panel({
  title,
  sub,
  actions,
  children,
  className = "",
}: {
  title: string;
  sub?: string;
  actions?: ReactNode;
  children: ReactNode;
  className?: string;
}) {
  return (
    <section className={"panel " + className}>
      <div className="panel-head">
        <div>
          <h2>{title}</h2>
          {sub && <p>{sub}</p>}
        </div>
        <div className="panel-actions">{actions}</div>
      </div>
      {children}
    </section>
  );
}
/** Thin bars for a short series; the peak fills the height. */
function Bars({
  values,
  accent,
  className = "",
  titles,
}: {
  values: number[];
  accent?: string;
  className?: string;
  titles?: string[];
}) {
  const peak = Math.max(...values, 0) || 1;
  return (
    <div
      className={"bars " + className}
      aria-hidden={titles ? undefined : true}
    >
      {values.map((v, i) => (
        <i
          key={i}
          title={titles?.[i]}
          style={{
            height: `${Math.max(4, (v / peak) * 100)}%`,
            background: waveColor(i, values.length, accent),
          }}
        />
      ))}
    </div>
  );
}
function Stat({
  label,
  value,
  unit,
  detail,
  icon: Icon,
  values,
  accent,
  className = "",
}: {
  label: string;
  value: ReactNode;
  unit?: string;
  detail: ReactNode;
  icon: LucideIcon;
  values?: number[];
  accent?: string;
  className?: string;
}) {
  return (
    <div className={"stat " + className}>
      <div className="stat-label">
        {label}
        <Icon size={15} />
      </div>
      <div className="stat-value">
        {value}
        {unit && <small>{unit}</small>}
      </div>
      <div className="stat-detail">{detail}</div>
      {values && values.length > 0 && <Bars values={values} accent={accent} />}
    </div>
  );
}
function KeyValues({ rows }: { rows: [string, ReactNode][] }) {
  return (
    <dl className="key-values">
      {rows.map(([key, val]) => (
        <div key={key}>
          <dt>{key}</dt>
          <dd>{val}</dd>
        </div>
      ))}
    </dl>
  );
}
function Dialog({
  title,
  children,
  onClose,
}: {
  title: string;
  children: ReactNode;
  onClose: () => void;
}) {
  const ref = useRef<HTMLDialogElement>(null);
  useEffect(() => {
    ref.current?.showModal();
  }, []);
  return (
    <dialog
      ref={ref}
      className="dialog"
      onCancel={(e) => {
        e.preventDefault();
        onClose();
      }}
    >
      <div className="dialog-head">
        <h2>{title}</h2>
        <button
          className="icon-button"
          aria-label="关闭对话框"
          onClick={onClose}
        >
          <X size={17} />
        </button>
      </div>
      {children}
    </dialog>
  );
}

function App() {
  const [aboutOpen, setAboutOpen] = useState(false);
  const [updateTarget, setUpdateTarget] = useState<HTMLDivElement | null>(null);
  const [updateAvailable, setUpdateAvailable] = useState(false);
  const [page, setPage] = useState<Page>("overview");
  const [dark, setDark] = useState(
    () => window.matchMedia("(prefers-color-scheme:dark)").matches,
  );
  const [sidebarCollapsed, setSidebarCollapsed] =
    useState(readSidebarCollapsed);
  const toggleSidebar = () =>
    setSidebarCollapsed((collapsed) => {
      try {
        localStorage.setItem(SIDEBAR_KEY, collapsed ? "0" : "1");
      } catch {
        // Not remembering the sidebar state is harmless.
      }
      return !collapsed;
    });
  useEffect(() => {
    const theme = dark ? "dark" : "light";
    document.documentElement.style.colorScheme = theme;
    // Lets CSS pick per-theme assets, such as the dark-mode app icons.
    document.documentElement.dataset.theme = theme;
    try {
      localStorage.setItem("observatory.color-scheme", theme);
    } catch {
      // In private mode or with storage disabled the theme is simply not
      // remembered, which does not affect usage.
    }
    if (window.__TAURI_INTERNALS__) {
      void getCurrentWindow()
        .setTheme(theme)
        .catch((error) => {
          console.error("系统标题栏主题同步失败", error);
        });
    }
  }, [dark]);
  const [busy, setBusy] = useState(false);
  const [notice, setNotice] = useState("");
  const noticeTimer = useRef<ReturnType<typeof setTimeout>>();
  const [refreshInterval, setRefreshInterval] = useState("30");
  const qc = useQueryClient();
  const usageRefresh = useRef<() => Promise<void>>(async () => {});
  const registerRefresh = useCallback((fn: () => Promise<void>) => {
    usageRefresh.current = fn;
  }, []);
  const [nativeStatus, setNativeStatus] = useState<NativeStatus | undefined>();
  const nativeInfo = useQuery({
    queryKey: ["usage", "status"],
    queryFn: () => usageCall<NativeStatus>("status"),
    enabled: page === "overview",
    staleTime: 10000,
  });
  useEffect(() => {
    if (nativeInfo.data) {
      setNativeStatus(nativeInfo.data);
      if (nativeInfo.data.refreshInterval)
        setRefreshInterval(nativeInfo.data.refreshInterval);
    }
  }, [nativeInfo.data]);
  const [dayKey, syncDayKey] = useLocalDayKey();
  const todayStartSec = () => {
    const d = new Date();
    d.setHours(0, 0, 0, 0);
    return Math.floor(d.getTime() / 1000);
  };
  const weekStartSec = () => {
    const d = new Date();
    d.setDate(d.getDate() - 6);
    d.setHours(0, 0, 0, 0);
    return Math.floor(d.getTime() / 1000);
  };
  // The overview only needs the summary, trends and per-app grouping: use
  // the lightweight tray_dashboard so the backend does not also compute
  // request pagination, provider/model stats with medians and the filter
  // dropdowns.
  const overviewUsage = useQuery({
    queryKey: ["usage", "overview", dayKey],
    queryFn: () =>
      usageCall<OverviewUsage>("tray_dashboard", {
        startDate: todayStartSec(),
        endDate: Math.floor(Date.now() / 1000),
      }),
    enabled: page === "overview",
    staleTime: 30000,
    refetchInterval:
      refreshInterval === "manual" ? false : Number(refreshInterval) * 1000,
  });
  const overviewDevices = useQuery({
    queryKey: ["usage", "lan"],
    queryFn: () => usageCall<LanStatus>("lan_status"),
    enabled: page === "overview",
    staleTime: 10000,
    refetchInterval: 30000,
    retry: false,
  });
  const overviewWeek = useQuery({
    queryKey: ["usage", "overview-week", dayKey],
    queryFn: () =>
      usageCall<OverviewUsage>("tray_dashboard", {
        startDate: weekStartSec(),
        endDate: Math.floor(Date.now() / 1000),
      }),
    enabled: page === "overview",
    staleTime: 60000,
  });
  const overviewAllDevices = useQuery({
    queryKey: ["usage", "overview-all", dayKey],
    queryFn: () =>
      usageCall<Pick<Dashboard, "summary" | "trends">>("tray_dashboard", {
        startDate: todayStartSec(),
        endDate: Math.floor(Date.now() / 1000),
        multiDevice: true,
      }),
    enabled: page === "overview",
    staleTime: 30000,
  });
  // The overview uses a low-frequency passive snapshot (no forced metadata
  // refresh); live rate sampling lives on the network page.
  const overviewNet = useQuery({
    queryKey: ["usage", "overview-net"],
    queryFn: () =>
      usageCall<{
        interfaces: {
          name: string;
          defaultRoute: boolean;
          state: string;
          ipv4: string[];
          gateways: string[];
          dns: string[];
        }[];
      }>("network_snapshot", { refreshMetadata: false }),
    enabled: page === "overview",
    staleTime: 15000,
    refetchInterval: 30000,
    retry: false,
  });
  const toast = (message: string) => {
    setNotice(message);
    clearTimeout(noticeTimer.current);
    noticeTimer.current = setTimeout(() => setNotice(""), 3000);
  };
  // Only the overview shows the header refresh button; the other pages own
  // their refresh controls.
  const refreshOverview = () => {
    syncDayKey();
    setBusy(true);
    void usageCall("sync")
      .then(() => qc.invalidateQueries({ queryKey: ["usage"] }))
      .then(() => toast("AI 用量已更新"))
      .catch((e) => toast(e instanceof Error ? e.message : String(e)))
      .finally(() => setBusy(false));
  };
  const go = (p: Page) => setPage(p);
  const lanDevices = overviewDevices.data?.devices ?? [];
  const activeDevices = lanDevices.filter((x) => x.enabled);
  const errorDevices = lanDevices.filter((x) => x.lastError).length;
  const latestSync = activeDevices.reduce<number | null>(
    (acc, x) =>
      x.lastSyncAt && (acc === null || x.lastSyncAt > acc) ? x.lastSyncAt : acc,
    null,
  );
  const sources = nativeInfo.data?.sources ?? [];
  const enabledSources = sources.filter((s) => s.enabled);
  const netInterfaces = overviewNet.data?.interfaces ?? [];
  const defaultInterface =
    netInterfaces.find((x) => x.defaultRoute) ?? netInterfaces[0];
  // `lastSyncAt` of a device is in seconds, `lastScanAt` in milliseconds.
  const clock = (ms?: number | null) =>
    ms
      ? new Date(ms).toLocaleTimeString("zh-CN", {
          hour12: false,
          hour: "2-digit",
          minute: "2-digit",
        })
      : "尚未同步";
  const weekTrends = overviewWeek.data?.trends ?? [];
  const weekTokens = weekTrends.map((t) => t.totalTokens);
  const overview = (
    <>
      <div className="stats-grid">
        <Stat
          className="ov-cost"
          label="本机今日 AI 费用"
          value={
            overviewUsage.data
              ? "$" + Number(overviewUsage.data.summary.totalCost).toFixed(2)
              : "—"
          }
          detail={
            <>
              <span className="positive">
                {overviewUsage.data?.summary.totalRequests ?? "—"} 次调用
              </span>{" "}
              · 本机真实数据
            </>
          }
          icon={Boxes}
          values={overviewUsage.data?.trends.map((t) => Number(t.totalCost))}
        />
        <Stat
          label="全部设备今日费用"
          value={
            overviewAllDevices.data
              ? "$" +
                Number(overviewAllDevices.data.summary.totalCost).toFixed(2)
              : "—"
          }
          detail={
            <>
              <span className="positive">
                {overviewAllDevices.data?.summary.totalRequests ?? "—"} 次
              </span>{" "}
              · 含已纳入设备的最近同步快照
            </>
          }
          icon={Laptop}
          className="ov-all"
          accent="var(--accent-cyan)"
          values={overviewAllDevices.data?.trends.map((t) =>
            Number(t.totalCost),
          )}
        />
        <Stat
          label="采集健康"
          value={`${enabledSources.length}/${sources.length}`}
          unit="来源启用"
          detail={
            nativeInfo.data?.initialImportError ? (
              <span className="negative">历史导入报错，见数据来源页</span>
            ) : (
              <>最近本机扫描 {clock(nativeInfo.data?.lastScanAt)}</>
            )
          }
          icon={Radar}
        />
        <Stat
          label="多设备同步"
          value={activeDevices.length}
          unit="台纳入汇总"
          detail={
            <>
              最新同步 {clock(latestSync && latestSync * 1000)}
              {errorDevices > 0 ? (
                <span className="negative"> · {errorDevices} 台报错</span>
              ) : (
                ""
              )}
            </>
          }
          icon={Radio}
        />
      </div>
      <div className="overview-grid">
        <Panel title="近 7 天用量" sub="每日总 Token · 本机">
          {weekTrends.length ? (
            <>
              <Bars
                className="week-bars"
                values={weekTokens}
                titles={weekTrends.map(
                  (t) =>
                    `${t.date.slice(5, 10)} · ${formatTokens(t.totalTokens)}`,
                )}
              />
              <div className="week-labels">
                {weekTrends.map((t) => (
                  <span key={t.date}>{t.date.slice(5, 10)}</span>
                ))}
              </div>
            </>
          ) : (
            <p className="hint">暂无用量数据；扫描与历史迁移后显示。</p>
          )}
          <div className="panel-foot">
            <span className="muted">
              共{" "}
              <b className="foreground">
                {weekTrends.reduce((acc, t) => acc + t.requestCount, 0)}
              </b>{" "}
              次请求
            </span>
            <button className="text-button" onClick={() => go("usage")}>
              查看用量明细
              <ArrowUpRight size={13} />
            </button>
          </div>
        </Panel>
        <Panel
          title="网络状态"
          actions={
            defaultInterface ? (
              <Badge
                tone={defaultInterface.state === "up" ? "good" : "blue"}
                key="netbadge"
              >
                <span className="status-dot" />
                {defaultInterface.state === "up"
                  ? "已连接"
                  : defaultInterface.state}
              </Badge>
            ) : undefined
          }
        >
          {defaultInterface ? (
            <KeyValues
              rows={[
                [
                  "接口",
                  <span className="inline" key="if">
                    <Wifi size={13} />
                    {defaultInterface.name}
                  </span>,
                ],
                [
                  "IPv4",
                  <code key="ip">{defaultInterface.ipv4[0] ?? "未获取"}</code>,
                ],
                [
                  "网关",
                  <code key="gw">
                    {defaultInterface.gateways[0] ?? "未获取"}
                  </code>,
                ],
                [
                  "DNS",
                  <code key="dns">
                    {defaultInterface.dns.slice(0, 2).join(" / ") || "未获取"}
                  </code>,
                ],
              ]}
            />
          ) : (
            <p className="hint">正在读取网络信息…</p>
          )}
          <div className="panel-foot">
            <span className="muted">实时速率、IP 与诊断见网络页</span>
            <button className="text-button" onClick={() => go("network")}>
              网络概况
              <ArrowRight size={13} />
            </button>
          </div>
        </Panel>
      </div>
      <div className="overview-grid bottom-grid">
        <Panel
          title="本机 AI 应用费用 · Top 4"
          actions={
            <button className="text-button" onClick={() => go("usage")}>
              用量明细
              <ArrowUpRight size={13} />
            </button>
          }
        >
          <div className="app-costs">
            {overviewUsage.data?.byApp
              .filter((a) => a.summary.totalRequests > 0)
              .sort(
                (a, b) =>
                  Number(b.summary.totalCost) - Number(a.summary.totalCost),
              )
              .slice(0, 4)
              .map((a) => (
                <div key={a.appType}>
                  <div className="app-cost-label">
                    <span className="inline">
                      {brandIcons[a.appType] && <BrandIcon id={a.appType} />}{" "}
                      {nativeStatus?.sources.find((s) => s.id === a.appType)
                        ?.name ?? a.appType}
                    </span>
                    <code>{money(Number(a.summary.totalCost))}</code>
                  </div>
                  <div className="meter">
                    <span
                      style={{
                        width:
                          Math.max(
                            1,
                            (Number(a.summary.totalCost) /
                              Math.max(
                                0.0001,
                                Number(overviewUsage.data?.summary.totalCost),
                              )) *
                              100,
                          ) + "%",
                      }}
                    />
                  </div>
                </div>
              ))}
            {!overviewUsage.data && (
              <p className="hint">本地统计尚未连接，请打开 AI 用量页。</p>
            )}
          </div>
        </Panel>
        <Panel title="模块接入状态">
          <KeyValues
            rows={[
              [
                "AI 用量",
                <Badge tone="good" key="usage">
                  真实数据
                </Badge>,
              ],
              [
                "多设备同步",
                <Badge tone={lanDevices.length ? "good" : "blue"} key="lan">
                  {lanDevices.length ? "真实数据" : "未配置设备"}
                </Badge>,
              ],
              [
                "网络概况",
                <Badge tone="good" key="net">
                  真实数据
                </Badge>,
              ],
            ]}
          />
          <div className="panel-foot">
            <span className="muted">设备共享与授权见多设备页</span>
            <button className="text-button" onClick={() => go("devices")}>
              多设备
              <ArrowUpRight size={13} />
            </button>
          </div>
        </Panel>
      </div>
    </>
  );
  const usage = (
    <UsageWorkspace
      interval={refreshInterval}
      onRefreshReady={registerRefresh}
      onBusy={setBusy}
      onStatus={setNativeStatus}
    />
  );
  const devices = <DeviceManager />;
  return (
    <div
      className={`obs-shell${IS_WINDOWS ? " windows-polish" : ""}`}
      data-sidebar={sidebarCollapsed ? "collapsed" : "expanded"}
      style={{ colorScheme: dark ? "dark" : "light" }}
    >
      <aside className="app-sidebar">
        <button
          className="brand"
          onClick={() => go("overview")}
          aria-label="观测台首页"
          title="观测台"
        >
          <span className="brand-mark">
            <img src={observatoryMark} alt="" />
          </span>
          <span className="brand-text">
            观测台<small>Observatory</small>
          </span>
        </button>
        <nav aria-label="主导航">
          {navGroups.map((group) => (
            <div className="nav-group" key={group.label}>
              <div className="nav-group-label">{group.label}</div>
              {group.items.map((n) => (
                <button
                  key={n.id}
                  aria-current={page === n.id ? "page" : undefined}
                  onClick={() => go(n.id)}
                  title={n.label}
                >
                  <n.icon size={16} />
                  <span className="nav-label">{n.label}</span>
                </button>
              ))}
            </div>
          ))}
        </nav>
        <div className="sidebar-foot">
          <button
            className="sidebar-action"
            aria-label="关于观测台"
            title="关于观测台"
            onClick={() => setAboutOpen(true)}
          >
            <Info size={16} />
            {updateAvailable && (
              <span className="about-update-dot" aria-label="有新版本" />
            )}
          </button>
          <button
            className="sidebar-action"
            aria-label="切换明暗外观"
            title={dark ? "切换为浅色" : "切换为深色"}
            onClick={() => setDark(!dark)}
          >
            {dark ? <Sun size={16} /> : <Moon size={16} />}
          </button>
          <button
            className="sidebar-action sidebar-collapse"
            aria-label={sidebarCollapsed ? "展开侧边栏" : "收起侧边栏"}
            title={sidebarCollapsed ? "展开侧边栏" : "收起侧边栏"}
            aria-expanded={!sidebarCollapsed}
            onClick={toggleSidebar}
          >
            {sidebarCollapsed ? (
              <PanelLeftOpen size={16} />
            ) : (
              <PanelLeftClose size={16} />
            )}
          </button>
        </div>
      </aside>
      {aboutOpen && (
        <Dialog title="关于观测台" onClose={() => setAboutOpen(false)}>
          <div className="about-details">
            <div className="about-brand">
              <img className="about-mark" src={observatoryMark} alt="" />
              <strong>观测台 · Observatory</strong>
            </div>
            <p>AI 用量与设备观测工作台</p>
            <dl>
              <div>
                <dt>软件版本</dt>
                <dd>v{packageInfo.version}</dd>
              </div>
              <div>
                <dt>构建时间</dt>
                <dd>
                  <time dateTime={__OBS_BUILD_TIME__}>
                    {new Date(__OBS_BUILD_TIME__).toLocaleString("zh-CN", {
                      dateStyle: "medium",
                      timeStyle: "medium",
                    })}
                  </time>
                </dd>
              </div>
            </dl>
            <small>构建时间按当前设备的本地时区显示。</small>
            <p className="hint">
              关闭窗口后继续后台运行；可从系统托盘或菜单栏重新打开、查看本机今日用量或退出。
            </p>
            <div className="about-update" ref={setUpdateTarget} />
          </div>
        </Dialog>
      )}
      <UpdateControl target={updateTarget} onAvailable={setUpdateAvailable} />
      <main>
        <div className="page-heading">
          <div>
            <div className="eyebrow">
              {page === "overview"
                ? "WORKSPACE OVERVIEW"
                : page === "insights"
                  ? "DEEP INSIGHTS"
                  : page === "benchmarks"
                    ? "MODEL BENCHMARKS"
                    : page === "network"
                      ? "NETWORK OBSERVABILITY"
                      : page === "activity"
                        ? "ACTIVITY LOG"
                        : "OBSERVATORY"}
            </div>
            <h1>{titles[page][0]}</h1>
            <p>{titles[page][1]}</p>
          </div>
          <div className="page-controls">
            {page === "overview" && (
              <button
                className="icon-button"
                aria-label="刷新当前指标"
                onClick={refreshOverview}
                disabled={busy}
              >
                <RefreshCw size={15} className={busy ? "refreshing" : ""} />
              </button>
            )}
          </div>
        </div>
        {page !== "network" &&
          page !== "subscriptions" &&
          page !== "insights" &&
          page !== "benchmarks" &&
          page !== "markets" &&
          page !== "activity" && (
            <div className="context-bar">
              {page === "usage" || page === "devices" ? (
                <div className="inline">
                  <span className="status-dot" />
                  <span>
                    {nativeStatus?.device.deviceName ?? "本机真实用量"}
                  </span>
                  <span className="separator" />
                  <span>本机日志 · 只读历史导入</span>
                </div>
              ) : (
                <div className="inline">
                  <span className="status-dot" />
                  <span>
                    {overviewDevices.data?.local.deviceName ?? "本机"}
                  </span>
                  <span className="separator" />
                  <span>
                    {defaultInterface
                      ? defaultInterface.name
                      : "网络信息读取中"}
                  </span>
                  {defaultInterface?.ipv4[0] && (
                    <>
                      <span className="separator" />
                      <code>{defaultInterface.ipv4[0]}</code>
                    </>
                  )}
                </div>
              )}
              <div className="inline">
                <span>
                  {page === "usage" || page === "devices"
                    ? nativeStatus?.lastScanAt
                      ? "最近扫描 " +
                        new Date(nativeStatus.lastScanAt).toLocaleTimeString(
                          "zh-CN",
                          { hour12: false },
                        )
                      : "等待本机扫描"
                    : nativeInfo.data?.lastScanAt
                      ? "最近扫描 " +
                        new Date(nativeInfo.data.lastScanAt).toLocaleTimeString(
                          "zh-CN",
                          { hour12: false },
                        )
                      : "等待本机扫描"}
                </span>
                <select
                  value={refreshInterval}
                  onChange={(e) => {
                    const value = e.target.value;
                    setRefreshInterval(value);
                    void usageCall("preferences_save", {
                      refreshInterval: value,
                    })
                      .then(() =>
                        qc.invalidateQueries({ queryKey: ["usage", "status"] }),
                      )
                      .catch((error) => toast(String(error)));
                  }}
                  aria-label="自动刷新间隔"
                >
                  <option value="30">每 30 秒</option>
                  <option value="60">每 60 秒</option>
                  <option value="manual">手动刷新</option>
                </select>
              </div>
            </div>
          )}
        <Suspense fallback={<div className="panel page-loading">加载中…</div>}>
          {page === "overview" ? (
            overview
          ) : page === "usage" ? (
            usage
          ) : page === "insights" ? (
            <InsightsWorkspace
              sources={nativeStatus?.sources ?? nativeInfo.data?.sources ?? []}
            />
          ) : page === "subscriptions" ? (
            <SubscriptionWorkspace />
          ) : page === "benchmarks" ? (
            <BenchmarksWorkspace />
          ) : page === "markets" ? (
            <MarketsWorkspace />
          ) : page === "network" ? (
            <NetworkOverview />
          ) : page === "activity" ? (
            <ActivityLog />
          ) : (
            devices
          )}
        </Suspense>
        <footer className="app-footer">
          <span className="inline">
            <ShieldCheck size={13} />
            指标本地采集 · 自主选择共享
          </span>
          <span>数据只保存在本机 · 局域网共享默认关闭</span>
        </footer>
      </main>
      {notice && (
        <div className="toast" role="status">
          <Check size={15} />
          {notice}
        </div>
      )}
    </div>
  );
}
const isTrayPanel =
  new URLSearchParams(window.location.search).get("surface") === "tray";
if (isTrayPanel) {
  document.documentElement.classList.add("tray-window");
  document.body.classList.add("tray-window");
}
createRoot(document.getElementById("observatory-root")!).render(
  <QueryClientProvider client={queryClient}>
    {isTrayPanel ? <TrayPanel /> : <App />}
  </QueryClientProvider>,
);
