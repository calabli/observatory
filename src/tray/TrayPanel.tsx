import { useEffect, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { invoke } from "@tauri-apps/api/core";
import { Radar, X, ExternalLink, Power } from "lucide-react";
import { usageCall } from "../native/usageClient";
import { AppGlyph } from "../usage/AppGlyph";
import { formatMoney, formatTokens, timeLabel } from "../usage/ui";
import { trendTokens } from "../usage/chartData";
import type { Dashboard, LanStatus, NativeStatus } from "../usage/types";
import "./tray.css";

type TrayData = Pick<Dashboard, "summary" | "byApp" | "trends">;
const intervals = (range: "today" | "week") => {
  const start = new Date();
  start.setHours(0, 0, 0, 0);
  if (range === "week") start.setDate(start.getDate() - 6);
  return {
    startDate: Math.floor(start.getTime() / 1000),
    endDate: Math.floor(Date.now() / 1000),
  };
};
export function TrayPanel() {
  const [range, setRange] = useState<"today" | "week">("today");
  const [scope, setScope] = useState("all");
  const [visible, setVisible] = useState(
    document.visibilityState === "visible",
  );
  const [theme, setTheme] = useState(
    () =>
      localStorage.getItem("observatory.color-scheme") ||
      (matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light"),
  );
  useEffect(() => {
    const onVisibility = () =>
      setVisible(document.visibilityState === "visible");
    const onStorage = (event: StorageEvent) => {
      if (event.key === "observatory.color-scheme" && event.newValue)
        setTheme(event.newValue);
    };
    const onKeyDown = (event: KeyboardEvent) => {
      if (
        event.key === "Escape" &&
        event.target instanceof HTMLElement &&
        event.target.tagName !== "SELECT"
      ) {
        void invoke("tray_action", { action: "close" }).catch(console.error);
      }
    };
    document.addEventListener("visibilitychange", onVisibility);
    window.addEventListener("storage", onStorage);
    window.addEventListener("keydown", onKeyDown);
    return () => {
      document.removeEventListener("visibilitychange", onVisibility);
      window.removeEventListener("storage", onStorage);
      window.removeEventListener("keydown", onKeyDown);
    };
  }, []);
  const status = useQuery({
    queryKey: ["tray", "status"],
    queryFn: () => usageCall<NativeStatus>("status"),
    enabled: visible,
    refetchInterval: visible ? 30000 : false,
    retry: false,
  });
  const lan = useQuery({
    queryKey: ["tray", "lan"],
    queryFn: () => usageCall<LanStatus>("lan_status"),
    enabled: visible && !!status.data,
    refetchInterval: visible ? 30000 : false,
    retry: false,
  });
  const enabled = lan.data?.devices.filter((d) => d.enabled) ?? [];
  useEffect(() => {
    if (
      lan.data &&
      scope !== "all" &&
      scope !== "local" &&
      !lan.data.devices.some((d) => d.enabled && d.deviceId === scope)
    )
      setScope("all");
  }, [lan.data, scope]);
  const peer = enabled.find((d) => d.deviceId === scope);
  const selectedAvailable = scope === "all" || scope === "local" || !!peer;
  const stats = useQuery({
    queryKey: ["tray", "dashboard", scope, range],
    queryFn: () =>
      usageCall<TrayData>("tray_dashboard", {
        ...intervals(range),
        multiDevice: scope === "all",
        ...(peer ? { deviceId: peer.deviceId } : {}),
      }),
    enabled: visible && !!status.data && !!lan.data && selectedAvailable,
    refetchInterval: visible ? 30000 : false,
    refetchOnWindowFocus: true,
    retry: false,
  });
  const unknownEmptyPeer =
    !!peer && !peer.lastSyncAt && stats.data?.summary.totalRequests === 0;
  const visibleData =
    selectedAvailable && !stats.isError && !unknownEmptyPeer
      ? stats.data
      : undefined;
  const summary = visibleData?.summary;
  const apps = [...(visibleData?.byApp ?? [])]
    .sort((a, b) => Number(b.summary.totalCost) - Number(a.summary.totalCost))
    .slice(0, 3);
  const points = (visibleData?.trends ?? []).slice(-15).map(trendTokens);
  const peak = Math.max(0, ...points);
  const cacheDenominator = summary
    ? summary.totalInputTokens +
      summary.totalCacheReadTokens +
      summary.totalCacheCreationTokens
    : 0;
  const name = (app: string) =>
    status.data?.sources.find((s) => s.id === app)?.name ?? app;
  const pendingPeers = enabled.filter((d) => !d.lastSyncAt).length;
  const failedPeers = enabled.filter((d) => d.lastError).length;
  const syncedTimes = enabled.flatMap((d) =>
    d.lastSyncAt ? [d.lastSyncAt] : [],
  );
  const oldestSync = syncedTimes.length ? Math.min(...syncedTimes) : null;
  const info =
    scope === "all"
      ? enabled.length
        ? `本机 + ${enabled.length} 台设备${pendingPeers ? ` · ${pendingPeers} 台同步时间未知` : ""}${failedPeers ? ` · ${failedPeers} 台同步失败` : ""}${oldestSync ? ` · 最早同步 ${timeLabel(oldestSync)}` : ""}`
        : "仅本机（没有启用的远端设备）"
      : scope === "local"
        ? "仅本机"
        : peer
          ? `${peer.deviceName} · ${peer.lastSyncAt ? timeLabel(peer.lastSyncAt) : "同步时间未知，数据可能不完整"}${peer.lastError ? " · 同步失败，显示缓存" : ""}`
          : "设备不可用";
  const action = (name: string) =>
    void invoke("tray_action", { action: name }).catch(console.error);
  return (
    <div
      className="obs-shell tray-shell"
      style={{ colorScheme: theme === "dark" ? "dark" : "light" }}
    >
      <div className="tray-panel">
        <header className="tray-head">
          <div className="tray-brand">
            <span className="tray-mark">
              <Radar size={20} />
            </span>
            <span>
              <strong>观测台</strong>
              <small>OBSERVATORY</small>
            </span>
          </div>
          <span className="tray-status">
            {status.isError
              ? "连接失败"
              : status.data
                ? "本机运行中"
                : "连接中"}
          </span>
          <button
            className="tray-icon"
            aria-label="关闭面板"
            onClick={() => action("close")}
          >
            <X size={17} />
          </button>
        </header>
        <main className="tray-body">
          <div className="tray-heading">
            <div>
              <h1>用量概览</h1>
              <span>按所选设备汇总</span>
            </div>
            <div className="tray-range" aria-label="统计时间">
              <button
                aria-pressed={range === "today"}
                onClick={() => setRange("today")}
              >
                今天
              </button>
              <button
                aria-pressed={range === "week"}
                onClick={() => setRange("week")}
              >
                近 7 天
              </button>
            </div>
          </div>
          <div className="tray-device">
            <label htmlFor="tray-device">统计设备</label>
            <select
              id="tray-device"
              value={scope}
              onChange={(e) => setScope(e.target.value)}
            >
              <option value="all">全部设备 · {1 + enabled.length} 台</option>
              <option value="local">
                本机 · {lan.data?.local.deviceName ?? "本机"}
              </option>
              {enabled.map((d) => (
                <option key={d.deviceId} value={d.deviceId}>
                  {d.deviceName}
                  {d.lastSyncAt ? "" : " · 同步时间未知"}
                </option>
              ))}
            </select>
          </div>
          <div className="tray-device-note">{info}</div>
          {(stats.isError || status.isError || lan.isError) && (
            <p className="tray-error" role="alert">
              读取失败：{String(stats.error ?? status.error ?? lan.error)}
            </p>
          )}
          {status.data?.initialImportError && (
            <p className="tray-error" role="alert">
              历史用量导入未完成，请打开观测台查看。
            </p>
          )}
          {!selectedAvailable && (
            <p className="tray-error">所选设备不存在或未启用，请重新选择。</p>
          )}
          <div className="tray-hero">
            <div>
              <span>
                估算费用 <small>USD</small>
              </span>
              <strong>{summary ? formatMoney(summary.totalCost) : "—"}</strong>
              <small>按记录价格估算</small>
            </div>
            <div
              className="tray-bars"
              role="img"
              aria-label="所选范围 Token 趋势"
            >
              {points.map((v, i) => (
                <span
                  key={i}
                  title={stats.data?.trends.slice(-15)[i]?.date}
                  style={{
                    height:
                      peak > 0 ? `${Math.max(2, (v / peak) * 100)}%` : "2%",
                  }}
                />
              ))}
            </div>
          </div>
          <div className="tray-stats">
            {[
              ["请求次数", summary?.totalRequests.toLocaleString()],
              [
                "总 Token",
                summary ? formatTokens(summary.realTotalTokens) : undefined,
              ],
              [
                "缓存命中率",
                summary && cacheDenominator > 0
                  ? (summary.cacheHitRate * 100).toFixed(1) + "%"
                  : undefined,
              ],
              [
                "成功率",
                summary && summary.totalRequests > 0
                  ? summary.successRate.toFixed(1) + "%"
                  : undefined,
              ],
            ].map(([label, value]) => (
              <div key={label}>
                <span>{label}</span>
                <strong>{value ?? "—"}</strong>
              </div>
            ))}
          </div>
          <div className="tray-section">
            <strong>应用用量</strong>
            <span>按费用 · USD</span>
          </div>
          <div className="tray-apps">
            {apps.length ? (
              apps.map((a) => (
                <div key={a.appType}>
                  <AppGlyph app={a.appType} />
                  <span>
                    {name(a.appType)}
                    <small>
                      {a.summary.totalRequests.toLocaleString()} 次请求
                    </small>
                  </span>
                  <b>{formatMoney(a.summary.totalCost)}</b>
                </div>
              ))
            ) : (
              <p>
                {stats.isLoading
                  ? "正在读取…"
                  : unknownEmptyPeer
                    ? "未发现可用缓存"
                    : "暂无用量记录"}
              </p>
            )}
          </div>
          <div className="tray-foot">
            <span>
              本机采集：{status.data?.lastScanAt ? "已扫描" : "尚未扫描"}
            </span>
            <span>
              {status.data?.lastScanAt
                ? new Date(status.data.lastScanAt).toLocaleTimeString("zh-CN", {
                    hour12: false,
                  })
                : "—"}
            </span>
            <span>
              {status.data?.refreshInterval === "manual"
                ? "手动刷新"
                : status.data?.refreshInterval
                  ? `每 ${status.data.refreshInterval} 秒`
                  : "—"}
            </span>
          </div>
        </main>
        <footer className="tray-actions">
          <button className="tray-primary" onClick={() => action("open")}>
            <ExternalLink size={14} />
            打开观测台
          </button>
          <button onClick={() => action("quit")}>
            <Power size={14} />
            退出
          </button>
        </footer>
      </div>
    </div>
  );
}
