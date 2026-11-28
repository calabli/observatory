import { MultiDeviceScanCoordinator, type ScanBatch } from "./multiDeviceScan";
import {
  bounds,
  dateInput,
  exportPage,
  loadView,
  multiScanEvent,
  type Props,
  type Range,
  saveView,
  type ScanFeedback,
  type UsageScope,
} from "./usageShared";
import { memo, useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import {
  customTimeRange,
  parseRangePreset,
  RANGE_PRESETS,
  USAGE_RANGES,
} from "./timeRange";
import { rankApps, type RankingMetric } from "./appRanking";
import { usageCall } from "../native/usageClient";
import {
  type Dashboard,
  type LanStatus,
  type NativeStatus,
  type UsageFilters,
} from "./types";
import type { UsageSummary } from "../types/usage";
import { useLocalDayKey } from "./useLocalDayKey";
import { formatMoney, formatTokens, friendlyError, UsageDialog } from "./ui";
import { syncModelsDevPricingOnStartup } from "../lib/modelsDevAutoSync";
import { modelDisplayName } from "./modelDisplay";
import {
  computeCycleWindow,
  DEFAULT_CYCLE_PLANS,
  normalizeCyclePlan,
  periodDeltaPct,
  type UsageCyclePlan,
} from "./cycles";
import { CycleNavigator } from "./CycleNavigator";
import { CycleConfigModal } from "./CycleConfigModal";
import { CycleHistoryPanel, type CycleHistoryEntry } from "./CycleHistoryPanel";
import {
  Activity,
  ArrowLeft,
  ArrowRight,
  Boxes,
  Database,
  Download,
  FileDown,
  Laptop,
  Layers3,
  Monitor,
  RefreshCw,
  Settings2,
  Tags,
  TriangleAlert,
} from "lucide-react";
import { AppSelect } from "./AppSelect";
import { DateTimeFields } from "./DateTimeFields";
import { MultiDeviceScanPanel } from "./MultiDeviceScanPanel";
import {
  ActivityGrid,
  LogRow,
  Metric,
  ModelSpeedChart,
  StatsTable,
} from "./UsagePanels";
import { UsageChart } from "./UsageChart";
import { AppGlyph } from "./AppGlyph";
import { PricingManager } from "./PricingManager";
import { DeviceManager } from "./DeviceManager";
import { RequestDetail, SourcesDialog } from "./UsageDialogs";
import { isMobilePlatform } from "../mobile";

let activeMultiScan = false;

let latestMultiScan: ScanBatch | null = null;

function saveMultiScan(batch: ScanBatch) {
  latestMultiScan = batch;
  try {
    localStorage.setItem(
      "observatory.usage.lastMultiScan",
      JSON.stringify(batch),
    );
  } catch {
    // When storage is unavailable the scan result is kept for this session
    // only.
  }
  window.dispatchEvent(new Event(multiScanEvent));
}

function loadMultiScan(): ScanBatch | null {
  if (latestMultiScan) return latestMultiScan;
  try {
    const batch = JSON.parse(
      localStorage.getItem("observatory.usage.lastMultiScan") || "null",
    ) as ScanBatch | null;
    if (!batch || !Array.isArray(batch.devices)) return null;
    if (batch.status === "running") {
      batch.devices = batch.devices.map((d) =>
        ["waiting", "scanning", "syncing"].includes(d.state)
          ? { ...d, state: "unknown", error: "应用重启，无法确认任务状态" }
          : d,
      );
      batch.status = "incomplete";
    }
    return batch;
  } catch {
    return null;
  }
}

function UsageWorkspaceImpl({
  interval,
  onRefreshReady,
  onBusy,
  onStatus,
}: Props) {
  const saved = useMemo(loadView, []);
  const qc = useQueryClient();
  const [scope, setScope] = useState<UsageScope>(() => {
    if (saved.scope === "multi" || saved.scope === "local") return saved.scope;
    return isMobilePlatform() ? "multi" : "local";
  });
  const [selectedDevices, setSelectedDevices] = useState<string[] | null>(null);
  const [range, setRange] = useState<Range>(() =>
    saved.range === "all" || saved.range === "custom" || saved.range === "cycle"
      ? saved.range
      : (parseRangePreset(saved.range, USAGE_RANGES, true) ?? "today"),
  );
  const [cyclePlanId, setCyclePlanId] = useState(saved.cyclePlanId ?? "");
  const [cycleOffset, setCycleOffset] = useState(0);
  const [cycleNow, setCycleNow] = useState(() => Date.now());
  const [manualAppOverride, setManualAppOverride] = useState(false);
  const [cycleModal, setCycleModal] = useState<{
    open: true;
    planId: string | null;
  } | null>(null);
  const [cycleSaveError, setCycleSaveError] = useState("");
  const [app, setApp] = useState(saved.app ?? "");
  const [model, setModel] = useState(saved.model ?? "");
  const [provider, setProvider] = useState(saved.provider ?? "");
  const [customStart, setCustomStart] = useState(dateInput() + "T00:00:00");
  const [customEnd, setCustomEnd] = useState(dateInput() + "T23:59:59");
  const [page, setPage] = useState(1);
  const [size, setSize] = useState(20);
  const [statusCode, setStatusCode] = useState("");
  const [tab, setTab] = useState("logs");
  const [metric, setMetric] = useState<RankingMetric>("tokens");
  const [chartType, setChartType] = useState<"bar" | "area" | "blocks">("bar");
  const [modal, setModal] = useState<"pricing" | "sources" | "devices" | null>(
    null,
  );
  const [detail, setDetail] = useState<string | null>(null);
  const [notice, setNotice] = useState("");
  const [scanBusy, setScanBusy] = useState(false);
  const [scanFeedback, setScanFeedback] = useState<
    Partial<Record<UsageScope, ScanFeedback>>
  >({});
  const currentScan = scanFeedback[scope];
  const [multiBatch, setMultiBatch] = useState<ScanBatch | null>(loadMultiScan);
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
  // Remote and background scans run in the backend, independently of this
  // window. Watch their cheap in-memory revision even in manual refresh mode.
  // Stop polling while the window is hidden/minimized; the first round after
  // it becomes visible makes up for the missed ones.
  const scanRevision = useQuery({
    queryKey: ["usage", "scan-revision"],
    queryFn: () =>
      usageCall<{ revision: number; epoch: number }>("scan_revision"),
    enabled: !!status.data,
    refetchInterval: () =>
      document.visibilityState === "visible" ? 2000 : false,
    retry: false,
  });
  // The invalidation gate uses epoch (it changes only on any local row-level
  // write): an empty scan increments revision every round without producing
  // data, so invalidating by revision would recompute the full aggregate
  // every 30 seconds while idle. LAN snapshot imports and remote scan imports
  // are row-level writes too, so epoch covers them as well.
  useEffect(() => {
    if (scanRevision.data?.epoch === undefined) return;
    void qc.invalidateQueries({ queryKey: ["usage", "status"] });
    void qc.invalidateQueries({ queryKey: ["usage", "dashboard"] });
    void qc.invalidateQueries({ queryKey: ["usage", "cycle-history"] });
    void qc.invalidateQueries({ queryKey: ["usage", "insights"] });
  }, [scanRevision.data?.epoch, qc]);
  useEffect(() => {
    if (range !== "cycle") return;
    const tick = () => {
      if (document.visibilityState === "visible") setCycleNow(Date.now());
    };
    const timer = setInterval(tick, 60000);
    document.addEventListener("visibilitychange", tick);
    window.addEventListener("focus", tick);
    return () => {
      clearInterval(timer);
      document.removeEventListener("visibilitychange", tick);
      window.removeEventListener("focus", tick);
    };
  }, [range]);
  const cyclesQuery = useQuery({
    queryKey: ["usage", "cycles"],
    queryFn: () => usageCall<Partial<UsageCyclePlan>[]>("usage_cycles_get"),
    enabled: !!status.data,
    staleTime: 60000,
    retry: false,
  });
  const saveCycles = useMutation({
    mutationFn: (plans: UsageCyclePlan[]) =>
      usageCall("usage_cycles_save", { plans }),
    onSuccess: (_res, plans) => {
      qc.setQueryData(["usage", "cycles"], plans);
      void qc.invalidateQueries({ queryKey: ["usage", "cycles"] });
    },
  });
  const cyclePlans = useMemo<UsageCyclePlan[]>(() => {
    const raw = cyclesQuery.data;
    if (Array.isArray(raw) && raw.length > 0) {
      return raw.map((item, i) => normalizeCyclePlan(item, i));
    }
    return DEFAULT_CYCLE_PLANS;
  }, [cyclesQuery.data]);
  const activeCyclePlan = useMemo(
    () => cyclePlans.find((p) => p.id === cyclePlanId) ?? cyclePlans[0],
    [cyclePlans, cyclePlanId],
  );
  const activeCycleWindow = useMemo(
    () => computeCycleWindow(activeCyclePlan, new Date(cycleNow), cycleOffset),
    [activeCyclePlan, cycleNow, cycleOffset],
  );
  const lan = useQuery({
    queryKey: ["usage", "lan"],
    queryFn: () => usageCall<LanStatus>("lan_status"),
    enabled: !!status.data,
    staleTime: 10000,
    refetchInterval: () =>
      document.visibilityState === "visible" ? 10000 : false,
  });
  const [dayKey, syncDayKey] = useLocalDayKey();
  const period = useMemo(
    () =>
      range === "cycle"
        ? {
            startDate: activeCycleWindow.startDate,
            endDate: activeCycleWindow.endDate,
          }
        : bounds(range, customStart, customEnd, dayKey),
    [range, activeCycleWindow, customStart, customEnd, dayKey],
  );
  const validPeriod =
    range !== "custom" || customTimeRange(customStart, customEnd) !== null;
  // Disconnected or disabled devices must not stay in the multi-select:
  // otherwise every dashboard request fails with "device does not exist or
  // is not enabled" and the page stays stuck in an error state. Keep the
  // value as-is until the device list arrives; fall back to "all devices"
  // when the filtered list ends up empty.
  const activeDevices = useMemo(() => {
    if (selectedDevices === null || !lan.data) return selectedDevices;
    const available = new Set([
      "local",
      ...lan.data.devices
        .filter((d) => d.enabled && d.paired)
        .map((d) => d.deviceId),
    ]);
    const kept = selectedDevices.filter((id) => available.has(id));
    return kept.length ? kept : null;
  }, [selectedDevices, lan.data]);
  const filters: UsageFilters = {
    ...period,
    appType: app || undefined,
    model: model || undefined,
    providerName: provider || undefined,
    statusCode: statusCode ? Number(statusCode) : undefined,
    multiDevice: scope === "multi",
    deviceIds: scope === "multi" ? (activeDevices ?? undefined) : undefined,
    page,
    pageSize: size,
  };
  const dashboard = useQuery({
    queryKey: ["usage", "dashboard", range, dayKey, filters],
    queryFn: () => {
      const liveBounds =
        range === "custom" || range === "cycle"
          ? period
          : bounds(range, customStart, customEnd);
      return usageCall<Dashboard>("dashboard", {
        ...filters,
        ...liveBounds,
      });
    },
    enabled: !!status.data && !!validPeriod,
    staleTime: 5000,
    refetchOnWindowFocus: false,
    retry: false,
  });
  const historyWindows = useMemo(() => {
    const offsets =
      cycleOffset >= -2
        ? [0, -1, -2, -3]
        : [cycleOffset, cycleOffset - 1, cycleOffset - 2, cycleOffset - 3];
    const nowDate = new Date(cycleNow);
    return offsets.map((off) => ({
      offset: off,
      window: computeCycleWindow(activeCyclePlan, nowDate, off),
    }));
  }, [activeCyclePlan, cycleNow, cycleOffset]);
  const historyKey = historyWindows
    .map((w) => `${w.window.startDate}-${w.window.endDate}`)
    .join(",");
  const cycleHistory = useQuery({
    queryKey: [
      "usage",
      "cycle-history",
      activeCyclePlan.id,
      scope,
      activeDevices,
      app,
      provider,
      model,
      historyKey,
    ],
    queryFn: () =>
      usageCall<UsageSummary[]>("usage_cycle_history", {
        windows: historyWindows.map((w) => ({
          startDate: w.window.startDate,
          endDate: w.window.endDate,
        })),
        appType: app || undefined,
        providerName: provider || undefined,
        model: model || undefined,
        multiDevice: scope === "multi",
        deviceIds: scope === "multi" ? (activeDevices ?? undefined) : undefined,
      }),
    enabled: !!status.data && range === "cycle",
    staleTime: 5000,
    refetchOnWindowFocus: false,
    retry: false,
  });
  const historyEntries = useMemo<CycleHistoryEntry[]>(
    () =>
      historyWindows.map((item, idx) => ({
        offset: item.offset,
        window: item.window,
        summary: cycleHistory.data?.[idx],
      })),
    [historyWindows, cycleHistory.data],
  );
  const prevCycleSummary = useMemo(() => {
    const idx = historyWindows.findIndex((w) => w.offset === cycleOffset - 1);
    return idx >= 0 ? cycleHistory.data?.[idx] : undefined;
  }, [historyWindows, cycleHistory.data, cycleOffset]);
  const peerRevision = JSON.stringify(
    lan.data?.devices.map((d) => [
      d.deviceId,
      d.enabled,
      d.paired,
      d.lastSyncAt,
    ]) ?? [],
  );
  useEffect(() => {
    if (scope === "multi")
      void qc.invalidateQueries({ queryKey: ["usage", "dashboard"] });
  }, [peerRevision, qc, scope]);
  // Device names for peer sync errors, read from a ref so the refresh
  // callback (and the timers keyed on it) is not rebuilt on every LAN poll.
  const peerNames = useRef(new Map<string, string>());
  peerNames.current = new Map(
    (lan.data?.devices ?? []).map((d) => [d.deviceId, d.deviceName]),
  );
  /**
   * "This machine" scans only local logs and never contacts paired devices,
   * so an offline peer cannot surface as a local error. Automatic local
   * refreshes do not scan at all: the engine's background loop already scans
   * on the same interval, and the revision poll picks up its results.
   * The multi-device scope keeps scanning locally and pulling every peer.
   */
  const refresh = useCallback(
    async (automatic = false) => {
      syncDayKey();
      if (scanRef.current) return scanRef.current;
      if (activeMultiScan) return;
      if (scope !== "multi" && automatic) {
        await qc.invalidateQueries({ queryKey: ["usage"] });
        return;
      }
      const task = (async () => {
        setScanBusy(true);
        onBusy?.(true);
        try {
          if (scope === "multi") {
            const r = await usageCall<{
              scan: { imported: number; errors: string[] };
              peers: { deviceId: string; error?: string }[];
            }>("sync", { automatic });
            if (mounted.current) {
              const errors = [
                ...r.scan.errors,
                ...r.peers.flatMap((p) =>
                  p.error
                    ? [
                        `设备 ${peerNames.current.get(p.deviceId) ?? p.deviceId} 未连上，保留上次同步的数据：${p.error}`,
                      ]
                    : [],
                ),
              ];
              setScanFeedback((previous) => ({
                ...previous,
                multi: {
                  errors,
                  notice: `本机扫描完成，新增或更新 ${r.scan.imported} 条记录；已同步 ${r.peers.filter((p) => !p.error).length}/${r.peers.length} 台设备。`,
                },
              }));
            }
          } else {
            const r = await usageCall<{ imported: number; errors: string[] }>(
              "scan_local_only",
            );
            if (mounted.current) {
              setScanFeedback((previous) => ({
                ...previous,
                local: {
                  errors: r.errors,
                  notice: `本机扫描完成，新增或更新 ${r.imported} 条记录。`,
                },
              }));
            }
          }
          await qc.invalidateQueries({ queryKey: ["usage"] });
        } catch (e) {
          if (mounted.current) {
            setScanFeedback((previous) => ({
              ...previous,
              [scope]: { notice: "扫描失败：" + friendlyError(e), errors: [] },
            }));
          }
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
    [qc, onBusy, scope, syncDayKey],
  );
  const refreshMulti = async () => {
    syncDayKey();
    if (scanRef.current || activeMultiScan || !status.data || !lan.data) return;
    activeMultiScan = true;
    const local = status.data.device;
    const key = crypto.randomUUID();
    const task = (async () => {
      setScanBusy(true);
      onBusy?.(true);
      setScanFeedback((previous) => ({ ...previous, multi: undefined }));
      try {
        const coordinator = new MultiDeviceScanCoordinator(
          {
            scanLocal: () => usageCall("scan_local_only"),
            startRemote: async (deviceId, batchId) => {
              const r = await usageCall<{ jobId: string }>(
                "scan_remote_start",
                { deviceId, key: batchId },
              );
              return { jobId: r.jobId };
            },
            waitRemote: async (deviceId) => {
              const deadline = Date.now() + 300000;
              while (Date.now() < deadline) {
                const job = await usageCall<{
                  state: string;
                  imported?: number;
                  errors: string[];
                }>("scan_remote_status", { deviceId, key });
                if (
                  job.state === "completed" ||
                  job.state === "partial_failure"
                )
                  return { imported: job.imported, errors: job.errors };
                if (job.state === "failed")
                  throw new Error(job.errors.join("；") || "远端扫描失败");
                await new Promise((resolve) => setTimeout(resolve, 1500));
              }
              throw Object.assign(new Error("等待超时，远端扫描可能仍在进行"), {
                code: "timeout",
              });
            },
            syncRemote: (deviceId) =>
              usageCall("scan_remote_sync", { deviceId }),
          },
          saveMultiScan,
        );
        await coordinator.start(key, [
          {
            deviceId: local.deviceId,
            deviceName: local.deviceName,
            local: true,
            enabled: true,
            paired: true,
          },
          ...lan.data!.devices.map((d) => ({
            deviceId: d.deviceId,
            deviceName: d.deviceName,
            local: false,
            enabled: d.enabled,
            paired: d.paired === true,
            lastSyncAt: d.lastSyncAt,
          })),
        ]);
        await qc.invalidateQueries({ queryKey: ["usage"] });
      } catch (e) {
        if (mounted.current) {
          setScanFeedback((previous) => ({
            ...previous,
            multi: { notice: "更新失败：" + friendlyError(e), errors: [] },
          }));
        }
      } finally {
        scanRef.current = null;
        activeMultiScan = false;
        window.dispatchEvent(new Event(multiScanEvent));
        if (mounted.current) setScanBusy(false);
        onBusy?.(false);
      }
    })();
    scanRef.current = task;
    await task;
  };
  useEffect(() => {
    const update = () => {
      setMultiBatch(loadMultiScan());
      setScanBusy(activeMultiScan);
    };
    window.addEventListener(multiScanEvent, update);
    update();
    return () => window.removeEventListener(multiScanEvent, update);
  }, []);
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
      const detected = status.data.ccSwitchPath;
      // Fill in the detected path only when it is empty, never override
      // user input.
      setImportPath((path) => path || detected);
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
    // Rebuild the timer only when the device or the import error changes:
    // status.data is a new object on every poll, so using the whole thing as
    // a dependency would reset the timer over and over.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [
    interval,
    status.data?.device.deviceId,
    status.data?.initialImportError,
    refresh,
  ]);
  useEffect(
    () =>
      saveView({
        scope,
        range,
        app,
        provider,
        model,
        cyclePlanId: activeCyclePlan.id,
      }),
    [scope, range, app, provider, model, activeCyclePlan.id],
  );
  const change = (fn: () => void) => {
    setPage(1);
    setDetail(null);
    fn();
  };
  const handleSelectCyclePlan = (id: string) => {
    const next = cyclePlans.find((p) => p.id === id);
    if (!next) return;
    change(() => {
      setCyclePlanId(next.id);
      setCycleOffset(0);
      setManualAppOverride(false);
      if (next.appType) {
        setApp(next.appType);
        setProvider("");
        setModel("");
      }
    });
  };
  const handleSaveCyclePlan = (savedPlan: UsageCyclePlan) => {
    setCycleSaveError("");
    const exists = cyclePlans.some((p) => p.id === savedPlan.id);
    const nextPlans = exists
      ? cyclePlans.map((p) => (p.id === savedPlan.id ? savedPlan : p))
      : [...cyclePlans, savedPlan];
    saveCycles.mutate(nextPlans, {
      onSuccess: () => {
        change(() => {
          setRange("cycle");
          setCyclePlanId(savedPlan.id);
          setCycleOffset(0);
          setManualAppOverride(false);
          if (savedPlan.appType) {
            setApp(savedPlan.appType);
            setProvider("");
            setModel("");
          }
        });
        setCycleModal(null);
      },
      onError: (e) => {
        setCycleSaveError("保存周期方案失败：" + friendlyError(e));
      },
    });
  };
  const handleDeleteCyclePlan = (id: string) => {
    setCycleSaveError("");
    const nextPlans = cyclePlans.filter((p) => p.id !== id);
    if (!nextPlans.length) return;
    saveCycles.mutate(nextPlans, {
      onSuccess: () => {
        change(() => {
          setCyclePlanId(nextPlans[0].id);
          setCycleOffset(0);
          setManualAppOverride(false);
        });
        setCycleModal(null);
      },
      onError: (e) => {
        setCycleSaveError("删除周期方案失败：" + friendlyError(e));
      },
    });
  };
  const makeDelta = (
    current: number | undefined,
    previous: number | undefined,
    invertGood = false,
  ): { text: string; tone: "good" | "warn" | "muted" } | undefined => {
    if (range !== "cycle" || current === undefined || previous === undefined)
      return undefined;
    const pct = periodDeltaPct(current, previous);
    if (pct === null) return undefined;
    const sign = pct >= 0 ? "+" : "";
    const warn = invertGood ? pct < -5 : pct > 15;
    const good = invertGood ? pct >= 0 : pct <= 0;
    return {
      text: `较上周期 ${sign}${pct.toFixed(1)}%`,
      tone: warn ? "warn" : good ? "good" : "muted",
    };
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
  // The model dropdown can hold hundreds of entries: twenty thousand regex
  // calls per render (two per entry) is not negligible, so cache until the
  // options change.
  const modelOptions = useMemo(
    () =>
      (dashboard.data?.options.models ?? []).map((m) => {
        const display = modelDisplayName(m);
        return display === m ? m : `${display} (${m})`;
      }),
    [dashboard.data?.options.models],
  );
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
      <div className="usage-android-banner">
        <div className="usage-android-banner-info">
          <strong>
            {lan.data?.devices.filter((d) => d.enabled && d.paired).length
              ? `已连接 ${lan.data.devices.filter((d) => d.enabled && d.paired).length} 台局域网电脑`
              : "局域网伴侣 · 未配对电脑"}
          </strong>
          <small>支持按时间范围、平台/应用、供应商与模型查询</small>
        </div>
        <div className="usage-android-banner-actions">
          <button
            type="button"
            className="button"
            onClick={() => setModal("devices")}
          >
            配对
          </button>
          <button
            type="button"
            className="button primary"
            disabled={
              scanBusy || !status.data || (scope === "multi" && !lan.data)
            }
            onClick={() =>
              void (scope === "multi" ? refreshMulti() : refresh()).catch(
                () => {},
              )
            }
          >
            <RefreshCw size={12} />
            {scanBusy ? "同步中" : "同步"}
          </button>
        </div>
      </div>
      <div
        className="usage-mobile-range-bar"
        role="tablist"
        aria-label="快捷时间范围"
      >
        {(
          [
            ...USAGE_RANGES.map(
              (key) => [key, RANGE_PRESETS[key].label] as const,
            ),
            ["all", "全部"] as const,
            ["cycle", "按周期"] as const,
            ["custom", "自定义"] as const,
          ] as const
        ).map(([key, label]) => (
          <button
            key={key}
            type="button"
            className={`usage-mobile-range-pill${range === key ? " active" : ""}`}
            aria-pressed={range === key}
            onClick={() => {
              change(() => {
                setRange(key);
                if (key === "cycle") {
                  setCycleOffset(0);
                  setCycleNow(Date.now());
                  setManualAppOverride(false);
                  if (activeCyclePlan.appType && !app) {
                    setApp(activeCyclePlan.appType);
                    setProvider("");
                    setModel("");
                  }
                }
              });
            }}
          >
            {label}
          </button>
        ))}
      </div>
      <div className="filter-row">
        <div className="filters">
          <select
            className="usage-range-desktop-select"
            aria-label="统计时间"
            value={range}
            onChange={(e) => {
              const next = e.target.value as Range;
              change(() => {
                setRange(next);
                if (next === "cycle") {
                  setCycleOffset(0);
                  setCycleNow(Date.now());
                  setManualAppOverride(false);
                  if (activeCyclePlan.appType && !app) {
                    setApp(activeCyclePlan.appType);
                    setProvider("");
                    setModel("");
                  }
                }
              });
            }}
          >
            {USAGE_RANGES.map((key) => (
              <option key={key} value={key}>
                {RANGE_PRESETS[key].label}
              </option>
            ))}
            <option value="all">全部时间</option>
            <option value="custom">自定义时间</option>
            <option value="cycle">按周期查询</option>
          </select>
          {scope === "multi" && (
            <details className="usage-device-select">
              <summary aria-label="选择统计设备">
                {activeDevices === null
                  ? "全部设备"
                  : `已选 ${activeDevices.length} 台设备`}
              </summary>
              <div className="usage-device-options">
                <button
                  type="button"
                  onClick={() => change(() => setSelectedDevices(null))}
                >
                  全部设备
                </button>
                {[
                  {
                    deviceId: "local",
                    deviceName: `${lan.data?.local.deviceName ?? "本机"}（本机）`,
                  },
                  ...(lan.data?.devices.filter((d) => d.enabled && d.paired) ??
                    []),
                ].map((device) => {
                  const all = [
                    "local",
                    ...(lan.data?.devices
                      .filter((d) => d.enabled && d.paired)
                      .map((d) => d.deviceId) ?? []),
                  ];
                  const current = activeDevices ?? all;
                  const checked = current.includes(device.deviceId);
                  return (
                    <label key={device.deviceId}>
                      <input
                        type="checkbox"
                        checked={checked}
                        onChange={() =>
                          change(() => {
                            const next = checked
                              ? current.filter((id) => id !== device.deviceId)
                              : [...current, device.deviceId];
                            if (next.length) setSelectedDevices(next);
                          })
                        }
                      />{" "}
                      <span>{device.deviceName}</span>
                    </label>
                  );
                })}
                <small>至少选择一台设备</small>
              </div>
            </details>
          )}
          <AppSelect
            sources={sources}
            value={app}
            onChange={(value) =>
              change(() => {
                if (range === "cycle") setManualAppOverride(true);
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
            {modelOptions.map((m) => (
              <option key={m} value={m}>
                {m}
              </option>
            ))}
          </select>
        </div>
        <div className="filters usage-desktop-actions">
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
            disabled={
              scanBusy || !status.data || (scope === "multi" && !lan.data)
            }
            onClick={() =>
              void (scope === "multi" ? refreshMulti() : refresh()).catch(
                () => {},
              )
            }
          >
            <RefreshCw size={13} />
            {scanBusy ? "扫描中…" : "扫描更新"}
          </button>
        </div>
      </div>
      {range === "cycle" && (
        <CycleNavigator
          plans={cyclePlans}
          activePlan={activeCyclePlan}
          window={activeCycleWindow}
          offset={cycleOffset}
          summary={s}
          linkedAppName={
            !manualAppOverride &&
            activeCyclePlan.appType &&
            app === activeCyclePlan.appType
              ? appName(activeCyclePlan.appType)
              : undefined
          }
          onSelectPlan={handleSelectCyclePlan}
          onPrev={() => change(() => setCycleOffset((o) => o - 1))}
          onNext={() => change(() => setCycleOffset((o) => Math.min(0, o + 1)))}
          onResetOffset={() =>
            change(() => {
              setCycleOffset(0);
              setCycleNow(Date.now());
            })
          }
          onEditPlan={(planId) => {
            setCycleSaveError("");
            setCycleModal({ open: true, planId });
          }}
          onNewPlan={() => {
            setCycleSaveError("");
            setCycleModal({ open: true, planId: null });
          }}
          onUnlinkApp={() =>
            change(() => {
              setManualAppOverride(true);
              setApp("");
              setProvider("");
              setModel("");
            })
          }
        />
      )}
      {range === "custom" && (
        <div className="usage-date-range">
          <DateTimeFields
            label="开始时间"
            value={customStart}
            onChange={(value) => change(() => setCustomStart(value))}
          />
          <span>至</span>
          <DateTimeFields
            label="结束时间"
            value={customEnd}
            onChange={(value) => change(() => setCustomEnd(value))}
          />
          {!validPeriod && (
            <span className="usage-error">
              请选择有效时间，开始时间不能晚于结束时间。
            </span>
          )}
          <span className="hint">
            按本机时区，包含结束秒；未完整覆盖的日期不计入日汇总。
          </span>
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
      {currentScan?.notice && (
        <div className="usage-note" role="status">
          {currentScan.notice}
        </div>
      )}
      {scope === "multi" && multiBatch && (
        <MultiDeviceScanPanel batch={multiBatch} />
      )}
      {currentScan && currentScan.errors.length > 0 && (
        <details className="usage-error">
          <summary>
            {currentScan.errors.length} 项来源暂未完成扫描或同步，已有记录保留
          </summary>
          {currentScan.errors.map((e, i) => (
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
          delta={makeDelta(
            s ? Number(s.totalCost) : undefined,
            prevCycleSummary ? Number(prevCycleSummary.totalCost) : undefined,
          )}
          onSelect={() => setMetric("cost")}
        />
        <Metric
          label="请求次数"
          value={s ? s.totalRequests.toLocaleString() : "—"}
          detail={s ? "成功率 " + s.successRate.toFixed(1) + "%" : "读取统计中"}
          icon={Activity}
          selected={metric === "requests"}
          delta={makeDelta(s?.totalRequests, prevCycleSummary?.totalRequests)}
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
          delta={makeDelta(
            s?.realTotalTokens,
            prevCycleSummary?.realTotalTokens,
          )}
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
          delta={makeDelta(
            s ? s.cacheHitRate * 100 : undefined,
            prevCycleSummary ? prevCycleSummary.cacheHitRate * 100 : undefined,
            true,
          )}
          onSelect={() => setMetric("hitRate")}
        />
      </div>
      <div className="usage-token-parts" aria-label="Token 构成">
        {(() => {
          const parts: [string, number | undefined, string][] = [
            ["新增输入", s?.totalInputTokens, "var(--accent-blue)"],
            ["输出", s?.totalOutputTokens, "var(--accent-pink)"],
            ["缓存写入", s?.totalCacheCreationTokens, "var(--accent-orange)"],
            ["缓存读取", s?.totalCacheReadTokens, "var(--accent-cyan)"],
          ];
          const total = parts.reduce((sum, [, v]) => sum + (v ?? 0), 0);
          return (
            <>
              {total > 0 && (
                <div className="token-capsule" aria-hidden="true">
                  {parts.map(([label, value, color]) => (
                    <i
                      key={label}
                      style={{ flexGrow: value ?? 0, background: color }}
                    />
                  ))}
                </div>
              )}
              <div className="token-parts-legend">
                {parts.map(([label, value, color]) => (
                  <span key={label}>
                    <span>
                      <i style={{ background: color }} />
                      {label}
                    </span>
                    <b>
                      {typeof value === "number" ? formatTokens(value) : "—"}
                    </b>
                  </span>
                ))}
              </div>
            </>
          );
        })()}
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
                {(
                  [
                    ["tokens", "Token"],
                    ["cost", "费用"],
                    ["requests", "请求"],
                    ["hitRate", "命中率"],
                  ] as [RankingMetric, string][]
                ).map(([key, label]) => (
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
                <button
                  aria-pressed={chartType === "blocks"}
                  onClick={() => setChartType("blocks")}
                >
                  方块
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
            <h2>
              {metric === "cost"
                ? "应用费用排名"
                : metric === "requests"
                  ? "应用请求排名"
                  : metric === "hitRate"
                    ? "应用缓存命中率排名"
                    : "应用 Token 排名"}
            </h2>
            <span className="muted">
              {metric === "cost"
                ? "USD"
                : metric === "requests"
                  ? "次"
                  : metric === "hitRate"
                    ? "%"
                    : "Token"}
            </span>
          </div>
          <div className="native-app-costs">
            {rankApps(dashboard.data?.byApp ?? [], metric, app).map((a) => (
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
                  <b>
                    {a.value === null
                      ? "—"
                      : metric === "cost"
                        ? formatMoney(a.value)
                        : metric === "requests"
                          ? a.value.toLocaleString()
                          : metric === "hitRate"
                            ? (a.value * 100).toFixed(1) + "%"
                            : formatTokens(a.value)}
                  </b>
                </span>
                <span className="meter">
                  <span
                    style={{
                      width:
                        Math.max(
                          0,
                          Math.min(
                            100,
                            metric === "hitRate"
                              ? (a.value ?? 0) * 100
                              : ((a.value ?? 0) /
                                  Math.max(
                                    0.000001,
                                    metric === "cost"
                                      ? Number(s?.totalCost ?? 0)
                                      : metric === "requests"
                                        ? (s?.totalRequests ?? 0)
                                        : (s?.realTotalTokens ?? 0),
                                  )) *
                                  100,
                          ),
                        ) + "%",
                    }}
                  />
                </span>
              </button>
            ))}
          </div>
        </section>
      </div>
      {range === "cycle" && (
        <CycleHistoryPanel
          entries={historyEntries}
          activeOffset={cycleOffset}
          metric={metric}
          loading={cycleHistory.isFetching}
          onSelectOffset={(off) => change(() => setCycleOffset(off))}
        />
      )}
      <ModelSpeedChart models={dashboard.data?.models ?? []} />
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
                    <LogRow
                      key={r.requestId}
                      log={r}
                      appNameText={appName(r.appType)}
                      onOpen={setDetail}
                    />
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
      {cycleModal !== null && (
        <CycleConfigModal
          plans={cyclePlans}
          initialPlanId={cycleModal.planId}
          sources={sources}
          saving={saveCycles.isPending}
          saveError={cycleSaveError}
          onSave={handleSaveCyclePlan}
          onDelete={handleDeleteCyclePlan}
          onClose={() => {
            setCycleSaveError("");
            setCycleModal(null);
          }}
        />
      )}
      {modal === "devices" && (
        <UsageDialog title="设备连接" close={() => setModal(null)} wide>
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
          deviceIds={activeDevices ?? undefined}
          close={() => setDetail(null)}
        />
      )}
    </div>
  );
}

export const UsageWorkspace = memo(UsageWorkspaceImpl);
