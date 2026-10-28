import { useEffect, useRef, useState } from "react";
import { useMutation, useQuery } from "@tanstack/react-query";
import { usageCall } from "../native/usageClient";
import { calculateRate, type Counters } from "./networkRate";
import "./network.css";

type NetInterface = {
  id: string;
  name: string;
  systemName: string;
  defaultRoute: boolean;
  state: string;
  ipv4: string[];
  ipv6: string[];
  gateways: string[];
  dns: string[];
  received: number | null;
  transmitted: number | null;
};
type Snapshot = { interfaces: NetInterface[] };
type Sample = { time: number; down: number; up: number };
function speed(bytes: number | null) {
  if (bytes === null) return "等待采样";
  if (bytes >= 1048576) return `${(bytes / 1048576).toFixed(2)} MB/s`;
  return `${(bytes / 1024).toFixed(1)} KB/s`;
}
function stateLabel(state: string) {
  return (
    (
      {
        up: "已连接",
        down: "已断开",
        dormant: "休眠",
        lowerlayerdown: "底层未连接",
        notpresent: "设备不存在",
        testing: "测试中",
        unknown: "状态未知",
      } as Record<string, string>
    )[state] ?? "状态未知"
  );
}

export function NetworkOverview() {
  const [tab, setTab] = useState("overview");
  const [selected, setSelected] = useState("");
  const [samples, setSamples] = useState<Sample[]>([]);
  const [rate, setRate] = useState<{ down: number; up: number } | null>(null);
  const publicIp = useMutation({
    mutationFn: () =>
      usageCall<{ ip: string; checkedAt: number }>(
        "network_international_ipv4",
      ),
  });
  const previous = useRef<Counters | null>(null);
  const forceRefresh = useRef(false);
  const query = useQuery({
    queryKey: ["network", "snapshot"],
    queryFn: () => {
      const refreshMetadata = forceRefresh.current;
      forceRefresh.current = false;
      return usageCall<Snapshot>("network_snapshot", { refreshMetadata });
    },
    enabled: tab === "overview",
    refetchInterval: 3000,
    refetchIntervalInBackground: false,
    refetchOnWindowFocus: true,
    retry: false,
  });
  const interfaces = query.data?.interfaces ?? [];
  const current =
    interfaces.find((i) => i.id === selected) ??
    (selected ? undefined : interfaces.find((i) => i.defaultRoute));
  useEffect(() => {
    if (query.isError) {
      previous.current = null;
      setRate(null);
      return;
    }
    if (
      !query.data ||
      !current ||
      current.received == null ||
      current.transmitted == null
    ) {
      previous.current = null;
      setRate(null);
      return;
    }
    const now = query.dataUpdatedAt;
    const last = previous.current;
    previous.current = {
      id: current.id,
      received: current.received,
      transmitted: current.transmitted,
      at: now,
    };
    const calculated = calculateRate(last, previous.current);
    if (!calculated) {
      setRate(null);
      if (!last || last.id !== current.id || now - last.at > 15000)
        setSamples([]);
      return;
    }
    const next = {
      time: now,
      ...calculated,
    };
    setRate(next);
    setSamples((s) => [...s, next].slice(-60));
  }, [query.dataUpdatedAt, query.isError, current?.id]);
  const max = Math.max(1, ...samples.flatMap((s) => [s.down, s.up]));
  const points = (key: "down" | "up") =>
    samples
      .map(
        (s, i) =>
          `${(i * 1000) / Math.max(1, samples.length - 1)},${150 - (s[key] / max) * 140}`,
      )
      .join(" ");
  return (
    <div className="network-live">
      <div className="tabs" aria-label="网络诊断功能">
        {[
          ["overview", "连接概览"],
          ["dns", "DNS 诊断"],
          ["interfaces", "网卡与路由"],
        ].map(([id, label]) => (
          <button
            key={id}
            aria-pressed={tab === id}
            onClick={() => {
              setTab(id);
              previous.current = null;
              setSamples([]);
              setRate(null);
            }}
          >
            {label}
          </button>
        ))}
      </div>
      {tab !== "overview" ? (
        <section className="panel section-gap">
          <h2>{tab === "dns" ? "DNS 诊断" : "网卡与路由"}</h2>
          <p className="hint">此功能尚未接入，后续阶段提供。</p>
        </section>
      ) : (
        <>
          <div className="network-toolbar">
            <span>本机网络 · 页面可见时采样 · 网卡配置每 30 秒刷新</span>
            <button
              className="button"
              disabled={query.isFetching}
              onClick={() => {
                forceRefresh.current = true;
                void query.refetch();
              }}
            >
              刷新
            </button>
          </div>
          {query.isError && (
            <p className="usage-error" role="alert">
              读取网卡失败：{String(query.error)}
            </p>
          )}
          <section className="panel network-public-ip">
            <div>
              <h2>国际出口 IPv4</h2>
              <strong>{publicIp.data?.ip ?? "—"}</strong>
              <p className="hint">
                {publicIp.data
                  ? `ipify · ${new Date(publicIp.data.checkedAt).toLocaleString("zh-CN", { hour12: false })}`
                  : "尚未检测"}
              </p>
            </div>
            <div className="network-public-action">
              <button
                className="button"
                disabled={publicIp.isPending}
                onClick={() => {
                  publicIp.reset();
                  publicIp.mutate();
                }}
              >
                {publicIp.isPending ? "查询中…" : "检测出口 IP"}
              </button>
              <small>
                手动请求 ipify；结果由系统及代理路由决定，与下方网卡选择不绑定。
              </small>
            </div>
            {publicIp.isError && (
              <p className="usage-error" role="alert">
                查询失败：{String(publicIp.error)}
              </p>
            )}
          </section>
          <div className="network-interface-choice">
            <label>
              网卡{" "}
              <select
                aria-label="选择网卡"
                value={current?.id ?? ""}
                onChange={(e) => {
                  setSelected(e.target.value);
                  previous.current = null;
                  setSamples([]);
                  setRate(null);
                }}
              >
                <option value="">
                  {interfaces.length ? "请选择网卡" : "没有可用网卡"}
                </option>
                {interfaces.map((i) => (
                  <option key={i.id} value={i.id}>
                    {i.name}
                    {i.defaultRoute ? " · 推荐网卡" : ""}
                  </option>
                ))}
              </select>
            </label>
            <span className="hint">
              {query.dataUpdatedAt
                ? `最近读取 ${new Date(query.dataUpdatedAt).toLocaleTimeString("zh-CN", { hour12: false })}`
                : "等待读取"}
            </span>
          </div>
          <div className="network-cards">
            <section className="panel">
              <span>本地 IPv4</span>
              <strong>{current?.ipv4[0] ?? "—"}</strong>
              <small>{current?.name ?? "请选择网卡"}</small>
            </section>
            <section className="panel">
              <span>连接状态</span>
              <strong>{current ? stateLabel(current.state) : "—"}</strong>
              <small>网卡状态不代表互联网可用</small>
            </section>
            <section className="panel">
              <span>下载速度</span>
              <strong>{speed(rate?.down ?? null)}</strong>
              <small>所选网卡接收速率</small>
            </section>
            <section className="panel">
              <span>上传速度</span>
              <strong>{speed(rate?.up ?? null)}</strong>
              <small>所选网卡发送速率</small>
            </section>
          </div>
          <div className="network-columns">
            <section className="panel network-chart">
              <h2>网络吞吐量</h2>
              <p className="hint">
                最近 {samples.length} 次采样 · 仅记录本次打开页面期间的数据
              </p>
              {samples.length >= 2 ? (
                <>
                  <div className="network-chart-legend">
                    <span>● 下载</span>
                    <span>● 上传</span>
                    <span>峰值 {speed(max)}</span>
                  </div>
                  <svg
                    viewBox="0 0 1000 160"
                    preserveAspectRatio="none"
                    role="img"
                    aria-label="所选网卡最近下载与上传速度"
                  >
                    <polyline
                      points={points("down")}
                      fill="none"
                      stroke="var(--accent)"
                      strokeWidth="2"
                      vectorEffect="non-scaling-stroke"
                    />
                    <polyline
                      points={points("up")}
                      fill="none"
                      stroke="var(--teal)"
                      strokeWidth="2"
                      vectorEffect="non-scaling-stroke"
                    />
                  </svg>
                  <div className="network-chart-times">
                    <span>
                      {new Date(samples[0].time).toLocaleTimeString("zh-CN", {
                        hour12: false,
                      })}
                    </span>
                    <span>
                      {new Date(
                        samples[samples.length - 1].time,
                      ).toLocaleTimeString("zh-CN", { hour12: false })}
                    </span>
                  </div>
                </>
              ) : (
                <p className="network-wait">等待至少两次有效速率采样…</p>
              )}
            </section>
            <section className="panel network-connection">
              <h2>当前网卡</h2>
              <dl>
                {[
                  ["接口", current?.name],
                  ["系统名称", current?.systemName],
                  ["IPv4", current?.ipv4.join(" / ")],
                  ["IPv6", current?.ipv6.join(" / ")],
                  ["默认网关", current?.gateways.join(" / ")],
                  ["DNS 服务器", current?.dns.join(" / ")],
                ].map(([label, value]) => (
                  <div key={label}>
                    <dt>{label}</dt>
                    <dd>{value || "未获取"}</dd>
                  </div>
                ))}
              </dl>
              <p className="hint">
                推荐网卡由系统路由推测，VPN 或分流时可手动切换。DNS
                与网关来自所选网卡的系统配置，VPN 或系统策略可能影响实际路径。
              </p>
            </section>
          </div>
          <section className="panel section-gap">
            <h2>待接入</h2>
            <p className="hint">
              地区信息、网关延迟、应用连接与进程流量将在后续阶段提供。
            </p>
          </section>
        </>
      )}
    </div>
  );
}
