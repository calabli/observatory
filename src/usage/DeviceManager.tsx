import { memo, useEffect, useRef, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import {
  Check,
  ChevronDown,
  Copy,
  KeyRound,
  Laptop,
  Plus,
  Radar,
  RefreshCw,
  Trash2,
} from "lucide-react";
import { isMobilePlatform } from "../mobile";
import { usageCall } from "../native/usageClient";
import type { LanStatus, LanSettings, Discovered, NativeStatus } from "./types";
import { friendlyError, timeLabel } from "./ui";

type PairTarget = {
  deviceId: string;
  deviceName: string;
  host: string;
  port: number;
};

function formatPairCode(code: string): string {
  return code.length === 6 ? `${code.slice(0, 3)} ${code.slice(3)}` : code;
}

function DeviceManagerImpl() {
  const qc = useQueryClient();
  const isAndroid = isMobilePlatform();
  const [deviceNameDraft, setDeviceNameDraft] = useState<string | null>(null);
  const [activeTarget, setActiveTarget] = useState<PairTarget | null>(null);
  const [targetCode, setTargetCode] = useState("");
  const [manualOpen, setManualOpen] = useState(false);
  const [manualHost, setManualHost] = useState("");
  const [manualCode, setManualCode] = useState("");
  const [pairCode, setPairCode] = useState<{
    code: string;
    expiresAt: number;
  } | null>(null);
  const [copiedCode, setCopiedCode] = useState(false);
  const [found, setFound] = useState<Discovered[]>([]);
  const [hasScanned, setHasScanned] = useState(false);
  const [message, setMessage] = useState("");
  const [apiKey, setApiKey] = useState<string | null>(null);
  const followUpTimers = useRef<number[]>([]);
  const didAutoScan = useRef(false);

  useEffect(() => {
    return () => {
      for (const id of followUpTimers.current) window.clearTimeout(id);
      followUpTimers.current = [];
    };
  }, []);

  const apiKeyStatus = useQuery({
    queryKey: ["usage", "api-key-status"],
    queryFn: () => usageCall<{ configured: boolean }>("usage_api_key_status"),
    enabled: !isAndroid,
  });
  const generateApiKey = useMutation({
    mutationFn: () => usageCall<{ key: string }>("usage_api_key_rotate"),
    onSuccess: (r) => {
      setApiKey(r.key);
      void qc.invalidateQueries({ queryKey: ["usage", "api-key-status"] });
    },
  });
  const revokeApiKey = useMutation({
    mutationFn: () => usageCall("usage_api_key_revoke"),
    onSuccess: () => {
      setApiKey(null);
      void qc.invalidateQueries({ queryKey: ["usage", "api-key-status"] });
    },
  });
  const state = useQuery({
    queryKey: ["usage", "lan"],
    queryFn: () => usageCall<LanStatus>("lan_status"),
    staleTime: 10000,
    refetchInterval: 10000,
  });
  const status = useQuery({
    queryKey: ["usage", "status"],
    queryFn: () => usageCall<NativeStatus>("status"),
    staleTime: 10000,
    enabled: !isAndroid,
  });
  const incoming = useQuery({
    queryKey: ["usage", "incoming"],
    queryFn: () => usageCall<string[]>("lan_incoming_list"),
    refetchInterval: 30000,
  });

  const invalidate = async () => {
    await qc.invalidateQueries({ queryKey: ["usage"] });
  };
  const invalidateDevices = async () => {
    await Promise.all([
      qc.invalidateQueries({ queryKey: ["usage", "lan"] }),
      qc.invalidateQueries({ queryKey: ["usage", "incoming"] }),
    ]);
  };

  const latestSettings = useRef<LanSettings | null>(null);
  const pendingSaves = useRef(0);
  const saveChain = useRef<Promise<unknown>>(Promise.resolve());
  const settings = useMutation({
    mutationFn: (s: LanSettings) => {
      const run = saveChain.current
        .catch(() => undefined)
        .then(() => usageCall("lan_settings_save", s));
      saveChain.current = run;
      return run;
    },
    onMutate: (s) => {
      qc.setQueryData<LanStatus>(["usage", "lan"], (old) =>
        old ? { ...old, settings: s } : old,
      );
    },
    onError: () => {
      latestSettings.current = null;
    },
    onSettled: async () => {
      pendingSaves.current -= 1;
      if (pendingSaves.current === 0) latestSettings.current = null;
      await invalidateDevices();
    },
  });

  const discovery = useMutation({
    mutationFn: () => usageCall<Discovered[]>("lan_discover"),
    onSuccess: (r) => {
      setFound(r);
      setHasScanned(true);
    },
    onError: () => {
      setHasScanned(true);
    },
  });

  const discoverNow = discovery.mutate;
  useEffect(() => {
    if (didAutoScan.current) return;
    didAutoScan.current = true;
    discoverNow();
  }, [discoverNow]);

  const connect = useMutation({
    mutationFn: (args: { host: string; port?: number; code: string }) =>
      usageCall<{ syncPending?: boolean }>("lan_connect", args),
    onSuccess: async (r) => {
      setActiveTarget(null);
      setTargetCode("");
      setManualHost("");
      setManualCode("");
      setManualOpen(false);
      setMessage(
        r.syncPending ? "已连接，正在后台同步用量数据…" : "设备已连接。",
      );
      void invalidate();
      // Background initial sync completes a few seconds later; refresh usage caches automatically.
      for (const delay of [2000, 5000]) {
        const timer = window.setTimeout(() => {
          void qc.invalidateQueries({ queryKey: ["usage"] });
        }, delay);
        followUpTimers.current.push(timer);
      }
    },
  });

  const generateCode = useMutation({
    mutationFn: () =>
      usageCall<{ code: string; expiresAt: number }>("lan_pair_code"),
    onSuccess: (r) => {
      setPairCode(r);
      setCopiedCode(false);
      void invalidateDevices();
    },
  });

  const enable = useMutation({
    mutationFn: (args: { deviceId: string; enabled: boolean }) =>
      usageCall("lan_device_enabled", args),
    onSuccess: invalidate,
  });

  const disconnect = useMutation({
    mutationFn: (deviceId: string) =>
      usageCall<{ remoteRevoked: boolean }>("lan_disconnect", { deviceId }),
    onSuccess: async (r) => {
      setMessage(
        r.remoteRevoked
          ? "已断开连接。"
          : "本机已断开；若对方仍显示本机，可在对方列表中移除。",
      );
      await invalidate();
    },
  });

  const revokeIncoming = useMutation({
    mutationFn: (deviceId: string) =>
      usageCall("lan_incoming_revoke", { deviceId }),
    onSuccess: invalidateDevices,
  });

  const sync = useMutation({
    mutationFn: () =>
      usageCall<{ scan: { errors: string[] }; peers: { error?: string }[] }>(
        "sync",
      ),
    onSuccess: async (r) => {
      const failures =
        r.scan.errors.length + r.peers.filter((peer) => peer.error).length;
      setMessage(
        failures ? `同步未全部完成（${failures} 项失败）。` : "已完成同步。",
      );
      await invalidate();
    },
  });

  const error =
    generateApiKey.error ||
    revokeApiKey.error ||
    apiKeyStatus.error ||
    state.error ||
    status.error ||
    settings.error ||
    discovery.error ||
    connect.error ||
    incoming.error ||
    revokeIncoming.error ||
    generateCode.error ||
    enable.error ||
    disconnect.error ||
    sync.error;

  const config = state.data?.settings;
  const devices = state.data?.devices ?? [];
  const pairedIds = new Set(
    devices.filter((d) => d.paired).map((d) => d.deviceId),
  );

  const set = (
    patch:
      | Partial<LanSettings>
      | ((current: LanSettings) => Partial<LanSettings>),
  ) => {
    const base = latestSettings.current ?? config;
    if (!base) return;
    const next = {
      ...base,
      ...(typeof patch === "function" ? patch(base) : patch),
    };
    latestSettings.current = next;
    pendingSaves.current += 1;
    settings.mutate(next);
  };

  const copyPairCode = async () => {
    if (!pairCode) return;
    try {
      await navigator.clipboard.writeText(pairCode.code);
      setCopiedCode(true);
      window.setTimeout(() => setCopiedCode(false), 1800);
    } catch {
      // ignore clipboard errors
    }
  };

  const submitTargetConnect = () => {
    if (!activeTarget || targetCode.length !== 6 || connect.isPending) return;
    setMessage("");
    connect.mutate({
      host: activeTarget.host,
      port: activeTarget.port,
      code: targetCode,
    });
  };

  const submitManualConnect = () => {
    if (!manualHost.trim() || manualCode.length !== 6 || connect.isPending)
      return;
    setMessage("");
    connect.mutate({
      host: manualHost.trim(),
      code: manualCode,
    });
  };

  return (
    <div className="native-devices dm-root">
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

      {/* 1. Nearby / Auto-discovered LAN devices */}
      <section className="dm-card">
        <div className="dm-card-head">
          <div>
            <h2>附近设备</h2>
            <p>同一局域网下自动发现，点击连接并输入对方 6 位配对码即可</p>
          </div>
          <button
            type="button"
            className="button dm-compact-btn"
            disabled={discovery.isPending}
            onClick={() => {
              setMessage("");
              discovery.mutate();
            }}
          >
            <Radar size={14} />
            {discovery.isPending ? "扫描中…" : "重新扫描"}
          </button>
        </div>

        {found.length > 0 ? (
          <div className="dm-list">
            {found.map((d) => {
              const isPaired = pairedIds.has(d.deviceId);
              const isActive = activeTarget?.deviceId === d.deviceId;
              return (
                <div className="dm-item" key={d.deviceId}>
                  <div className="dm-item-row">
                    <span className="dm-item-icon">
                      <Laptop size={16} />
                    </span>
                    <div className="dm-item-info">
                      <strong>{d.deviceName}</strong>
                      <small>
                        {d.host}:{d.port}
                      </small>
                    </div>
                    {isPaired ? (
                      <span className="dm-badge-connected">
                        <Check size={12} />
                        已连接
                      </span>
                    ) : (
                      <button
                        type="button"
                        className={`button dm-compact-btn ${isActive ? "" : "primary"}`}
                        onClick={() => {
                          if (isActive) {
                            setActiveTarget(null);
                            setTargetCode("");
                          } else {
                            setActiveTarget({
                              deviceId: d.deviceId,
                              deviceName: d.deviceName,
                              host: d.host,
                              port: d.port,
                            });
                            setTargetCode("");
                            setManualOpen(false);
                          }
                        }}
                      >
                        {isActive ? "收起" : "连接"}
                      </button>
                    )}
                  </div>

                  {isActive && (
                    <div className="dm-pair-inline">
                      <p className="dm-pair-tip">
                        请在「{d.deviceName}」的观测台点击「生成配对码」后输入：
                      </p>
                      <div className="dm-pair-inputs">
                        <input
                          className="usage-input dm-code-input"
                          inputMode="numeric"
                          maxLength={6}
                          autoFocus
                          placeholder="6 位配对码"
                          aria-label={`${d.deviceName} 的配对码`}
                          value={targetCode}
                          onChange={(e) =>
                            setTargetCode(
                              e.target.value.replace(/\D/g, "").slice(0, 6),
                            )
                          }
                          onKeyDown={(e) => {
                            if (e.key === "Enter") submitTargetConnect();
                          }}
                        />
                        <button
                          type="button"
                          className="button primary dm-compact-btn"
                          disabled={
                            connect.isPending || targetCode.length !== 6
                          }
                          onClick={submitTargetConnect}
                        >
                          {connect.isPending ? "连接中…" : "确认连接"}
                        </button>
                      </div>
                    </div>
                  )}
                </div>
              );
            })}
          </div>
        ) : (
          <p className="dm-empty-inline">
            {discovery.isPending
              ? "正在扫描局域网内的观测台设备…"
              : hasScanned
                ? "暂未自动发现设备，请确认电脑已打开观测台并连在同一 Wi-Fi。"
                : "准备扫描局域网设备…"}
          </p>
        )}

        <div className="dm-manual-wrap">
          <button
            type="button"
            className="text-button dm-manual-toggle"
            onClick={() => {
              setManualOpen((v) => !v);
              setActiveTarget(null);
            }}
          >
            <Plus size={13} />
            {manualOpen ? "收起手动连接" : "手动输入 IP 地址连接"}
          </button>

          {manualOpen && (
            <div className="dm-manual-form">
              <input
                className="usage-input"
                placeholder="例如 192.168.1.10:17891"
                aria-label="设备地址"
                value={manualHost}
                onChange={(e) => setManualHost(e.target.value)}
              />
              <div className="dm-pair-inputs">
                <input
                  className="usage-input dm-code-input"
                  inputMode="numeric"
                  maxLength={6}
                  placeholder="6 位配对码"
                  aria-label="对方的配对码"
                  value={manualCode}
                  onChange={(e) =>
                    setManualCode(e.target.value.replace(/\D/g, "").slice(0, 6))
                  }
                  onKeyDown={(e) => {
                    if (e.key === "Enter") submitManualConnect();
                  }}
                />
                <button
                  type="button"
                  className="button primary dm-compact-btn"
                  disabled={
                    connect.isPending ||
                    !manualHost.trim() ||
                    manualCode.length !== 6
                  }
                  onClick={submitManualConnect}
                >
                  {connect.isPending ? "连接中…" : "连接"}
                </button>
              </div>
            </div>
          )}
        </div>
      </section>

      {/* 2. Connected Devices */}
      {devices.length > 0 && (
        <section className="dm-card">
          <div className="dm-card-head">
            <div>
              <h2>已连接设备（{devices.length}）</h2>
              <p>数据会自动增量同步，停用后仅暂停拉取但保留历史记录</p>
            </div>
            <button
              type="button"
              className="button dm-compact-btn"
              disabled={sync.isPending}
              onClick={() => {
                setMessage("");
                sync.mutate();
              }}
            >
              <RefreshCw size={14} />
              {sync.isPending ? "同步中…" : "立即同步"}
            </button>
          </div>

          <div className="dm-list">
            {devices.map((d) => (
              <div className="dm-item" key={d.deviceId}>
                <div className="dm-item-row">
                  <span className="dm-item-icon">
                    <Laptop size={16} />
                  </span>
                  <div className="dm-item-info">
                    <strong>{d.deviceName}</strong>
                    <small>
                      {d.host}:{d.port} ·{" "}
                      {!d.paired ? (
                        <span className="dm-status-err">需要重新配对</span>
                      ) : d.lastError ? (
                        <span className="dm-status-err">{d.lastError}</span>
                      ) : d.lastSyncAt ? (
                        <span className="dm-status-ok">
                          已同步 · {timeLabel(d.lastSyncAt)}
                        </span>
                      ) : (
                        <span>正在首次同步…</span>
                      )}
                    </small>
                  </div>
                  <div className="dm-item-actions">
                    <input
                      className="toggle"
                      type="checkbox"
                      aria-label={`启用 ${d.deviceName}`}
                      disabled={enable.isPending}
                      checked={d.enabled}
                      onChange={(e) =>
                        enable.mutate({
                          deviceId: d.deviceId,
                          enabled: e.target.checked,
                        })
                      }
                    />
                    <button
                      type="button"
                      className="button dm-icon-danger"
                      title={`断开 ${d.deviceName}`}
                      aria-label={`断开 ${d.deviceName}`}
                      disabled={disconnect.isPending}
                      onClick={() => {
                        if (
                          window.confirm(
                            `断开 ${d.deviceName}？本地缓存将被移除，不影响对方设备数据。`,
                          )
                        ) {
                          disconnect.mutate(d.deviceId);
                        }
                      }}
                    >
                      <Trash2 size={14} />
                    </button>
                  </div>
                </div>
              </div>
            ))}
          </div>
        </section>
      )}

      {/* 3. Local Pairing Code (1-click, auto-enables LAN listener) */}
      <section className="dm-card">
        <div className="dm-card-head">
          <div>
            <h2>本机配对码</h2>
            <p>
              供其他设备主动连接本机（
              {state.data?.local.deviceName ?? "本机"} · {config?.port ?? 17891}
              ）
            </p>
          </div>
          <button
            type="button"
            className="button dm-compact-btn"
            disabled={generateCode.isPending}
            onClick={() => {
              setMessage("");
              generateCode.mutate();
            }}
          >
            <KeyRound size={14} />
            {generateCode.isPending
              ? "生成中…"
              : pairCode
                ? "重新生成"
                : "生成配对码"}
          </button>
        </div>

        {pairCode && (
          <div className="dm-pair-code-box">
            <div className="dm-pair-code-value">
              <strong>{formatPairCode(pairCode.code)}</strong>
              <small>一次有效 · 有效至 {timeLabel(pairCode.expiresAt)}</small>
            </div>
            <button
              type="button"
              className="button dm-compact-btn"
              onClick={copyPairCode}
            >
              {copiedCode ? <Check size={14} /> : <Copy size={14} />}
              {copiedCode ? "已复制" : "复制"}
            </button>
          </div>
        )}
      </section>

      {/* 4. Advanced Settings (collapsed by default) */}
      {config && (
        <details className="dm-advanced">
          <summary className="dm-advanced-summary">
            <span>高级设置</span>
            <ChevronDown size={15} className="dm-advanced-chevron" />
          </summary>

          <div className="dm-advanced-body">
            {!isAndroid && (
              <div className="dm-adv-row">
                <div>
                  <strong>局域网共享开关</strong>
                  <small>关闭后其他设备将无法拉取本机用量</small>
                </div>
                <label className="inline">
                  <span>{config.enabled ? "已开启" : "已关闭"}</span>
                  <input
                    className="toggle"
                    type="checkbox"
                    aria-label="开启本机用量共享"
                    checked={config.enabled}
                    onChange={(e) => set({ enabled: e.target.checked })}
                  />
                </label>
              </div>
            )}

            <div className="usage-lan-fields">
              <label>
                本机显示名称
                <input
                  value={deviceNameDraft ?? config.deviceName}
                  placeholder={state.data?.local.deviceName}
                  onChange={(e) => setDeviceNameDraft(e.target.value)}
                  onBlur={(e) => {
                    setDeviceNameDraft(null);
                    if (e.target.value !== config.deviceName)
                      set({ deviceName: e.target.value });
                  }}
                />
              </label>
              {!isAndroid && (
                <label>
                  监听端口
                  <input
                    type="number"
                    min="1024"
                    max="65535"
                    defaultValue={config.port}
                    key={config.port}
                    onBlur={(e) => {
                      const port = Number(e.target.value);
                      if (
                        Number.isInteger(port) &&
                        port >= 1024 &&
                        port <= 65535
                      )
                        set({ port });
                    }}
                  />
                </label>
              )}
              <label className="usage-check">
                <input
                  type="checkbox"
                  checked={config.autoSync}
                  onChange={(e) => set({ autoSync: e.target.checked })}
                />
                后台自动同步
              </label>
            </div>

            {!isAndroid && (
              <>
                <div className="usage-section-title">
                  <h3>允许共享的应用</h3>
                  <button
                    type="button"
                    className="text-button"
                    onClick={() =>
                      set({
                        sharedAppTypes: (status.data?.sources ?? []).map(
                          (a) => a.id,
                        ),
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
                        checked={config.sharedAppTypes.includes(a.id)}
                        onChange={(e) => {
                          const checked = e.target.checked;
                          set((current) => ({
                            sharedAppTypes: checked
                              ? [
                                  ...current.sharedAppTypes.filter(
                                    (id) => id !== a.id,
                                  ),
                                  a.id,
                                ]
                              : current.sharedAppTypes.filter(
                                  (id) => id !== a.id,
                                ),
                          }));
                        }}
                      />
                      {a.name}
                    </label>
                  ))}
                </div>
              </>
            )}

            {incoming.data && incoming.data.length > 0 && (
              <div className="dm-adv-block">
                <h3>已授权访问本机的设备</h3>
                {incoming.data.map((id) => (
                  <div className="dm-adv-row" key={id}>
                    <span>
                      {devices.find((d) => d.deviceId === id)?.deviceName ?? id}
                    </span>
                    <button
                      type="button"
                      className="button dm-compact-btn"
                      disabled={revokeIncoming.isPending}
                      onClick={() => revokeIncoming.mutate(id)}
                    >
                      撤销授权
                    </button>
                  </div>
                ))}
              </div>
            )}

            {!isAndroid && (
              <div className="dm-adv-block">
                <h3>只读用量 HTTP 接口（TinyPane 等组件）</h3>
                <div className="filters">
                  <button
                    type="button"
                    className="button dm-compact-btn"
                    disabled={generateApiKey.isPending}
                    onClick={() => {
                      if (
                        !apiKeyStatus.data?.configured ||
                        window.confirm("重置后原 API 密钥将立即失效，继续？")
                      )
                        generateApiKey.mutate();
                    }}
                  >
                    {apiKeyStatus.data?.configured
                      ? "重置 API 密钥"
                      : "生成 API 密钥"}
                  </button>
                  {apiKeyStatus.data?.configured && (
                    <button
                      type="button"
                      className="button dm-compact-btn"
                      disabled={revokeApiKey.isPending}
                      onClick={() => revokeApiKey.mutate()}
                    >
                      停用密钥
                    </button>
                  )}
                </div>
                {apiKey && (
                  <label className="dm-apikey-field">
                    新 API 密钥（仅显示一次）
                    <input
                      className="usage-input"
                      aria-label="新 API 密钥"
                      readOnly
                      value={apiKey}
                      onFocus={(e) => e.target.select()}
                    />
                  </label>
                )}
                <code className="dm-code-sample">
                  http://127.0.0.1:{config.port}
                  /api/v1/usage/summary?days=7&amp;groupBy=day
                </code>
              </div>
            )}
          </div>
        </details>
      )}
    </div>
  );
}

export const DeviceManager = memo(DeviceManagerImpl);
