import React, {
  useCallback,
  useEffect,
  useRef,
  useState,
  type ReactNode,
} from "react";
import { createRoot } from "react-dom/client";
import { getCurrentWindow } from "@tauri-apps/api/window";
import {
  Activity,
  ArrowDown,
  ArrowDownLeft,
  ArrowRight,
  ArrowUp,
  ArrowUpRight,
  Bell,
  Boxes,
  Cable,
  Check,
  CheckCheck,
  ChevronDown,
  ChevronRight,
  CircleHelp,
  Clock3,
  Cpu,
  Database,
  Download,
  EthernetPort,
  ExternalLink,
  Globe,
  Info,
  HardDrive,
  Laptop,
  Layers3,
  ListTree,
  MemoryStick,
  Monitor,
  Moon,
  Network,
  Plus,
  Radio,
  Radar,
  RefreshCw,
  Search,
  Settings2,
  ShieldCheck,
  Sun,
  Tags,
  Terminal,
  Waypoints,
  Wifi,
  X,
  type LucideIcon,
} from "lucide-react";
import {
  connections,
  cpuSeries,
  deviceInfo,
  downSeries,
  memorySeries,
  processes,
  spans,
  upSeries,
  usageSeries,
  type DeviceId,
  type Page,
} from "./data";
import openai from "./assets/openai.svg";
import claude from "./assets/claude.svg";
import deepseek from "./assets/deepseek.svg";
import zcode from "./assets/zcode.png";
import "./styles.css";
import "./usage/usage.css";
import "./theme/ccSwitchTheme.css";
import {
  QueryClient,
  QueryClientProvider,
  useQuery,
  useQueryClient,
} from "@tanstack/react-query";
import { UsageWorkspace } from "./usage/UsageWorkspace";
import { DeviceManager } from "./usage/DeviceManager";
import { UpdateControl } from "./UpdateControl";
import packageInfo from "../package.json";
import { usageCall } from "./native/usageClient";
import type { NativeStatus, Dashboard } from "./usage/types";
const queryClient = new QueryClient({
  defaultOptions: { queries: { retry: false, refetchOnWindowFocus: false } },
});

type PreviewState = { page?: Page; device?: DeviceId };
declare global {
  interface Window {
    openai?: {
      widgetState?: { privateContent?: PreviewState };
      setWidgetState?: (value: {
        modelContent: PreviewState;
        privateContent: PreviewState;
      }) => Promise<void>;
    };
  }
}
const restored = window.openai?.widgetState?.privateContent;
function remember(value: PreviewState) {
  window.openai
    ?.setWidgetState?.({ modelContent: value, privateContent: value })
    .catch(() => {});
}

const brandIcons: Record<string, string> = {
  codex: openai,
  claude,
  dsh: deepseek,
  zcode,
};
const nav: { id: Page; label: string; icon: LucideIcon }[] = [
  { id: "overview", label: "总览", icon: Radar },
  { id: "usage", label: "AI 用量", icon: Boxes },
  { id: "agents", label: "Agent 追踪", icon: ListTree },
  { id: "performance", label: "设备性能", icon: Activity },
  { id: "network", label: "网络诊断", icon: Network },
  { id: "devices", label: "多设备", icon: Monitor },
];
const titles: Record<Page, [string, string]> = {
  overview: ["运行总览", "把 AI 调用、设备负载与网络状态放在一起看。"],
  usage: ["AI 用量", "从一次调用到多设备汇总，费用与用量清晰可查。"],
  agents: [
    "Agent 追踪",
    "沿着任务调用链，查看模型、工具与子 Agent 的执行过程。",
  ],
  performance: ["设备性能", "观察资源变化，找到影响工作流的负载。"],
  network: ["网络诊断", "从地址、DNS 到连接，逐层查看网络状态。"],
  devices: ["多设备", "选择观测设备，管理用量与指标的共享范围。"],
};
const money = (n: number) => "$" + n.toFixed(2);
const compact = (n: number) =>
  n >= 1e6
    ? (n / 1e6).toFixed(2) + "M"
    : n >= 1000
      ? (n / 1000).toFixed(1) + "K"
      : String(n);
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
function Button({
  children,
  onClick,
  primary = false,
  disabled = false,
  icon: Icon,
}: {
  children: ReactNode;
  onClick?: () => void;
  primary?: boolean;
  disabled?: boolean;
  icon?: LucideIcon;
}) {
  return (
    <button
      className={"button " + (primary ? "primary" : "")}
      onClick={onClick}
      disabled={disabled}
    >
      {Icon && <Icon size={14} />} {children}
    </button>
  );
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
function Spark({
  values,
  color = "var(--accent)",
}: {
  values: number[];
  color?: string;
}) {
  const points = values
    .map(
      (v, i) =>
        `${(i * 90) / (values.length - 1)},${34 - (v / Math.max(...values)) * 27}`,
    )
    .join(" ");
  return (
    <svg className="spark" viewBox="0 0 90 38" aria-hidden="true">
      <polyline
        points={points}
        fill="none"
        stroke={color}
        strokeWidth="1.6"
        strokeLinejoin="round"
      />
    </svg>
  );
}
function Stat({
  label,
  value,
  unit,
  detail,
  icon: Icon,
  values,
  tone = "blue",
}: {
  label: string;
  value: ReactNode;
  unit?: string;
  detail: ReactNode;
  icon: LucideIcon;
  values?: number[];
  tone?: string;
}) {
  return (
    <div className="stat">
      <div className="stat-label">
        {label}
        <Icon size={15} />
      </div>
      <div className="stat-value">
        {value}
        {unit && <small>{unit}</small>}
      </div>
      <div className="stat-detail">{detail}</div>
      {values && (
        <Spark
          values={values}
          color={tone === "teal" ? "var(--teal)" : "var(--accent)"}
        />
      )}
    </div>
  );
}
function Segmented({
  options,
  value,
  onChange,
  label,
}: {
  options: { id: string; label: string }[];
  value: string;
  onChange: (s: string) => void;
  label: string;
}) {
  return (
    <div className="segmented" aria-label={label}>
      {options.map((x) => (
        <button
          key={x.id}
          aria-pressed={x.id === value}
          onClick={() => onChange(x.id)}
        >
          {x.label}
        </button>
      ))}
    </div>
  );
}
function Trend({
  series,
  max = 100,
  unit = "%",
  ticks = ["12:20", "12:25", "12:30", "12:35", "12:40"],
  height = 156,
}: {
  series: { name: string; values: number[]; color: string }[];
  max?: number;
  unit?: string;
  ticks?: string[];
  height?: number;
}) {
  const [hover, setHover] = useState<number | null>(null);
  const format = (v: number) =>
    v === 0 ? "0" : v.toFixed(max <= 5 ? 1 : 0) + unit;
  return (
    <div className="trend">
      <div className="legend">
        {series.map((x) => (
          <span key={x.name}>
            <i style={{ background: x.color }} />
            {x.name}
          </span>
        ))}
      </div>
      <div className="plot-row">
        <div className="yaxis" style={{ height }}>
          {[1, 0.75, 0.5, 0.25, 0].map((v) => (
            <span key={v}>{format(max * v)}</span>
          ))}
        </div>
        <div
          className="plot"
          style={{ height }}
          onPointerMove={(e) => {
            const r = e.currentTarget.getBoundingClientRect();
            setHover(
              Math.max(
                0,
                Math.min(23, Math.round(((e.clientX - r.left) / r.width) * 23)),
              ),
            );
          }}
          onPointerLeave={() => setHover(null)}
        >
          <svg
            viewBox="0 0 1000 180"
            preserveAspectRatio="none"
            role="img"
            aria-label={series.map((s) => s.name).join("、") + "变化趋势"}
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
            {series.map((s, i) => {
              const pts = s.values
                .map(
                  (v, k) =>
                    `${(k * 1000) / (s.values.length - 1)},${168 - (v / max) * 150}`,
                )
                .join(" ");
              return (
                <g key={s.name}>
                  {i === 0 && (
                    <polygon
                      points={"0,168 " + pts + " 1000,168"}
                      fill={s.color}
                      opacity=".075"
                    />
                  )}
                  <polyline
                    points={pts}
                    fill="none"
                    stroke={s.color}
                    strokeWidth="2"
                    vectorEffect="non-scaling-stroke"
                    strokeLinecap="round"
                    strokeLinejoin="round"
                  />
                </g>
              );
            })}
            {hover !== null && (
              <line
                x1={(hover * 1000) / 23}
                x2={(hover * 1000) / 23}
                y1="0"
                y2="174"
                stroke="var(--muted)"
                strokeWidth="1"
                vectorEffect="non-scaling-stroke"
                strokeDasharray="3 4"
              />
            )}
          </svg>
          {hover !== null && (
            <div
              className="chart-tooltip"
              style={{
                left: `clamp(5px, ${(hover / 23) * 100 - 12}%, calc(100% - 148px))`,
              }}
            >
              <small>采样点 {hover + 1}</small>
              {series.map((s) => (
                <span key={s.name}>
                  <i style={{ background: s.color }} />
                  {s.name}
                  <b>
                    {format(s.values[Math.min(hover, s.values.length - 1)])}
                  </b>
                </span>
              ))}
            </div>
          )}
        </div>
      </div>
      <div className="xaxis">
        {ticks.map((t, i) => (
          <span key={i}>{t}</span>
        ))}
      </div>
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
function Toggle({
  label,
  checked,
  onChange,
  sub,
}: {
  label: string;
  checked: boolean;
  onChange: (v: boolean) => void;
  sub?: string;
}) {
  return (
    <label className="toggle-row">
      <span>
        {label}
        {sub && <small>{sub}</small>}
      </span>
      <input
        className="toggle"
        type="checkbox"
        checked={checked}
        onChange={(e) => onChange(e.target.checked)}
        aria-label={label}
      />
    </label>
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
  const [page, setPage] = useState<Page>(
    nav.some((n) => n.id === restored?.page) ? restored!.page! : "overview",
  );
  const [device, setDevice] = useState<DeviceId>(
    restored?.device === "mac" ? "mac" : "windows",
  );
  const [dark, setDark] = useState(
    () => window.matchMedia("(prefers-color-scheme:dark)").matches,
  );
  useEffect(() => {
    const apply = (event: Event) => {
      const state = (event as CustomEvent).detail?.globals?.widgetState
        ?.privateContent as PreviewState | undefined;
      if (state?.page && nav.some((n) => n.id === state.page))
        setPage(state.page);
      if (state?.device === "mac" || state?.device === "windows")
        setDevice(state.device);
    };
    window.addEventListener("openai:set_globals", apply);
    return () => window.removeEventListener("openai:set_globals", apply);
  }, []);
  useEffect(() => {
    const theme = dark ? "dark" : "light";
    document.documentElement.style.colorScheme = theme;
    if (window.__TAURI_INTERNALS__) {
      void getCurrentWindow()
        .setTheme(theme)
        .catch((error) => {
          console.error("系统标题栏主题同步失败", error);
        });
    }
  }, [dark]);
  const [busy, setBusy] = useState(false);
  const [updated, setUpdated] = useState(false);
  const [notice, setNotice] = useState("");
  const noticeTimer = useRef<ReturnType<typeof setTimeout>>();
  const [range, setRange] = useState("15m");
  const [netTab, setNetTab] = useState("connections");
  const [perfTab, setPerfTab] = useState("overview");
  const [query, setQuery] = useState("");
  const [domain, setDomain] = useState("api.deepseek.com");
  const [dnsServer, setDnsServer] = useState("system");
  const [dnsBusy, setDnsBusy] = useState(false);
  const [dnsResult, setDnsResult] = useState(false);
  const [dnsError, setDnsError] = useState("");
  const [selectedSpan, setSelectedSpan] = useState(2);

  const [refreshInterval, setRefreshInterval] = useState("30");
  const d = deviceInfo[device];
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
  const todayStart = new Date();
  todayStart.setHours(0, 0, 0, 0);
  const overviewUsage = useQuery({
    queryKey: ["usage", "overview", todayStart.getTime()],
    queryFn: () =>
      usageCall<Dashboard>("dashboard", {
        startDate: Math.floor(todayStart.getTime() / 1000),
        endDate: Math.floor(Date.now() / 1000),
        pageSize: 20,
      }),
    enabled: page === "overview",
    staleTime: 30000,
    refetchInterval:
      refreshInterval === "manual" ? false : Number(refreshInterval) * 1000,
  });
  const toast = (message: string) => {
    setNotice(message);
    clearTimeout(noticeTimer.current);
    noticeTimer.current = setTimeout(() => setNotice(""), 3000);
  };
  const refresh = () => {
    if (page === "usage") {
      void usageRefresh.current().catch(() => {});
      return;
    }
    if (page === "overview") {
      setBusy(true);
      void usageCall("sync")
        .then(() => qc.invalidateQueries({ queryKey: ["usage"] }))
        .then(() => {
          setUpdated(true);
          toast("AI 用量已更新；设备与网络数据仍为演示");
        })
        .catch((e) => toast(e instanceof Error ? e.message : String(e)))
        .finally(() => setBusy(false));
      return;
    }
    setBusy(true);
    setTimeout(() => {
      setBusy(false);
      setUpdated(true);
      toast("设备与网络模块仍为演示数据");
    }, 600);
  };
  const go = (p: Page) => {
    setPage(p);
    setQuery("");
    remember({ page: p, device });
  };
  const perfFactor = range === "hour" ? 1.16 : range === "day" ? 0.82 : 1;
  const cpu = cpuSeries.map((v) =>
    Math.min(96, v * perfFactor * (device === "mac" ? 0.7 : 1)),
  );
  const runDns = () => {
    if (
      !/^(?:[a-z0-9](?:[a-z0-9-]*[a-z0-9])?\.)+[a-z]{2,}$/i.test(domain.trim())
    ) {
      setDnsError("请输入有效域名，例如 api.deepseek.com");
      setDnsResult(false);
      return;
    }
    setDnsError("");
    setDnsBusy(true);
    setDnsResult(false);
    setTimeout(() => {
      setDnsBusy(false);
      setDnsResult(true);
    }, 650);
  };
  const performanceChart = (
    <Trend
      ticks={
        range === "15m"
          ? ["12:25", "12:29", "12:33", "12:37", "12:40"]
          : range === "hour"
            ? ["11:40", "11:55", "12:10", "12:25", "12:40"]
            : ["昨天12:40", "18:40", "00:40", "06:40", "12:40"]
      }
      series={[
        { name: "CPU 使用率", values: cpu, color: "var(--accent)" },
        {
          name: "内存使用率",
          values: memorySeries.map(
            (v) =>
              (v * ((d.memory / d.memoryTotal) * 100)) / ((12.6 / 32) * 100),
          ),
          color: "var(--teal)",
        },
      ]}
    />
  );
  const rangePicker = (
    <Segmented
      label="性能时间范围"
      value={range}
      onChange={setRange}
      options={[
        { id: "15m", label: "15 分钟" },
        { id: "hour", label: "1 小时" },
        { id: "day", label: "24 小时" },
      ]}
    />
  );
  const networkRows: [string, ReactNode][] = [
    [
      "连接方式",
      <span className="inline">
        <Wifi size={13} />
        {d.adapter} · Studio Wi-Fi
      </span>,
    ],
    ["本地 IPv4", <code>{d.ip}</code>],
    ["公网 IPv4", <code>203.0.113.42</code>],
    ["默认网关", <code>{d.gateway}</code>],
    ["DNS 服务器", <code>223.5.5.5 / 1.1.1.1</code>],
    ["代理状态", <Badge tone="blue">Mihomo TUN</Badge>],
  ];
  const overview = (
    <>
      <div className="stats-grid">
        <Stat
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
          label="CPU 使用率"
          value={d.cpuUsage}
          unit="%"
          detail={d.cpu + " · " + d.cores}
          icon={Cpu}
          values={cpu}
        />
        <Stat
          label="内存使用"
          value={d.memory}
          unit={"/ " + d.memoryTotal + " GB"}
          detail={
            <>
              {Math.round((d.memory / d.memoryTotal) * 100)}% 已用 · 可用{" "}
              {(d.memoryTotal - d.memory).toFixed(1)} GB
            </>
          }
          icon={MemoryStick}
          values={memorySeries}
          tone="teal"
        />
        <Stat
          label="实时网络流量"
          value={
            <>
              <ArrowDown size={20} />
              {d.down}
            </>
          }
          unit="MB/s"
          detail={
            <span className="inline">
              <ArrowUp size={12} />
              {d.up} KB/s 上传 · {d.adapter}
            </span>
          }
          icon={Network}
          values={downSeries}
        />
      </div>
      <div className="overview-grid">
        <Panel title="设备负载" sub="CPU 与内存使用率" actions={rangePicker}>
          {performanceChart}
          <div className="panel-foot">
            <span className="inline">
              <span className="status-dot" />
              当前负载平稳
            </span>
            <button className="text-button" onClick={() => go("performance")}>
              查看设备性能
              <ArrowUpRight size={13} />
            </button>
          </div>
        </Panel>
        <Panel
          title="网络状态"
          actions={
            <Badge tone="good">
              <span className="status-dot" />
              已连接
            </Badge>
          }
        >
          {<KeyValues rows={networkRows} />}
          <div className="panel-foot">
            <span className="muted">
              网关延迟 <b className="foreground">3 ms</b>
            </span>
            <button
              className="text-button"
              onClick={() => {
                setNetTab("dns");
                go("network");
              }}
            >
              DNS 诊断
              <ArrowRight size={13} />
            </button>
          </div>
        </Panel>
      </div>
      <div className="overview-grid bottom-grid">
        <Panel
          title="最近 Agent 任务"
          sub="任务执行轨迹 · 规划预览"
          actions={
            <button className="text-button" onClick={() => go("agents")}>
              查看追踪
              <ArrowUpRight size={13} />
            </button>
          }
        >
          <div className="task-row">
            <span className="task-icon">
              <Waypoints size={17} />
            </span>
            <div>
              <strong>定位网络请求超时</strong>
              <small>主 Agent → 子 Agent → 4 次工具与模型调用</small>
            </div>
            <div className="task-meta">
              <Badge tone="good">已完成</Badge>
              <small>8.24 秒 · $0.048</small>
            </div>
          </div>
          <div className="task-row">
            <span className="task-icon">
              <ListTree size={17} />
            </span>
            <div>
              <strong>检查模型费用差异</strong>
              <small>Codex · 2 次模型调用</small>
            </div>
            <div className="task-meta">
              <Badge tone="good">已完成</Badge>
              <small>4.62 秒 · $0.032</small>
            </div>
          </div>
        </Panel>
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
  const agentPage = (
    <>
      <div className="callout">
        <ListTree size={16} />
        <span>
          调用栈设计预览：未来可关联任务、子
          Agent、模型与工具调用。当前记录为演示数据。
        </span>
        <Badge tone="blue">规划中</Badge>
      </div>
      <div className="trace-summary">
        <div>
          <span className="eyebrow">任务 / TRACE</span>
          <h2>定位网络请求超时</h2>
          <p>Codex · Windows 工作站 · 今天 12:38:24</p>
        </div>
        <div className="trace-summary-stats">
          <span>
            总耗时<strong>8.24 s</strong>
          </span>
          <span>
            总 Token<strong>32.8K</strong>
          </span>
          <span>
            估算费用<strong>$0.048</strong>
          </span>
          <Badge tone="good">已完成</Badge>
        </div>
      </div>
      <div className="trace-layout">
        <Panel title="调用栈与执行时间线" sub="点击调用查看详情">
          <div className="trace-axis">
            <span>调用</span>
            <div>
              {["0 s", "2 s", "4 s", "6 s", "8 s"].map((t) => (
                <span key={t}>{t}</span>
              ))}
            </div>
          </div>
          {spans.map((s) => (
            <button
              className={
                "span-row " + (selectedSpan === s.id ? "selected" : "")
              }
              key={s.id}
              onClick={() => setSelectedSpan(s.id)}
            >
              <span className="span-name" style={{ paddingLeft: s.depth * 14 }}>
                {s.depth > 0 ? (
                  <ChevronRight size={11} />
                ) : (
                  <Waypoints size={12} />
                )}
                <span>
                  {s.name}
                  <small>{s.type}</small>
                </span>
              </span>
              <span className="span-lane">
                <span
                  className={
                    "span-bar " + (s.type === "工具调用" ? "teal" : "")
                  }
                  style={{
                    left: (s.start / 8.5) * 100 + "%",
                    width: (s.duration / 8.5) * 100 + "%",
                  }}
                />
              </span>
              <span className="span-duration">{s.duration.toFixed(2)}s</span>
            </button>
          ))}
        </Panel>
        <Panel title="调用详情" actions={<Badge tone="good">已完成</Badge>}>
          <div className="span-detail-title">
            <Waypoints size={20} />
            <strong>{spans[selectedSpan].name}</strong>
          </div>
          <KeyValues
            rows={[
              ["类型", spans[selectedSpan].type],
              ["持续时间", spans[selectedSpan].duration.toFixed(2) + " 秒"],
              ["Token", spans[selectedSpan].tokens],
              ["费用", spans[selectedSpan].cost],
              [
                "父调用",
                selectedSpan === 0
                  ? "—"
                  : selectedSpan === 3 || selectedSpan === 4
                    ? "诊断 DNS 解析"
                    : "定位网络请求超时",
              ],
              ["所属设备", d.name],
            ]}
          />
          <p className="hint">
            此处预留输入输出摘要、工具参数及错误信息的查看入口。
          </p>
        </Panel>
      </div>
    </>
  );
  const performance = (
    <>
      <div className="device-heading">
        <div className="inline">
          <span className="device-icon">
            <Laptop size={24} />
          </span>
          <div>
            <strong>{d.name}</strong>
            <p>
              {d.os} · {d.cpu} · {d.cores}
            </p>
          </div>
        </div>
        <span className="muted inline">
          <Clock3 size={14} />
          已运行 {d.uptime}
        </span>
      </div>
      <div className="stats-grid">
        <Stat
          label="CPU 使用率"
          value={d.cpuUsage}
          unit="%"
          detail="平均频率 3.2 GHz"
          icon={Cpu}
          values={cpu}
        />
        <Stat
          label="内存使用"
          value={d.memory}
          unit={"/ " + d.memoryTotal + " GB"}
          detail="内存压力正常"
          icon={MemoryStick}
          values={memorySeries}
          tone="teal"
        />
        <Stat
          label="磁盘已用"
          value={d.disk}
          unit={"/ " + d.diskTotal + " GB"}
          detail={"使用率 " + Math.round((d.disk / d.diskTotal) * 100) + "%"}
          icon={HardDrive}
        />
        <Stat
          label="GPU 使用率"
          value={12}
          unit="%"
          detail="显存 2.1 / 8 GB · 演示"
          icon={Monitor}
          values={cpuSeries.map((v) => v * 0.6)}
        />
      </div>
      <div className="tabs">
        {[
          { id: "overview", label: "资源趋势" },
          { id: "processes", label: "进程" },
          { id: "disk", label: "磁盘与 I/O" },
        ].map((t) => (
          <button
            key={t.id}
            aria-pressed={perfTab === t.id}
            onClick={() => setPerfTab(t.id)}
          >
            {t.label}
          </button>
        ))}
      </div>
      {perfTab === "overview" ? (
        <Panel title="CPU 与内存" actions={rangePicker}>
          {performanceChart}
        </Panel>
      ) : perfTab === "disk" ? (
        <Panel title="磁盘与 I/O">
          <div className="disk-overview">
            <span className="device-icon">
              <HardDrive size={25} />
            </span>
            <div>
              <h3>系统磁盘 · SSD</h3>
              <p>
                {d.disk} GB 已用 / {d.diskTotal} GB
              </p>
              <div className="meter">
                <span style={{ width: (d.disk / d.diskTotal) * 100 + "%" }} />
              </div>
            </div>
            <span>
              读取<strong>18.4 MB/s</strong>
            </span>
            <span>
              写入<strong>4.2 MB/s</strong>
            </span>
          </div>
          <Trend
            series={[
              {
                name: "读取",
                values: downSeries.map((v) => v * 7),
                color: "var(--accent)",
              },
              {
                name: "写入",
                values: upSeries.map((v) => v * 7),
                color: "var(--teal)",
              },
            ]}
            max={30}
            unit=" MB/s"
          />
        </Panel>
      ) : null}
      <Panel
        title="进程资源"
        className="section-gap"
        actions={
          <label className="search">
            <Search size={14} />
            <input
              aria-label="搜索进程"
              placeholder="搜索进程"
              value={query}
              onChange={(e) => setQuery(e.target.value)}
            />
          </label>
        }
      >
        <div className="table-wrap">
          <table>
            <thead>
              <tr>
                <th>进程</th>
                <th>PID</th>
                <th className="numeric">CPU ↓</th>
                <th className="numeric">内存</th>
                <th className="numeric">网络</th>
                <th>类型</th>
              </tr>
            </thead>
            <tbody>
              {processes
                .filter((p) =>
                  p.name.toLowerCase().includes(query.toLowerCase()),
                )
                .map((p) => (
                  <tr key={p.pid}>
                    <td className="inline">
                      <span className="process-icon">
                        <Terminal size={12} />
                      </span>
                      {p.name}
                    </td>
                    <td className="muted mono">{p.pid}</td>
                    <td className="numeric">{p.cpu}%</td>
                    <td className="numeric">{p.memory.toLocaleString()} MB</td>
                    <td className="numeric">{p.network}</td>
                    <td>
                      <Badge>{p.role}</Badge>
                    </td>
                  </tr>
                ))}
            </tbody>
          </table>
        </div>
        {processes.filter((p) =>
          p.name.toLowerCase().includes(query.toLowerCase()),
        ).length === 0 && <div className="empty">没有匹配的进程</div>}
      </Panel>
    </>
  );
  const network = (
    <>
      <div className="tabs">
        {[
          { id: "connections", label: "连接概览", icon: Globe },
          { id: "dns", label: "DNS 诊断", icon: Search },
          { id: "interfaces", label: "网卡与路由", icon: EthernetPort },
        ].map((t) => (
          <button
            key={t.id}
            aria-pressed={netTab === t.id}
            onClick={() => setNetTab(t.id)}
          >
            <t.icon size={14} />
            {t.label}
          </button>
        ))}
      </div>
      {netTab === "connections" ? (
        <>
          <div className="stats-grid">
            <Stat
              label="本地 IPv4"
              value={<span className="ip-value">{d.ip}</span>}
              detail={d.adapter + " · IPv4 / IPv6"}
              icon={Laptop}
            />
            <Stat
              label="公网 IPv4"
              value={<span className="ip-value">203.0.113.42</span>}
              detail="演示地址"
              icon={Globe}
            />
            <Stat
              label="网关时延"
              value="3"
              unit="ms"
              detail="丢包率 0% · 演示采样"
              icon={Activity}
              values={[4, 3, 4, 5, 3, 4, 2, 3, 4, 3]}
            />
            <Stat
              label="下载 / 上传"
              value={d.down}
              unit="MB/s"
              detail={d.up + " KB/s 上传"}
              icon={ArrowDownLeft}
              values={downSeries}
            />
          </div>
          <div className="overview-grid">
            <Panel title="网络吞吐量" actions={<Badge>{d.adapter}</Badge>}>
              <Trend
                series={[
                  { name: "下载", values: downSeries, color: "var(--accent)" },
                  { name: "上传", values: upSeries, color: "var(--teal)" },
                ]}
                max={4}
                unit=" MB/s"
              />
            </Panel>
            <Panel title="当前连接" actions={<Badge tone="good">在线</Badge>}>
              <KeyValues rows={networkRows} />
            </Panel>
          </div>
          <Panel
            title="应用连接"
            sub="按应用查看目标地址、协议与实时流量"
            actions={
              <Button icon={RefreshCw} onClick={refresh} disabled={busy}>
                刷新
              </Button>
            }
          >
            <div className="table-wrap">
              <table>
                <thead>
                  <tr>
                    <th>进程 / 应用</th>
                    <th>目标</th>
                    <th>远端地址</th>
                    <th>协议</th>
                    <th className="numeric">流量</th>
                    <th>状态</th>
                  </tr>
                </thead>
                <tbody>
                  {connections.map((c) => (
                    <tr key={c.process}>
                      <td>{c.process}</td>
                      <td>{c.target}</td>
                      <td className="mono muted">{c.address}</td>
                      <td>
                        <Badge>{c.protocol}</Badge>
                      </td>
                      <td className="numeric">{c.traffic}</td>
                      <td>
                        <span className="positive">{c.status}</span>
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          </Panel>
        </>
      ) : netTab === "dns" ? (
        <>
          <Panel title="DNS 解析诊断" sub="比较系统 DNS 与指定服务器的解析结果">
            <div className="dns-form">
              <label>
                查询域名
                <div className="input-with-icon">
                  <Globe size={15} />
                  <input
                    aria-label="查询域名"
                    value={domain}
                    onChange={(e) => {
                      setDomain(e.target.value);
                      setDnsResult(false);
                    }}
                    onKeyDown={(e) => e.key === "Enter" && runDns()}
                  />
                </div>
              </label>
              <label>
                DNS 服务器
                <select
                  aria-label="DNS 服务器"
                  value={dnsServer}
                  onChange={(e) => {
                    setDnsServer(e.target.value);
                    setDnsResult(false);
                  }}
                >
                  <option value="system">系统默认 · 223.5.5.5</option>
                  <option value="1.1.1.1">1.1.1.1</option>
                  <option value="223.5.5.5">223.5.5.5</option>
                </select>
              </label>
              <Button primary icon={Search} onClick={runDns} disabled={dnsBusy}>
                {dnsBusy ? "查询中…" : "查询解析"}
              </Button>
            </div>
            {dnsError && (
              <p role="alert" className="error-message">
                {dnsError}
              </p>
            )}
            {dnsBusy && (
              <div className="loading-bars" aria-label="正在查询">
                <span />
                <span />
                <span />
              </div>
            )}
            {dnsResult ? (
              <div className="dns-results">
                <div className="panel-head">
                  <h3 className="inline">
                    <CheckCheck size={16} />
                    {domain}
                  </h3>
                  <Badge tone="good">演示解析成功 · 32 ms</Badge>
                </div>
                <div className="table-wrap">
                  <table>
                    <thead>
                      <tr>
                        <th>记录类型</th>
                        <th>解析结果</th>
                        <th>TTL</th>
                        <th>服务器</th>
                      </tr>
                    </thead>
                    <tbody>
                      <tr>
                        <td>
                          <Badge tone="blue">A</Badge>
                        </td>
                        <td className="mono">203.0.113.10</td>
                        <td>300 秒</td>
                        <td className="mono">
                          {dnsServer === "system" ? "223.5.5.5" : dnsServer}
                        </td>
                      </tr>
                      <tr>
                        <td>
                          <Badge tone="blue">AAAA</Badge>
                        </td>
                        <td className="mono">2001:db8::10</td>
                        <td>300 秒</td>
                        <td className="mono">
                          {dnsServer === "system" ? "223.5.5.5" : dnsServer}
                        </td>
                      </tr>
                    </tbody>
                  </table>
                </div>
                <p className="hint">
                  此为设计稿的演示结果，未发起真实 DNS 查询。
                </p>
              </div>
            ) : (
              !dnsBusy && (
                <div className="dns-empty">
                  <Waypoints size={30} />
                  <strong>从一次解析，检查网络路径</strong>
                  <span>查看记录类型、返回地址、TTL 与 DNS 服务器。</span>
                </div>
              )
            )}
          </Panel>
          <Panel title="DNS 配置" className="section-gap">
            <KeyValues
              rows={[
                ["系统 DNS", "223.5.5.5"],
                ["备用 DNS", "1.1.1.1"],
                ["解析方式", "系统解析器"],
                ["代理 DNS", "Mihomo · TUN 已启用"],
                ["所属网卡", d.adapter],
              ]}
            />
          </Panel>
        </>
      ) : (
        <>
          <Panel title="网络接口" sub="区分物理网卡、代理与虚拟网络">
            <div className="table-wrap">
              <table>
                <thead>
                  <tr>
                    <th>接口</th>
                    <th>类型</th>
                    <th>IPv4</th>
                    <th>子网前缀</th>
                    <th>网关</th>
                    <th>状态</th>
                  </tr>
                </thead>
                <tbody>
                  {[
                    [d.adapter, "物理网卡", d.ip, "/24", d.gateway, "已连接"],
                    [
                      "Mihomo",
                      "TUN 虚拟网卡",
                      "198.18.0.1",
                      "/30",
                      "198.18.0.2",
                      "已启用",
                    ],
                    [
                      "vEthernet (WSL)",
                      "虚拟网卡",
                      "198.18.192.1",
                      "/20",
                      "—",
                      "已启用",
                    ],
                  ].map((r) => (
                    <tr key={r[0]}>
                      {r.map((v, i) => (
                        <td key={i}>
                          {i === 1 ? (
                            <Badge>{v}</Badge>
                          ) : i === 5 ? (
                            <span className="positive">{v}</span>
                          ) : (
                            <span className={i >= 2 ? "mono" : ""}>{v}</span>
                          )}
                        </td>
                      ))}
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          </Panel>
          <Panel
            title="IPv4 路由"
            className="section-gap"
            sub="物理局域网与代理默认路由分开呈现"
          >
            <div className="table-wrap">
              <table>
                <thead>
                  <tr>
                    <th>目标网络</th>
                    <th>下一跳</th>
                    <th>接口</th>
                    <th className="numeric">Metric</th>
                    <th>用途</th>
                  </tr>
                </thead>
                <tbody>
                  {[
                    ["203.0.113.0/24", "直连", d.adapter, "25", "局域网"],
                    ["0.0.0.0/0", "198.18.0.2", "Mihomo", "5", "代理默认路由"],
                    ["0.0.0.0/0", d.gateway, d.adapter, "35", "物理网关"],
                  ].map((r) => (
                    <tr key={r[0] + r[3]}>
                      {r.map((v, i) => (
                        <td
                          key={i}
                          className={i === 3 ? "numeric" : i < 2 ? "mono" : ""}
                        >
                          {v}
                        </td>
                      ))}
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          </Panel>
          <div className="callout subtle">
            <ShieldCheck size={15} />
            <span>
              网卡、DNS
              和路由在此只读查看。真实监控接入后，配置修改会使用独立操作流程。
            </span>
          </div>
        </>
      )}
    </>
  );
  const devices = <DeviceManager />;
  return (
    <div
      className={`obs-shell${/Windows/i.test(navigator.userAgent) ? " windows-polish" : ""}`}
      style={{ colorScheme: dark ? "dark" : "light" }}
    >
      <header className="app-nav">
        <button
          className="brand"
          onClick={() => go("overview")}
          aria-label="观测台首页"
        >
          <span className="brand-mark">
            <Radar size={24} />
          </span>
          <span>
            观测台<small>OBSERVATORY</small>
          </span>
        </button>
        <nav aria-label="主导航">
          {nav.map((n) => (
            <button
              key={n.id}
              aria-current={page === n.id ? "page" : undefined}
              onClick={() => go(n.id)}
            >
              <n.icon size={15} />
              {n.label}
            </button>
          ))}
        </nav>
        <button
          className="icon-button"
          aria-label="关于观测台"
          title="关于观测台"
          onClick={() => setAboutOpen(true)}
        >
          <Info size={17} />
        </button>
        <button
          className="icon-button"
          aria-label="切换明暗外观"
          onClick={() => setDark(!dark)}
        >
          {dark ? <Sun size={17} /> : <Moon size={17} />}
        </button>
      </header>
      {aboutOpen && (
        <Dialog title="关于观测台" onClose={() => setAboutOpen(false)}>
          <div className="about-details">
            <div className="about-brand">
              <Radar size={24} />
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
          </div>
        </Dialog>
      )}
      <main>
        <div className="page-heading">
          <div>
            <div className="eyebrow">
              {page === "overview"
                ? "WORKSPACE OVERVIEW"
                : page === "network"
                  ? "NETWORK OBSERVABILITY"
                  : page === "performance"
                    ? "DEVICE METRICS"
                    : page === "agents"
                      ? "AGENT TRACING"
                      : "OBSERVATORY"}
            </div>
            <h1>
              {titles[page][0]}
              {page === "agents" && <Badge tone="blue">规划预览</Badge>}
              {page === "overview" && <Badge>部分指标为演示</Badge>}
              {(page === "performance" || page === "network") && (
                <Badge>演示数据</Badge>
              )}
            </h1>
            <p>{titles[page][1]}</p>
          </div>
          <div className="page-controls">
            <UpdateControl />
            {page !== "usage" && page !== "devices" && (
              <label className="device-picker">
                <Laptop size={14} />
                <select
                  value={device}
                  aria-label="观测设备"
                  onChange={(e) => setDevice(e.target.value as DeviceId)}
                >
                  <option value="windows">Windows 工作站</option>
                  <option value="mac">MacBook Pro</option>
                </select>
              </label>
            )}
            <button
              className="icon-button"
              aria-label="刷新当前指标"
              onClick={refresh}
              disabled={busy}
            >
              <RefreshCw size={15} className={busy ? "refreshing" : ""} />
            </button>
          </div>
        </div>
        <div className="context-bar">
          {page === "usage" || page === "devices" ? (
            <div className="inline">
              <span className="status-dot" />
              <span>{nativeStatus?.device.deviceName ?? "本机真实用量"}</span>
              <span className="separator" />
              <span>本机日志 · 只读历史导入</span>
            </div>
          ) : (
            <div className="inline">
              <span className="status-dot" />
              <span>{d.os}</span>
              <span className="separator" />
              <code>{d.ip}</code>
              <span className="separator" />
              <span>{d.adapter}</span>
              {page === "overview" && <Badge>本机与局域网</Badge>}
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
                : updated
                  ? "刚刚更新"
                  : "最近更新 12:42:08"}
            </span>
            <select
              value={refreshInterval}
              onChange={(e) => {
                const value = e.target.value;
                setRefreshInterval(value);
                void usageCall("preferences_save", { refreshInterval: value })
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
        {page === "overview"
          ? overview
          : page === "usage"
            ? usage
            : page === "agents"
              ? agentPage
              : page === "performance"
                ? performance
                : page === "network"
                  ? network
                  : devices}
        <footer className="app-footer">
          <span className="inline">
            <ShieldCheck size={13} />
            指标本地采集 · 自主选择共享
          </span>
          <span>AI 用量已接入 · 设备、网络和 Agent 为预览</span>
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
createRoot(document.getElementById("observatory-root")!).render(
  <QueryClientProvider client={queryClient}>
    <App />
  </QueryClientProvider>,
);
