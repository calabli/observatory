import { useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Laptop, Plus, Radar, RefreshCw, Check, Trash2 } from "lucide-react";
import { usageCall } from "../native/usageClient";
import type { LanStatus, LanSettings, Discovered, NativeStatus } from "./types";
import { friendlyError, timeLabel } from "./ui";

export function DeviceManager() {
  const qc = useQueryClient();
  const [host, setHost] = useState("");
  const [found, setFound] = useState<Discovered[]>([]);
  const [message, setMessage] = useState("");
  const state = useQuery({
    queryKey: ["usage", "lan"],
    queryFn: () => usageCall<LanStatus>("lan_status"),
  });
  const status = useQuery({
    queryKey: ["usage", "status"],
    queryFn: () => usageCall<NativeStatus>("status"),
  });
  const invalidate = async () => {
    await qc.invalidateQueries({ queryKey: ["usage"] });
  };
  const settings = useMutation({
    mutationFn: (s: LanSettings) => usageCall("lan_settings_save", s),
    onSuccess: invalidate,
  });
  const discovery = useMutation({
    mutationFn: () => usageCall<Discovered[]>("lan_discover"),
    onSuccess: (r) => {
      setFound(r);
      setMessage(
        r.length
          ? `发现 ${r.length} 台设备`
          : "未发现其他设备，可按 IP:端口手动连接。",
      );
    },
  });
  const connect = useMutation({
    mutationFn: (address: { host: string; port?: number }) =>
      usageCall<{ error?: string; imported: number }>("lan_connect", address),
    onSuccess: async (r) => {
      setHost("");
      setMessage(
        r.error
          ? "设备已登记，首次同步失败：" + r.error
          : "设备已连接，用量已同步。",
      );
      await invalidate();
    },
  });
  const enable = useMutation({
    mutationFn: (args: { deviceId: string; enabled: boolean }) =>
      usageCall("lan_device_enabled", args),
    onSuccess: invalidate,
  });
  const disconnect = useMutation({
    mutationFn: (deviceId: string) => usageCall("lan_disconnect", { deviceId }),
    onSuccess: invalidate,
  });
  const sync = useMutation({
    mutationFn: () => usageCall("sync"),
    onSuccess: async () => {
      setMessage("本机扫描和设备同步已完成。");
      await invalidate();
    },
  });
  const error =
    state.error ||
    status.error ||
    settings.error ||
    discovery.error ||
    connect.error ||
    enable.error ||
    disconnect.error ||
    sync.error;
  const config = state.data?.settings;
  const set = (patch: Partial<LanSettings>) => {
    if (config) settings.mutate({ ...config, ...patch });
  };
  return (
    <div className="native-devices">
      {error && (
        <p className="usage-error" role="alert">
          {friendlyError(error)}
        </p>
      )}
      {message && (
        <div className="usage-note" role="status">
          {message}
        </div>
      )}
      <div className="section-bar">
        <span className="muted">
          {state.data?.local.deviceName ?? "读取本机信息…"} ·{" "}
          {state.data?.devices.length ?? 0} 台已登记设备
        </span>
        <div className="filters">
          <button
            className="button"
            disabled={discovery.isPending}
            onClick={() => discovery.mutate()}
          >
            <Radar size={14} />
            {discovery.isPending ? "扫描中…" : "扫描局域网"}
          </button>
          <button
            className="button"
            disabled={sync.isPending}
            onClick={() => sync.mutate()}
          >
            <RefreshCw size={14} />
            {sync.isPending ? "同步中…" : "同步全部"}
          </button>
        </div>
      </div>
      {config && (
        <section className="panel">
          <div className="panel-head">
            <div>
              <h2>共享本机 AI 用量</h2>
              <p>独立端口默认 17891，连接 CC Switch 设备时可填写 IP:17890。</p>
            </div>
            <label className="inline">
              <span>{config.enabled ? "已开启" : "已关闭"}</span>
              <input
                className="toggle"
                type="checkbox"
                aria-label="开启本机用量共享"
                checked={config.enabled}
                disabled={settings.isPending}
                onChange={(e) => set({ enabled: e.target.checked })}
              />
            </label>
          </div>
          <div className="usage-lan-fields">
            <label>
              设备名称
              <input
                value={config.deviceName}
                placeholder={state.data?.local.deviceName}
                onChange={(e) => {
                  qc.setQueryData<LanStatus>(["usage", "lan"], (old) =>
                    old
                      ? {
                          ...old,
                          settings: {
                            ...old.settings,
                            deviceName: e.target.value,
                          },
                        }
                      : old,
                  );
                }}
                onBlur={(e) => set({ deviceName: e.target.value })}
              />
            </label>
            <label>
              共享端口
              <input
                type="number"
                min="1024"
                max="65535"
                defaultValue={config.port}
                key={config.port}
                onBlur={(e) => {
                  const port = Number(e.target.value);
                  if (Number.isInteger(port) && port >= 1024 && port <= 65535)
                    set({ port });
                }}
              />
            </label>
            <label className="usage-check">
              <input
                type="checkbox"
                checked={config.autoSync}
                disabled={settings.isPending}
                onChange={(e) => set({ autoSync: e.target.checked })}
              />
              后台自动同步
            </label>
          </div>
          <div className="usage-section-title">
            <h3>允许共享的应用</h3>
            <button
              className="text-button"
              disabled={settings.isPending}
              onClick={() =>
                set({
                  sharedAppTypes: (status.data?.sources ?? []).map((a) => a.id),
                })
              }
            >
              全部开启
            </button>
          </div>
          <div className="usage-share-grid">
            {status.data?.sources.map((a) => (
              <label key={a.id}>
                <input
                  type="checkbox"
                  disabled={settings.isPending}
                  checked={config.sharedAppTypes.includes(a.id)}
                  onChange={(e) =>
                    set({
                      sharedAppTypes: e.target.checked
                        ? [...config.sharedAppTypes, a.id]
                        : config.sharedAppTypes.filter((id) => id !== a.id),
                    })
                  }
                />
                {a.name}
              </label>
            ))}
          </div>
          <p className="hint">
            仅共享统计指标，不共享对话正文、API
            密钥或供应商配置。设备性能与网络监控尚未接入共享。
          </p>
        </section>
      )}
      <section className="panel section-gap">
        <div className="panel-head">
          <h2>按地址连接</h2>
          <p>连接为单向拉取，两台都需要合计时分别登记对方。</p>
        </div>
        <div className="filters">
          <input
            className="usage-input"
            placeholder="203.0.113.123:17890"
            aria-label="设备地址"
            value={host}
            onChange={(e) => setHost(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter" && host.trim())
                connect.mutate({ host: host.trim() });
            }}
          />
          <button
            className="button primary"
            disabled={connect.isPending || !host.trim()}
            onClick={() => connect.mutate({ host: host.trim() })}
          >
            <Plus size={14} />
            {connect.isPending ? "连接中…" : "连接设备"}
          </button>
        </div>
        {found.length > 0 && (
          <div className="usage-discovered">
            {found.map((d) => (
              <div key={d.deviceId}>
                <span className="inline">
                  <Laptop size={16} />
                  <span>
                    {d.deviceName}
                    <small className="cell-sub">
                      {d.host}:{d.port} · {d.appVersion}
                    </small>
                  </span>
                </span>
                <button
                  className="button"
                  disabled={connect.isPending}
                  onClick={() => connect.mutate({ host: d.host, port: d.port })}
                >
                  连接
                </button>
              </div>
            ))}
          </div>
        )}
      </section>
      <div className="device-grid section-gap">
        {state.data?.devices.map((d) => (
          <section className="device-card" key={d.deviceId}>
            <div className="device-card-head">
              <span className="device-icon">
                <Laptop size={22} />
              </span>
              <div>
                <h3>{d.deviceName}</h3>
                <p>
                  {d.host}:{d.port}
                </p>
              </div>
              <label className="inline">
                <input
                  className="toggle"
                  type="checkbox"
                  aria-label={"启用 " + d.deviceName}
                  disabled={enable.isPending}
                  checked={d.enabled}
                  onChange={(e) =>
                    enable.mutate({
                      deviceId: d.deviceId,
                      enabled: e.target.checked,
                    })
                  }
                />
              </label>
            </div>
            <div className="device-address">
              <span className="muted">最近成功同步</span>
              <span>{timeLabel(d.lastSyncAt)}</span>
            </div>
            {d.lastError ? (
              <p className="usage-error">{d.lastError}</p>
            ) : (
              <p className="positive inline">
                <Check size={13} />
                已缓存最近一次成功同步的快照
              </p>
            )}
            <p className="hint">
              停用后保留缓存并从汇总排除；断开连接会删除这台设备的本地副本。
            </p>
            <button
              className="button"
              disabled={disconnect.isPending}
              onClick={() => {
                if (
                  window.confirm(
                    `断开 ${d.deviceName}？其本地缓存将删除，对端原始记录不会改动。`,
                  )
                )
                  disconnect.mutate(d.deviceId);
              }}
            >
              <Trash2 size={13} />
              断开连接
            </button>
          </section>
        ))}
      </div>
      {state.data && !state.data.devices.length && (
        <div className="empty">
          <Laptop size={25} />
          <p>还没有连接其他设备。扫描局域网或填写对方地址开始同步。</p>
        </div>
      )}
    </div>
  );
}
