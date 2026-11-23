import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { getCurrentWindow } from "@tauri-apps/api/window";
import {
  Check,
  LayoutGrid,
  List,
  Maximize2,
  Minimize2,
  Plus,
  RefreshCw,
  Search,
  Star,
  Trash2,
  TrendingUp,
  X,
} from "lucide-react";
import { usageCall } from "../native/usageClient";
import { MARKET_PRESETS } from "./presets";
import { heatTileColor, layoutSquarified } from "./treemap";
import "./markets.css";

interface WatchItem {
  symbol: string;
  name: string;
  market: string;
  sector?: string;
  capHint?: number;
}

interface MarketsConfig {
  indices: WatchItem[];
  watchlist: WatchItem[];
  colorScheme: "cn" | "us";
  refreshSec: number;
}

interface ChartPoint {
  ts: number;
  price: number;
}

interface StockQuote {
  symbol: string;
  name: string;
  market: string;
  currency: string;
  exchange: string;
  price: number | null;
  prevClose: number | null;
  change: number | null;
  changePercent: number | null;
  open: number | null;
  high: number | null;
  low: number | null;
  volume: number | null;
  marketCap?: number | null;
  extendedPrice: number | null;
  extendedChangePercent: number | null;
  extendedLabel: string | null;
  marketState: string;
  source: string;
  quoteTime: number | null;
  sparkline: ChartPoint[];
  error: string | null;
}

interface StockChartDetail {
  symbol: string;
  range: string;
  currency: string;
  prevClose: number | null;
  source: string;
  points: ChartPoint[];
}

interface SearchHit {
  symbol: string;
  name: string;
  market: string;
  exchange: string;
}

type MarketFilter = "ALL" | "US" | "TW" | "CN" | "HK" | "GLOBAL";
type ChartRange = "1d" | "5d" | "1mo" | "3mo" | "1y";
type WatchViewMode = "heatmap" | "list";
type TreemapSizeMode = "cap" | "turnover" | "move" | "equal";

const VIEW_MODE_KEY = "observatory.markets.view-mode";
const POOL_ID_KEY = "observatory.markets.pool-id";

const MARKET_META: Record<string, { label: string; short: string }> = {
  US: { label: "美股", short: "US" },
  TW: { label: "台股", short: "TW" },
  CN: { label: "A 股", short: "CN" },
  HK: { label: "港股", short: "HK" },
  GLOBAL: { label: "全球", short: "GL" },
};

const CURRENCY_SYM: Record<string, string> = {
  USD: "$",
  CNY: "¥",
  HKD: "HK$",
  TWD: "NT$",
  JPY: "¥",
  EUR: "€",
};

function fmtPrice(v: number | null | undefined, currency?: string): string {
  if (v == null || !Number.isFinite(v)) return "—";
  const prefix = currency ? (CURRENCY_SYM[currency] ?? "") : "";
  const decimals = v >= 1000 ? 2 : v >= 1 ? 2 : 4;
  return `${prefix}${v.toLocaleString("en-US", {
    minimumFractionDigits: decimals,
    maximumFractionDigits: decimals,
  })}`;
}

function fmtSigned(v: number | null | undefined, suffix = ""): string {
  if (v == null || !Number.isFinite(v)) return "—";
  const sign = v > 0 ? "+" : "";
  return `${sign}${v.toFixed(2)}${suffix}`;
}

function fmtVolume(v: number | null | undefined): string {
  if (v == null || !Number.isFinite(v) || v <= 0) return "—";
  if (v >= 1_000_000_000) return `${(v / 1_000_000_000).toFixed(2)}B`;
  if (v >= 1_000_000) return `${(v / 1_000_000).toFixed(2)}M`;
  if (v >= 10_000) return `${(v / 10_000).toFixed(1)}万`;
  return v.toLocaleString();
}

function fmtMarketCap(v: number | null | undefined, currency?: string): string {
  if (v == null || !Number.isFinite(v) || v <= 0) return "—";
  const prefix = currency ? (CURRENCY_SYM[currency] ?? "") : "";
  if (currency === "CNY" || currency === "HKD" || currency === "TWD") {
    if (v >= 1e12) return `${prefix}${(v / 1e12).toFixed(2)}万亿`;
    if (v >= 1e8) return `${prefix}${(v / 1e8).toFixed(0)}亿`;
    return `${prefix}${v.toLocaleString()}`;
  }
  if (v >= 1e12) return `${prefix}${(v / 1e12).toFixed(2)}T`;
  if (v >= 1e9) return `${prefix}${(v / 1e9).toFixed(1)}B`;
  if (v >= 1e6) return `${prefix}${(v / 1e6).toFixed(0)}M`;
  return `${prefix}${v.toLocaleString()}`;
}

const FX_TO_USD: Record<string, number> = {
  USD: 1,
  CNY: 7.2,
  HKD: 7.8,
  TWD: 32,
  JPY: 150,
  EUR: 0.92,
};

function marketCapUsdBillions(
  item: WatchItem,
  q: StockQuote | undefined,
): number | null {
  if (q?.marketCap != null && Number.isFinite(q.marketCap) && q.marketCap > 0) {
    const fx = FX_TO_USD[q.currency] ?? 1;
    return q.marketCap / fx / 1e9;
  }
  if (item.capHint != null && item.capHint > 0) {
    return item.capHint;
  }
  return null;
}

function fmtClock(tsSec: number | null | undefined): string {
  if (!tsSec) return "—";
  return new Date(tsSec * 1000).toLocaleTimeString("zh-CN", { hour12: false });
}

function fmtChartTime(tsSec: number, range: ChartRange): string {
  const d = new Date(tsSec * 1000);
  if (range === "1d") {
    return d.toLocaleTimeString("zh-CN", {
      hour: "2-digit",
      minute: "2-digit",
      hour12: false,
    });
  }
  if (range === "5d") {
    return d.toLocaleString("zh-CN", {
      month: "2-digit",
      day: "2-digit",
      hour: "2-digit",
      minute: "2-digit",
      hour12: false,
    });
  }
  return d.toLocaleDateString("zh-CN", {
    year: "numeric",
    month: "2-digit",
    day: "2-digit",
  });
}

function trendTone(
  change: number | null | undefined,
  scheme: "cn" | "us",
): "up" | "down" | "flat" {
  if (change == null || Math.abs(change) < 1e-6) return "flat";
  const positive = change > 0;
  if (scheme === "cn") {
    return positive ? "up" : "down";
  }
  return positive ? "down" : "up";
}

function toneColor(tone: "up" | "down" | "flat"): string {
  if (tone === "up") return "var(--accent-red)";
  if (tone === "down") return "var(--accent-green)";
  return "var(--muted)";
}

export function MarketsWorkspace() {
  const qc = useQueryClient();
  const [poolId, setPoolId] = useState<string>(() => {
    try {
      const saved = localStorage.getItem(POOL_ID_KEY);
      if (
        saved &&
        (saved === "watchlist" || MARKET_PRESETS.some((p) => p.id === saved))
      ) {
        return saved;
      }
    } catch {
      // ignore storage errors
    }
    return "us_sp500";
  });
  const [filter, setFilter] = useState<MarketFilter>("ALL");
  const [sectorFilter, setSectorFilter] = useState<string>("ALL");
  const [poolQuery, setPoolQuery] = useState<string>("");
  const [viewMode, setViewMode] = useState<WatchViewMode>(() => {
    try {
      return localStorage.getItem(VIEW_MODE_KEY) === "list"
        ? "list"
        : "heatmap";
    } catch {
      return "heatmap";
    }
  });
  const [sizeMode, setSizeMode] = useState<TreemapSizeMode>("cap");
  const [selectedSymbol, setSelectedSymbol] = useState<string>("NVDA");
  const [chartRange, setChartRange] = useState<ChartRange>("1d");
  const [searchText, setSearchText] = useState("");
  const [debouncedQuery, setDebouncedQuery] = useState("");
  const [searchOpen, setSearchOpen] = useState(false);
  const [isFullscreen, setIsFullscreen] = useState(false);
  const [fsDrawerOpen, setFsDrawerOpen] = useState(false);
  const enteredOsFsRef = useRef(false);
  const searchWrapRef = useRef<HTMLDivElement>(null);

  const enterFullscreen = useCallback(async () => {
    setIsFullscreen(true);
    setFsDrawerOpen(false);
    try {
      const win = getCurrentWindow();
      const already = await win.isFullscreen();
      if (!already) {
        await win.setFullscreen(true);
        enteredOsFsRef.current = true;
      }
    } catch {
      // Browser/dev preview without Tauri window runtime: CSS viewport fullscreen still works.
    }
  }, []);

  const exitFullscreen = useCallback(async () => {
    setIsFullscreen(false);
    setFsDrawerOpen(false);
    if (enteredOsFsRef.current) {
      enteredOsFsRef.current = false;
      try {
        await getCurrentWindow().setFullscreen(false);
      } catch {
        // ignore if window already restored
      }
    }
  }, []);

  useEffect(() => {
    if (!isFullscreen) return;
    const prevOverflow = document.body.style.overflow;
    document.body.style.overflow = "hidden";
    return () => {
      document.body.style.overflow = prevOverflow;
    };
  }, [isFullscreen]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape" && isFullscreen) {
        e.preventDefault();
        if (fsDrawerOpen) {
          setFsDrawerOpen(false);
        } else {
          void exitFullscreen();
        }
        return;
      }
      if (
        (e.key === "f" || e.key === "F") &&
        !e.metaKey &&
        !e.ctrlKey &&
        !e.altKey &&
        viewMode === "heatmap"
      ) {
        const tag = (e.target as HTMLElement | null)?.tagName;
        if (tag === "INPUT" || tag === "TEXTAREA" || tag === "SELECT") return;
        e.preventDefault();
        if (isFullscreen) {
          void exitFullscreen();
        } else {
          void enterFullscreen();
        }
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [isFullscreen, fsDrawerOpen, viewMode, enterFullscreen, exitFullscreen]);

  useEffect(() => {
    return () => {
      if (enteredOsFsRef.current) {
        enteredOsFsRef.current = false;
        void getCurrentWindow()
          .setFullscreen(false)
          .catch(() => {});
      }
    };
  }, []);

  const changePool = (nextPoolId: string) => {
    setPoolId(nextPoolId);
    setSectorFilter("ALL");
    setPoolQuery("");
    try {
      localStorage.setItem(POOL_ID_KEY, nextPoolId);
    } catch {
      // ignore storage errors
    }
  };

  const changeViewMode = (next: WatchViewMode) => {
    setViewMode(next);
    try {
      localStorage.setItem(VIEW_MODE_KEY, next);
    } catch {
      // ignore storage errors
    }
  };

  useEffect(() => {
    const t = setTimeout(() => setDebouncedQuery(searchText.trim()), 280);
    return () => clearTimeout(t);
  }, [searchText]);

  useEffect(() => {
    const onDown = (e: MouseEvent) => {
      if (
        searchWrapRef.current &&
        !searchWrapRef.current.contains(e.target as Node)
      ) {
        setSearchOpen(false);
      }
    };
    document.addEventListener("mousedown", onDown);
    return () => document.removeEventListener("mousedown", onDown);
  }, []);

  const cfgQuery = useQuery({
    queryKey: ["markets", "config"],
    queryFn: () => usageCall<MarketsConfig>("markets_config_get"),
    staleTime: 60_000,
  });

  const saveCfg = useMutation({
    mutationFn: (next: MarketsConfig) =>
      usageCall<MarketsConfig>("markets_config_save", next),
    onSuccess: (saved) => {
      qc.setQueryData(["markets", "config"], saved);
    },
  });

  const config = cfgQuery.data;
  const colorScheme: "cn" | "us" = config?.colorScheme === "us" ? "us" : "cn";
  const refreshSec = config?.refreshSec ?? 15;

  const activePreset = useMemo(
    () => MARKET_PRESETS.find((p) => p.id === poolId) ?? null,
    [poolId],
  );
  const isCustomWatchlist = activePreset === null;

  const activePoolItems: WatchItem[] = useMemo(
    () => (activePreset ? activePreset.items : (config?.watchlist ?? [])),
    [activePreset, config?.watchlist],
  );

  const indexItems = useMemo(() => config?.indices ?? [], [config?.indices]);
  const indicesKey = useMemo(
    () => indexItems.map((i) => i.symbol).join(","),
    [indexItems],
  );
  const poolKey = useMemo(
    () => activePoolItems.map((i) => i.symbol).join(","),
    [activePoolItems],
  );

  const indicesQuery = useQuery({
    queryKey: ["markets", "indices", indicesKey],
    queryFn: () =>
      usageCall<StockQuote[]>("markets_quotes", {
        items: indexItems,
        skipSparkline: false,
      }),
    enabled: indexItems.length > 0,
    refetchInterval: () =>
      refreshSec > 0 && document.visibilityState === "visible"
        ? refreshSec * 1000
        : false,
    placeholderData: (prev) => prev,
    retry: false,
  });

  const quotesQuery = useQuery({
    queryKey: ["markets", "pool", poolId, poolKey, viewMode],
    queryFn: () =>
      usageCall<StockQuote[]>("markets_quotes", {
        items: activePoolItems.map(({ symbol, name, market }) => ({
          symbol,
          name,
          market,
        })),
        skipSparkline: viewMode === "heatmap",
      }),
    enabled: activePoolItems.length > 0,
    refetchInterval: () => {
      if (refreshSec <= 0 || document.visibilityState !== "visible") {
        return false;
      }
      const effectiveSec =
        activePoolItems.length > 100 ? Math.max(refreshSec, 15) : refreshSec;
      return effectiveSec * 1000;
    },
    placeholderData: (prev) => prev,
    retry: false,
  });

  const quoteBySymbol = useMemo(() => {
    const map = new Map<string, StockQuote>();
    for (const q of indicesQuery.data ?? []) {
      map.set(q.symbol, q);
    }
    for (const q of quotesQuery.data ?? []) {
      map.set(q.symbol, q);
    }
    return map;
  }, [indicesQuery.data, quotesQuery.data]);

  const knownItems = useMemo(
    () => [...indexItems, ...activePoolItems],
    [indexItems, activePoolItems],
  );

  const activeSymbol = useMemo(() => {
    if (knownItems.some((i) => i.symbol === selectedSymbol)) {
      return selectedSymbol;
    }
    return activePoolItems[0]?.symbol ?? indexItems[0]?.symbol ?? "";
  }, [knownItems, selectedSymbol, activePoolItems, indexItems]);

  const chartQuery = useQuery({
    queryKey: ["markets", "chart", activeSymbol, chartRange],
    queryFn: () =>
      usageCall<StockChartDetail>("markets_chart", {
        symbol: activeSymbol,
        range: chartRange,
      }),
    enabled: !!activeSymbol,
    refetchInterval: () =>
      chartRange === "1d" &&
      refreshSec > 0 &&
      document.visibilityState === "visible"
        ? Math.max(refreshSec, 15) * 1000
        : false,
    placeholderData: (prev) => prev,
    retry: false,
  });

  const searchQuery = useQuery({
    queryKey: ["markets", "search", debouncedQuery],
    queryFn: () =>
      usageCall<SearchHit[]>("markets_search", { query: debouncedQuery }),
    enabled: debouncedQuery.length >= 1,
    staleTime: 30_000,
    retry: false,
  });

  const watchlistSymbols = useMemo(
    () => new Set((config?.watchlist ?? []).map((w) => w.symbol)),
    [config?.watchlist],
  );

  const presetSectors = useMemo(() => {
    if (!activePreset) return [];
    const seen = new Set<string>();
    const out: string[] = [];
    for (const item of activePreset.items) {
      if (item.sector && !seen.has(item.sector)) {
        seen.add(item.sector);
        out.push(item.sector);
      }
    }
    return out;
  }, [activePreset]);

  const filteredWatchlist = useMemo(() => {
    let base = activePoolItems;
    if (isCustomWatchlist) {
      if (filter !== "ALL") base = base.filter((w) => w.market === filter);
    } else if (sectorFilter !== "ALL") {
      base = base.filter((w) => w.sector === sectorFilter);
    }
    const q = poolQuery.trim().toLowerCase();
    if (!q) return base;
    return base.filter(
      (w) =>
        w.symbol.toLowerCase().includes(q) ||
        w.name.toLowerCase().includes(q) ||
        (w.sector?.toLowerCase().includes(q) ?? false),
    );
  }, [activePoolItems, isCustomWatchlist, filter, sectorFilter, poolQuery]);

  const addSymbol = (hit: { symbol: string; name: string; market: string }) => {
    if (!config) return;
    if (config.watchlist.some((w) => w.symbol === hit.symbol)) {
      setSelectedSymbol(hit.symbol);
      setSearchOpen(false);
      return;
    }
    const next: MarketsConfig = {
      ...config,
      watchlist: [
        { symbol: hit.symbol, name: hit.name, market: hit.market },
        ...config.watchlist,
      ],
    };
    saveCfg.mutate(next);
    setSelectedSymbol(hit.symbol);
    setSearchText("");
    setSearchOpen(false);
  };

  const removeSymbol = (sym: string) => {
    if (!config) return;
    const next: MarketsConfig = {
      ...config,
      watchlist: config.watchlist.filter((w) => w.symbol !== sym),
    };
    saveCfg.mutate(next);
  };

  const toggleWatchlist = (item: {
    symbol: string;
    name: string;
    market: string;
  }) => {
    if (watchlistSymbols.has(item.symbol)) {
      removeSymbol(item.symbol);
    } else {
      addSymbol(item);
    }
  };

  const updatePrefs = (patch: Partial<MarketsConfig>) => {
    if (!config) return;
    saveCfg.mutate({ ...config, ...patch });
  };

  const manualRefresh = () => {
    void indicesQuery.refetch();
    void quotesQuery.refetch();
    void chartQuery.refetch();
  };

  const selectedQuote = quoteBySymbol.get(activeSymbol);
  const selectedItem = knownItems.find((i) => i.symbol === activeSymbol);
  const isAnyFetching = indicesQuery.isFetching || quotesQuery.isFetching;
  const lastUpdatedTs = Math.max(
    indicesQuery.dataUpdatedAt ?? 0,
    quotesQuery.dataUpdatedAt ?? 0,
  );
  const lastUpdatedStr = lastUpdatedTs
    ? new Date(lastUpdatedTs).toLocaleTimeString("zh-CN", {
        hour12: false,
      })
    : "—";

  return (
    <div className="markets-page">
      <div className="markets-toolbar">
        <div>
          <div className="markets-heading">
            <TrendingUp size={17} />
            <span>全球主要市场实时行情</span>
            <span className="markets-badge neutral">免注册 · 零 Key</span>
          </div>
          <p className="markets-sub">
            A 股 / 港股走腾讯实时（1–3s）· 台股走 TWSE 官方实时（5s）· 美股走
            Yahoo 实时（含盘前/盘后），仅在页面可见时自动刷新
          </p>
        </div>
        <div className="markets-controls">
          <div className="markets-pill-group">
            <button
              type="button"
              className={`markets-pill-btn${colorScheme === "cn" ? " active" : ""}`}
              onClick={() => updatePrefs({ colorScheme: "cn" })}
            >
              红涨绿跌
            </button>
            <button
              type="button"
              className={`markets-pill-btn${colorScheme === "us" ? " active" : ""}`}
              onClick={() => updatePrefs({ colorScheme: "us" })}
            >
              绿涨红跌
            </button>
          </div>
          <div className="markets-pill-group">
            {(
              [
                [5, "5s"],
                [15, "15s"],
                [30, "30s"],
                [0, "手动"],
              ] as const
            ).map(([sec, label]) => (
              <button
                key={sec}
                type="button"
                className={`markets-pill-btn${refreshSec === sec ? " active" : ""}`}
                onClick={() => updatePrefs({ refreshSec: sec })}
              >
                {label}
              </button>
            ))}
          </div>
          <button
            type="button"
            className="markets-refresh-btn"
            onClick={manualRefresh}
            disabled={isAnyFetching}
          >
            <RefreshCw
              size={13}
              className={isAnyFetching ? "markets-spin" : undefined}
            />
            <span>刷新 {lastUpdatedStr}</span>
          </button>
        </div>
      </div>

      {/* Global Indices Strip */}
      <section className="markets-indices-grid" aria-label="核心市场指数">
        {(config?.indices ?? []).map((idxItem) => {
          const q = quoteBySymbol.get(idxItem.symbol);
          const tone = trendTone(q?.change, colorScheme);
          const color = toneColor(tone);
          const isSelected = activeSymbol === idxItem.symbol;
          return (
            <button
              key={idxItem.symbol}
              type="button"
              className={`markets-index-card${isSelected ? " active" : ""}`}
              onClick={() => setSelectedSymbol(idxItem.symbol)}
            >
              <div className="markets-index-top">
                <span className="markets-mkt-tag">{idxItem.market}</span>
                <span className="markets-index-name">{idxItem.name}</span>
              </div>
              <div className="markets-index-price">
                {q?.price != null
                  ? q.price.toLocaleString("en-US", {
                      minimumFractionDigits: 2,
                      maximumFractionDigits: 2,
                    })
                  : indicesQuery.isPending
                    ? "…"
                    : "—"}
              </div>
              <div className="markets-index-bottom">
                <span style={{ color, fontWeight: 600 }}>
                  {fmtSigned(q?.changePercent, "%")}
                </span>
                <MiniSparkline
                  points={q?.sparkline ?? []}
                  prevClose={q?.prevClose ?? null}
                  color={color}
                />
              </div>
            </button>
          );
        })}
      </section>

      {/* Preset Pools / Universe Selector */}
      <div className="markets-pool-bar">
        <div className="markets-pill-group">
          <button
            type="button"
            className={`markets-pill-btn${poolId === "watchlist" ? " active" : ""}`}
            onClick={() => changePool("watchlist")}
          >
            我的自选 ({config?.watchlist.length ?? 0})
          </button>
          {MARKET_PRESETS.map((preset) => (
            <button
              key={preset.id}
              type="button"
              className={`markets-pill-btn${poolId === preset.id ? " active" : ""}`}
              onClick={() => changePool(preset.id)}
              title={preset.description}
            >
              {preset.label}
            </button>
          ))}
        </div>
        {activePreset && (
          <span className="markets-pool-desc">{activePreset.description}</span>
        )}
      </div>

      {/* Main Split: Full-width Heatmap + Detail below, OR side-by-side List + Detail */}
      <div
        className={`markets-split${viewMode === "heatmap" ? " heatmap-wide" : ""}`}
      >
        <section className="markets-card markets-watchlist-card">
          <div className="markets-card-head">
            <div className="markets-head-left">
              <div className="markets-pill-group">
                <button
                  type="button"
                  className={`markets-pill-btn${viewMode === "heatmap" ? " active" : ""}`}
                  onClick={() => changeViewMode("heatmap")}
                >
                  <LayoutGrid size={12} />
                  <span>热力云图</span>
                </button>
                <button
                  type="button"
                  className={`markets-pill-btn${viewMode === "list" ? " active" : ""}`}
                  onClick={() => changeViewMode("list")}
                >
                  <List size={12} />
                  <span>列表</span>
                </button>
              </div>
              {isCustomWatchlist ? (
                <div className="markets-pill-group">
                  {(
                    [
                      ["ALL", "全部"],
                      ["US", "美股"],
                      ["TW", "台股"],
                      ["CN", "A 股"],
                      ["HK", "港股"],
                      ["GLOBAL", "其他"],
                    ] as const
                  ).map(([m, label]) => (
                    <button
                      key={m}
                      type="button"
                      className={`markets-pill-btn${filter === m ? " active" : ""}`}
                      onClick={() => setFilter(m)}
                    >
                      {label}
                    </button>
                  ))}
                </div>
              ) : (
                <div className="markets-pill-group markets-sector-pills">
                  <button
                    type="button"
                    className={`markets-pill-btn${sectorFilter === "ALL" ? " active" : ""}`}
                    onClick={() => setSectorFilter("ALL")}
                  >
                    全部板块 ({activePoolItems.length})
                  </button>
                  {presetSectors.map((sec) => (
                    <button
                      key={sec}
                      type="button"
                      className={`markets-pill-btn${sectorFilter === sec ? " active" : ""}`}
                      onClick={() => setSectorFilter(sec)}
                    >
                      {sec}
                    </button>
                  ))}
                </div>
              )}
            </div>

            <div className="markets-search-wrap" ref={searchWrapRef}>
              <label className="markets-search-box">
                <Search size={13} />
                <input
                  type="search"
                  value={searchText}
                  onFocus={() => setSearchOpen(true)}
                  onChange={(e) => {
                    setSearchText(e.target.value);
                    setSearchOpen(true);
                  }}
                  placeholder="搜索代码/名称添加，如 NVDA、2330、茅台…"
                  aria-label="搜索并添加自选股"
                />
              </label>
              {searchOpen && debouncedQuery.length >= 1 && (
                <div className="markets-search-dropdown">
                  {searchQuery.isFetching ? (
                    <div className="markets-search-empty">搜索中…</div>
                  ) : (searchQuery.data ?? []).length === 0 ? (
                    <div className="markets-search-empty">
                      未找到匹配标的（可尝试完整代码如 2330.TW / 600519.SS）
                    </div>
                  ) : (
                    (searchQuery.data ?? []).map((hit) => {
                      const exists = watchlistSymbols.has(hit.symbol);
                      return (
                        <button
                          key={hit.symbol}
                          type="button"
                          className="markets-search-item"
                          onClick={() => addSymbol(hit)}
                        >
                          <span className="markets-mkt-tag">{hit.market}</span>
                          <span className="markets-search-info">
                            <strong>{hit.name}</strong>
                            <small>
                              {hit.symbol}
                              {hit.exchange ? ` · ${hit.exchange}` : ""}
                            </small>
                          </span>
                          <span className="markets-add-tag">
                            {exists ? (
                              <>
                                <Check size={12} /> 已自选
                              </>
                            ) : (
                              <>
                                <Plus size={12} /> 添加
                              </>
                            )}
                          </span>
                        </button>
                      );
                    })
                  )}
                </div>
              )}
            </div>
          </div>

          {viewMode === "heatmap" ? (
            <MarketHeatmap
              items={filteredWatchlist}
              quoteBySymbol={quoteBySymbol}
              colorScheme={colorScheme}
              groupBy={
                activePreset
                  ? sectorFilter === "ALL" && !poolQuery.trim()
                    ? "sector"
                    : "none"
                  : filter === "ALL"
                    ? "market"
                    : "none"
              }
              activeSector={sectorFilter !== "ALL" ? sectorFilter : null}
              onSelectSector={activePreset ? setSectorFilter : undefined}
              poolQuery={poolQuery}
              onPoolQueryChange={
                activePoolItems.length > 25 ? setPoolQuery : undefined
              }
              sizeMode={sizeMode}
              onSizeModeChange={setSizeMode}
              selectedSymbol={activeSymbol}
              onSelect={(sym) => {
                setSelectedSymbol(sym);
                if (isFullscreen) setFsDrawerOpen(true);
              }}
              isFullscreen={isFullscreen}
              onToggleFullscreen={() => {
                if (isFullscreen) {
                  void exitFullscreen();
                } else {
                  void enterFullscreen();
                }
              }}
              poolId={poolId}
              watchlistCount={config?.watchlist.length ?? 0}
              onChangePool={changePool}
              onRefresh={manualRefresh}
              isRefreshing={isAnyFetching}
              lastUpdatedStr={lastUpdatedStr}
            />
          ) : (
            <div className="markets-table-wrap">
              <table className="markets-table">
                <thead>
                  <tr>
                    <th>标的</th>
                    <th className="num">最新价</th>
                    <th className="num">涨跌</th>
                    <th className="num">分时</th>
                    <th className="num">来源 / 时间</th>
                    <th />
                  </tr>
                </thead>
                <tbody>
                  {filteredWatchlist.map((w) => {
                    const q = quoteBySymbol.get(w.symbol);
                    const tone = trendTone(q?.change, colorScheme);
                    const color = toneColor(tone);
                    const active = activeSymbol === w.symbol;
                    return (
                      <tr
                        key={w.symbol}
                        className={`markets-row${active ? " active" : ""}`}
                        onClick={() => setSelectedSymbol(w.symbol)}
                      >
                        <td>
                          <div className="markets-sym-cell">
                            <span className="markets-mkt-tag">
                              {MARKET_META[w.market]?.short ?? w.market}
                            </span>
                            <div className="markets-sym-text">
                              <strong>{q?.name || w.name || w.symbol}</strong>
                              <span>
                                {w.symbol}
                                {w.sector ? ` · ${w.sector}` : ""}
                                {q?.marketState &&
                                  q.marketState !== "已休市" && (
                                    <em className="markets-live-dot">
                                      {" "}
                                      · {q.marketState}
                                    </em>
                                  )}
                              </span>
                            </div>
                          </div>
                        </td>
                        <td className="num">
                          <div className="markets-price-main">
                            {fmtPrice(q?.price, q?.currency)}
                          </div>
                          {q?.extendedPrice != null && q.extendedLabel && (
                            <div className="markets-ext-line">
                              {q.extendedLabel}{" "}
                              {fmtPrice(q.extendedPrice, q.currency)} (
                              {fmtSigned(q.extendedChangePercent, "%")})
                            </div>
                          )}
                        </td>
                        <td className="num">
                          <span
                            className={`markets-chg-pill ${tone}`}
                            style={{ color }}
                          >
                            {fmtSigned(q?.changePercent, "%")}
                          </span>
                          <div className="markets-chg-sub" style={{ color }}>
                            {fmtSigned(q?.change)}
                          </div>
                        </td>
                        <td className="num">
                          <MiniSparkline
                            points={q?.sparkline ?? []}
                            prevClose={q?.prevClose ?? null}
                            color={color}
                            width={74}
                            height={24}
                          />
                        </td>
                        <td className="num">
                          <div className="markets-src-text">
                            {q?.error ? (
                              <span
                                className="markets-err-text"
                                title={q.error}
                              >
                                获取失败
                              </span>
                            ) : (
                              <>
                                <span>{q?.source ?? "—"}</span>
                                <small>{fmtClock(q?.quoteTime)}</small>
                              </>
                            )}
                          </div>
                        </td>
                        <td className="num">
                          {isCustomWatchlist ? (
                            <button
                              type="button"
                              className="markets-del-btn"
                              title="从自选移除"
                              aria-label={`移除 ${w.symbol}`}
                              onClick={(e) => {
                                e.stopPropagation();
                                removeSymbol(w.symbol);
                              }}
                            >
                              <Trash2 size={13} />
                            </button>
                          ) : (
                            <button
                              type="button"
                              className={`markets-row-star${watchlistSymbols.has(w.symbol) ? " active" : ""}`}
                              title={
                                watchlistSymbols.has(w.symbol)
                                  ? "从自选移除"
                                  : "加入我的自选"
                              }
                              aria-label={
                                watchlistSymbols.has(w.symbol)
                                  ? `从自选移除 ${w.symbol}`
                                  : `加入自选 ${w.symbol}`
                              }
                              onClick={(e) => {
                                e.stopPropagation();
                                toggleWatchlist({
                                  symbol: w.symbol,
                                  name: q?.name || w.name || w.symbol,
                                  market: w.market,
                                });
                              }}
                            >
                              <Star
                                size={13}
                                fill={
                                  watchlistSymbols.has(w.symbol)
                                    ? "currentColor"
                                    : "none"
                                }
                              />
                            </button>
                          )}
                        </td>
                      </tr>
                    );
                  })}
                  {filteredWatchlist.length === 0 && (
                    <tr>
                      <td colSpan={6} className="markets-empty-cell">
                        当前分类暂无自选股，可在右上角搜索代码添加
                      </td>
                    </tr>
                  )}
                </tbody>
              </table>
            </div>
          )}
        </section>

        {/* Right Detail & Interactive Chart Panel (becomes floating corner drawer in Fullscreen) */}
        <section
          className={`markets-card markets-detail-card${
            isFullscreen ? (fsDrawerOpen ? " fs-drawer" : " fs-hidden") : ""
          }`}
        >
          {activeSymbol ? (
            <>
              <div className="markets-detail-head">
                <div>
                  <div className="markets-detail-title">
                    <span className="markets-mkt-tag">
                      {selectedQuote?.market ?? selectedItem?.market ?? "US"}
                    </span>
                    <h2>
                      {selectedQuote?.name ??
                        selectedItem?.name ??
                        activeSymbol}
                    </h2>
                    <code>{activeSymbol}</code>
                    {selectedItem &&
                      !indexItems.some((i) => i.symbol === activeSymbol) && (
                        <button
                          type="button"
                          className={`markets-star-btn${watchlistSymbols.has(activeSymbol) ? " active" : ""}`}
                          onClick={() =>
                            toggleWatchlist({
                              symbol: activeSymbol,
                              name:
                                selectedQuote?.name ||
                                selectedItem.name ||
                                activeSymbol,
                              market:
                                selectedQuote?.market ||
                                selectedItem.market ||
                                "US",
                            })
                          }
                        >
                          <Star
                            size={12}
                            fill={
                              watchlistSymbols.has(activeSymbol)
                                ? "currentColor"
                                : "none"
                            }
                          />
                          <span>
                            {watchlistSymbols.has(activeSymbol)
                              ? "已自选"
                              : "加自选"}
                          </span>
                        </button>
                      )}
                  </div>
                  <div className="markets-detail-meta">
                    <span>{selectedQuote?.exchange ?? "—"}</span>
                    <span>·</span>
                    <span>{selectedQuote?.marketState ?? "—"}</span>
                    <span>·</span>
                    <span>
                      {selectedQuote?.source ?? "—"} ·{" "}
                      {fmtClock(selectedQuote?.quoteTime)}
                    </span>
                  </div>
                </div>
                <div className="markets-detail-head-right">
                  <div className="markets-pill-group">
                    {(
                      [
                        ["1d", "分时"],
                        ["5d", "5日"],
                        ["1mo", "1月"],
                        ["3mo", "3月"],
                        ["1y", "1年"],
                      ] as const
                    ).map(([r, label]) => (
                      <button
                        key={r}
                        type="button"
                        className={`markets-pill-btn${chartRange === r ? " active" : ""}`}
                        onClick={() => setChartRange(r)}
                      >
                        {label}
                      </button>
                    ))}
                  </div>
                  {isFullscreen && (
                    <button
                      type="button"
                      className="markets-drawer-close"
                      title="关闭走势浮窗 (Esc)"
                      aria-label="关闭走势浮窗"
                      onClick={() => setFsDrawerOpen(false)}
                    >
                      <X size={14} />
                    </button>
                  )}
                </div>
              </div>

              <div className="markets-detail-price-row">
                <span className="markets-detail-price">
                  {fmtPrice(selectedQuote?.price, selectedQuote?.currency)}
                </span>
                <span
                  className="markets-detail-chg"
                  style={{
                    color: toneColor(
                      trendTone(selectedQuote?.change, colorScheme),
                    ),
                  }}
                >
                  {fmtSigned(selectedQuote?.change)} (
                  {fmtSigned(selectedQuote?.changePercent, "%")})
                </span>
                {selectedQuote?.extendedPrice != null &&
                  selectedQuote.extendedLabel && (
                    <span className="markets-badge neutral">
                      {selectedQuote.extendedLabel}{" "}
                      {fmtPrice(
                        selectedQuote.extendedPrice,
                        selectedQuote.currency,
                      )}{" "}
                      ({fmtSigned(selectedQuote.extendedChangePercent, "%")})
                    </span>
                  )}
              </div>

              <div className="markets-ohlc-grid">
                <div>
                  <span>今开</span>
                  <strong>
                    {fmtPrice(selectedQuote?.open, selectedQuote?.currency)}
                  </strong>
                </div>
                <div>
                  <span>最高</span>
                  <strong>
                    {fmtPrice(selectedQuote?.high, selectedQuote?.currency)}
                  </strong>
                </div>
                <div>
                  <span>最低</span>
                  <strong>
                    {fmtPrice(selectedQuote?.low, selectedQuote?.currency)}
                  </strong>
                </div>
                <div>
                  <span>昨收</span>
                  <strong>
                    {fmtPrice(
                      selectedQuote?.prevClose,
                      selectedQuote?.currency,
                    )}
                  </strong>
                </div>
                <div>
                  <span>成交量</span>
                  <strong>{fmtVolume(selectedQuote?.volume)}</strong>
                </div>
                {selectedQuote?.marketCap != null &&
                  selectedQuote.marketCap > 0 && (
                    <div>
                      <span>总市值</span>
                      <strong>
                        {fmtMarketCap(
                          selectedQuote.marketCap,
                          selectedQuote.currency,
                        )}
                      </strong>
                    </div>
                  )}
              </div>

              <InteractivePriceChart
                points={
                  chartQuery.data?.points?.length
                    ? chartQuery.data.points
                    : (selectedQuote?.sparkline ?? [])
                }
                prevClose={
                  chartQuery.data?.prevClose ?? selectedQuote?.prevClose ?? null
                }
                currency={
                  chartQuery.data?.currency ?? selectedQuote?.currency ?? "USD"
                }
                range={chartRange}
                color={toneColor(trendTone(selectedQuote?.change, colorScheme))}
                loading={
                  chartQuery.isPending && !selectedQuote?.sparkline?.length
                }
                error={
                  chartQuery.isError
                    ? String(chartQuery.error)
                    : (selectedQuote?.error ?? null)
                }
              />
            </>
          ) : (
            <div className="markets-empty-cell">请选择任意标的查看分时走势</div>
          )}
        </section>
      </div>
    </div>
  );
}

function MiniSparkline({
  points,
  prevClose,
  color,
  width = 62,
  height = 20,
}: {
  points: ChartPoint[];
  prevClose: number | null;
  color: string;
  width?: number;
  height?: number;
}) {
  if (points.length < 2) {
    return (
      <svg width={width} height={height} className="markets-mini-svg">
        <line
          x1={2}
          y1={height / 2}
          x2={width - 2}
          y2={height / 2}
          stroke="var(--line)"
          strokeDasharray="2 2"
        />
      </svg>
    );
  }
  const prices = points.map((p) => p.price);
  const lo = Math.min(...prices, ...(prevClose ? [prevClose] : []));
  const hi = Math.max(...prices, ...(prevClose ? [prevClose] : []));
  const span = Math.max(hi - lo, hi * 0.002, 1e-6);
  const yOf = (v: number) => 2 + (1 - (v - lo) / span) * (height - 4);
  const pts = points
    .map(
      (p, i) =>
        `${(2 + (i / (points.length - 1)) * (width - 4)).toFixed(1)},${yOf(p.price).toFixed(1)}`,
    )
    .join(" ");

  return (
    <svg width={width} height={height} className="markets-mini-svg">
      {prevClose != null && (
        <line
          x1={2}
          y1={yOf(prevClose)}
          x2={width - 2}
          y2={yOf(prevClose)}
          stroke="var(--muted)"
          strokeWidth={0.8}
          strokeDasharray="2 2"
          opacity={0.45}
        />
      )}
      <polyline
        fill="none"
        stroke={color}
        strokeWidth={1.5}
        strokeLinecap="round"
        strokeLinejoin="round"
        points={pts}
      />
    </svg>
  );
}

function InteractivePriceChart({
  points,
  prevClose,
  currency,
  range,
  color,
  loading,
  error,
}: {
  points: ChartPoint[];
  prevClose: number | null;
  currency: string;
  range: ChartRange;
  color: string;
  loading: boolean;
  error: string | null;
}) {
  const [hoverIdx, setHoverIdx] = useState<number | null>(null);

  if (loading) {
    return <div className="markets-chart-empty">正在加载行情曲线…</div>;
  }
  if (points.length < 2) {
    return (
      <div className="markets-chart-empty">
        {error ? `曲线加载失败：${error}` : "当前区间暂无足够成交曲线数据"}
      </div>
    );
  }

  const W = 620;
  const H = 230;
  const padL = 62;
  const padR = 14;
  const padT = 16;
  const padB = 26;
  const plotW = W - padL - padR;
  const plotH = H - padT - padB;

  const prices = points.map((p) => p.price);
  const rawMin = Math.min(...prices, ...(prevClose ? [prevClose] : []));
  const rawMax = Math.max(...prices, ...(prevClose ? [prevClose] : []));
  const padVal = Math.max((rawMax - rawMin) * 0.08, rawMax * 0.001, 1e-4);
  const minP = rawMin - padVal;
  const maxP = rawMax + padVal;

  const xOf = (i: number) => padL + (i / (points.length - 1)) * plotW;
  const yOf = (v: number) => padT + (1 - (v - minP) / (maxP - minP)) * plotH;

  const linePts = points
    .map((p, i) => `${xOf(i).toFixed(1)},${yOf(p.price).toFixed(1)}`)
    .join(" ");
  const areaPts = `${padL},${padT + plotH} ${linePts} ${padL + plotW},${padT + plotH}`;
  const yTicks = [0, 0.33, 0.67, 1].map((r) => minP + (maxP - minP) * r);
  const xTickIndices = Array.from(
    new Set([
      0,
      Math.floor((points.length - 1) * 0.33),
      Math.floor((points.length - 1) * 0.67),
      points.length - 1,
    ]),
  );

  const hoverPt = hoverIdx != null ? points[hoverIdx] : null;
  const baseRef = prevClose ?? points[0].price;
  const hoverDiff = hoverPt ? hoverPt.price - baseRef : 0;
  const hoverPct = baseRef > 0 ? (hoverDiff / baseRef) * 100 : 0;

  return (
    <div
      className="markets-chart-wrap"
      onPointerMove={(e) => {
        const rect = e.currentTarget.getBoundingClientRect();
        const relX = ((e.clientX - rect.left) / Math.max(1, rect.width)) * W;
        const ratio = Math.max(0, Math.min(1, (relX - padL) / plotW));
        const idx = Math.round(ratio * (points.length - 1));
        setHoverIdx((prev) => (prev === idx ? prev : idx));
      }}
      onPointerLeave={() => setHoverIdx(null)}
    >
      <svg
        className="markets-chart-svg"
        viewBox={`0 0 ${W} ${H}`}
        role="img"
        aria-label="标的走势图"
      >
        {yTicks.map((val, i) => {
          const y = yOf(val);
          return (
            <g key={i}>
              <line
                x1={padL}
                y1={y}
                x2={W - padR}
                y2={y}
                stroke="var(--line)"
                strokeDasharray="3 3"
                opacity={0.65}
              />
              <text
                x={padL - 8}
                y={y + 4}
                textAnchor="end"
                fill="var(--muted)"
                fontSize={10.5}
              >
                {val.toFixed(val >= 100 ? 1 : 2)}
              </text>
            </g>
          );
        })}

        {prevClose != null && (
          <line
            x1={padL}
            y1={yOf(prevClose)}
            x2={W - padR}
            y2={yOf(prevClose)}
            stroke="var(--muted)"
            strokeWidth={1}
            strokeDasharray="4 3"
            opacity={0.7}
          />
        )}

        <polygon points={areaPts} fill={color} opacity={0.14} />
        <polyline
          fill="none"
          stroke={color}
          strokeWidth={2}
          strokeLinecap="round"
          strokeLinejoin="round"
          points={linePts}
        />

        {xTickIndices.map((idx) => (
          <text
            key={idx}
            x={xOf(idx)}
            y={H - 6}
            textAnchor={
              idx === 0 ? "start" : idx === points.length - 1 ? "end" : "middle"
            }
            fill="var(--muted)"
            fontSize={10.5}
          >
            {fmtChartTime(points[idx].ts, range)}
          </text>
        ))}

        {hoverIdx != null && hoverPt && (
          <g>
            <line
              x1={xOf(hoverIdx)}
              y1={padT}
              x2={xOf(hoverIdx)}
              y2={padT + plotH}
              stroke="var(--text)"
              strokeWidth={1}
              strokeDasharray="3 2"
              opacity={0.45}
            />
            <circle
              cx={xOf(hoverIdx)}
              cy={yOf(hoverPt.price)}
              r={4.2}
              fill={color}
              stroke="var(--surface)"
              strokeWidth={1.6}
            />
          </g>
        )}
      </svg>

      {hoverIdx != null && hoverPt && (
        <div
          className="markets-chart-tip"
          style={{
            left: `clamp(10px, calc(${((xOf(hoverIdx) / W) * 100).toFixed(
              1,
            )}% - 78px), calc(100% - 180px))`,
          }}
        >
          <small>{fmtChartTime(hoverPt.ts, range)}</small>
          <strong>{fmtPrice(hoverPt.price, currency)}</strong>
          <span>
            {fmtSigned(hoverDiff)} ({fmtSigned(hoverPct, "%")})
          </span>
        </div>
      )}
    </div>
  );
}

const MARKET_ORDER = ["US", "TW", "CN", "HK", "GLOBAL"];
const GROUP_HEADER_H = 24;

function tileWeight(
  item: WatchItem,
  q: StockQuote | undefined,
  mode: TreemapSizeMode,
): number {
  if (mode === "equal") return 1;
  if (mode === "move") {
    return Math.abs(q?.changePercent ?? 0) + 0.6;
  }
  if (mode === "cap") {
    const capUsdB = marketCapUsdBillions(item, q);
    if (capUsdB != null && capUsdB > 0) {
      // Smooth Finviz-style dampening: mega-caps dominate clearly while 500th-ranked names stay clickable.
      return Math.pow(Math.max(capUsdB, 1.5), 0.6);
    }
  }
  const turnover =
    q?.price != null && q.volume != null && q.price > 0 && q.volume > 0
      ? q.price * q.volume
      : 0;
  return turnover > 0 ? Math.pow(turnover, 0.42) : 1;
}

function MarketHeatmap({
  items,
  quoteBySymbol,
  colorScheme,
  groupBy,
  activeSector,
  onSelectSector,
  poolQuery,
  onPoolQueryChange,
  sizeMode,
  onSizeModeChange,
  selectedSymbol,
  onSelect,
  isFullscreen,
  onToggleFullscreen,
  poolId,
  watchlistCount,
  onChangePool,
  onRefresh,
  isRefreshing,
  lastUpdatedStr,
}: {
  items: WatchItem[];
  quoteBySymbol: Map<string, StockQuote>;
  colorScheme: "cn" | "us";
  groupBy: "sector" | "market" | "none";
  activeSector: string | null;
  onSelectSector?: (sector: string) => void;
  poolQuery: string;
  onPoolQueryChange?: (q: string) => void;
  sizeMode: TreemapSizeMode;
  onSizeModeChange: (m: TreemapSizeMode) => void;
  selectedSymbol: string;
  onSelect: (symbol: string) => void;
  isFullscreen: boolean;
  onToggleFullscreen: () => void;
  poolId: string;
  watchlistCount: number;
  onChangePool: (id: string) => void;
  onRefresh: () => void;
  isRefreshing: boolean;
  lastUpdatedStr: string;
}) {
  const [tip, setTip] = useState<{
    x: number;
    y: number;
    item: WatchItem;
    quote?: StockQuote;
  } | null>(null);
  const [viewport, setViewport] = useState(() => ({
    w: typeof window !== "undefined" ? window.innerWidth : 1600,
    h: typeof window !== "undefined" ? window.innerHeight : 960,
  }));

  useEffect(() => {
    if (!isFullscreen) return;
    const sync = () =>
      setViewport({ w: window.innerWidth, h: window.innerHeight });
    sync();
    window.addEventListener("resize", sync);
    return () => window.removeEventListener("resize", sync);
  }, [isFullscreen]);

  const canvasTier =
    items.length > 200 ? "mega" : items.length > 80 ? "large" : "normal";
  const mapW = isFullscreen
    ? Math.max(960, viewport.w)
    : canvasTier === "mega"
      ? 1680
      : canvasTier === "large"
        ? 1500
        : 1400;
  const mapH = isFullscreen
    ? Math.max(520, viewport.h - 56)
    : canvasTier === "mega"
      ? 920
      : canvasTier === "large"
        ? 760
        : 660;

  const breadth = useMemo(() => {
    let rise = 0;
    let fall = 0;
    let flat = 0;
    for (const it of items) {
      const pct = quoteBySymbol.get(it.symbol)?.changePercent;
      if (pct == null || !Number.isFinite(pct)) continue;
      if (pct > 0.005) rise += 1;
      else if (pct < -0.005) fall += 1;
      else flat += 1;
    }
    return { rise, fall, flat, total: items.length };
  }, [items, quoteBySymbol]);

  const layout = useMemo(() => {
    if (items.length === 0) {
      return { groups: [], tiles: [] };
    }
    if (groupBy === "none") {
      const nodes = items.map((item) => ({
        id: item.symbol,
        weight: tileWeight(item, quoteBySymbol.get(item.symbol), sizeMode),
        data: item,
      }));
      return {
        groups: [],
        tiles: layoutSquarified(nodes, 0, 0, mapW, mapH),
      };
    }

    const buckets = new Map<string, WatchItem[]>();
    for (const item of items) {
      const key = groupBy === "sector" ? (item.sector ?? "其他") : item.market;
      const list = buckets.get(key) ?? [];
      list.push(item);
      buckets.set(key, list);
    }

    const entries = Array.from(buckets.entries());
    if (groupBy === "market") {
      entries.sort(
        ([a], [b]) =>
          (MARKET_ORDER.indexOf(a) === -1 ? 99 : MARKET_ORDER.indexOf(a)) -
          (MARKET_ORDER.indexOf(b) === -1 ? 99 : MARKET_ORDER.indexOf(b)),
      );
    }

    const groupNodes = entries.map(([key, list]) => {
      const changes = list
        .map((it) => quoteBySymbol.get(it.symbol)?.changePercent)
        .filter((v): v is number => v != null && Number.isFinite(v));
      const avgPct =
        changes.length > 0
          ? changes.reduce((a, b) => a + b, 0) / changes.length
          : null;
      const label =
        groupBy === "market"
          ? `${MARKET_META[key]?.label ?? key} · ${key}`
          : `${key} (${list.length})`;
      const groupWeight =
        sizeMode === "cap" && groupBy === "sector"
          ? list.reduce(
              (acc, it) =>
                acc + tileWeight(it, quoteBySymbol.get(it.symbol), "cap"),
              0,
            )
          : Math.max(1, list.length);
      return {
        id: key,
        weight: Math.max(1, groupWeight),
        data: { key, label, items: list, avgPct },
      };
    });

    const groupRects = layoutSquarified(groupNodes, 0, 0, mapW, mapH);
    const groups = groupRects.map((gr) => {
      const pad = 2;
      const innerX = gr.x + pad;
      const innerY = gr.y + GROUP_HEADER_H;
      const innerW = Math.max(0, gr.w - pad * 2);
      const innerH = Math.max(0, gr.h - GROUP_HEADER_H - pad);
      const childNodes = gr.data.items.map((item) => ({
        id: item.symbol,
        weight: tileWeight(item, quoteBySymbol.get(item.symbol), sizeMode),
        data: item,
      }));
      const children = layoutSquarified(
        childNodes,
        innerX,
        innerY,
        innerW,
        innerH,
      );
      return { rect: gr, children };
    });

    return { groups, tiles: [] };
  }, [items, quoteBySymbol, groupBy, sizeMode, mapW, mapH]);

  const legendStops =
    colorScheme === "cn"
      ? [
          { label: "-4%", color: "#166534" },
          { label: "-2%", color: "#16a34a" },
          { label: "0%", color: "#3f495a" },
          { label: "+2%", color: "#dc2626" },
          { label: "+4%", color: "#991b1b" },
        ]
      : [
          { label: "-4%", color: "#991b1b" },
          { label: "-2%", color: "#dc2626" },
          { label: "0%", color: "#3f495a" },
          { label: "+2%", color: "#16a34a" },
          { label: "+4%", color: "#166534" },
        ];

  const renderTile = (
    rect: {
      id: string;
      x: number;
      y: number;
      w: number;
      h: number;
      data: WatchItem;
    },
    parentRect?: { x: number; y: number; w: number; h: number },
  ) => {
    const q = quoteBySymbol.get(rect.data.symbol);
    const bg = heatTileColor(q?.changePercent, colorScheme);
    const isSelected = selectedSymbol === rect.data.symbol;
    const baseX = parentRect?.x ?? 0;
    const baseY = parentRect?.y ?? 0;
    const baseW = parentRect?.w ?? mapW;
    const baseH = parentRect?.h ?? mapH;
    const leftPct = ((rect.x - baseX) / Math.max(1, baseW)) * 100;
    const topPct = ((rect.y - baseY) / Math.max(1, baseH)) * 100;
    const widthPct = (rect.w / Math.max(1, baseW)) * 100;
    const heightPct = (rect.h / Math.max(1, baseH)) * 100;

    const micro = rect.w < 26 || rect.h < 18;
    const tiny = !micro && (rect.w < 52 || rect.h < 34);
    const compact = !micro && !tiny && (rect.w < 96 || rect.h < 58);
    const showPrice = rect.w >= 122 && rect.h >= 82;
    const showSub = rect.w >= 88 && rect.h >= 58;
    const showPct = rect.w >= 44 && rect.h >= 30;
    // On tight US tiles (e.g. 503-stock S&P 500), show crisp ticker symbol just like Finviz!
    const primaryLabel =
      rect.data.market === "US" && (tiny || compact)
        ? rect.data.symbol
        : q?.name || rect.data.name || rect.data.symbol;

    return (
      <button
        key={rect.id}
        type="button"
        className={`markets-hm-tile${isSelected ? " active" : ""}${compact ? " compact" : ""}${tiny ? " tiny" : ""}${micro ? " micro" : ""}`}
        style={{
          left: `${leftPct.toFixed(3)}%`,
          top: `${topPct.toFixed(3)}%`,
          width: `${widthPct.toFixed(3)}%`,
          height: `${heightPct.toFixed(3)}%`,
          background: bg,
        }}
        onClick={() => onSelect(rect.data.symbol)}
        onMouseMove={(e) =>
          setTip({
            x: Math.min(e.clientX + 14, window.innerWidth - 290),
            y: Math.min(e.clientY + 14, window.innerHeight - 190),
            item: rect.data,
            quote: q,
          })
        }
        onMouseLeave={() => setTip(null)}
      >
        {!micro && <span className="markets-hm-name">{primaryLabel}</span>}
        {showSub && <span className="markets-hm-sym">{rect.data.symbol}</span>}
        {showPct && (
          <span className="markets-hm-pct">
            {fmtSigned(q?.changePercent, "%")}
          </span>
        )}
        {showPrice && (
          <span className="markets-hm-price">
            {fmtPrice(q?.price, q?.currency)}
          </span>
        )}
      </button>
    );
  };

  return (
    <div className={`markets-hm-shell${isFullscreen ? " fullscreen" : ""}`}>
      <div className="markets-hm-bar">
        <div className="markets-hm-bar-left">
          {isFullscreen && (
            <div className="markets-pill-group">
              <button
                type="button"
                className={`markets-pill-btn${poolId === "watchlist" ? " active" : ""}`}
                onClick={() => onChangePool("watchlist")}
              >
                自选 ({watchlistCount})
              </button>
              {MARKET_PRESETS.map((preset) => (
                <button
                  key={preset.id}
                  type="button"
                  className={`markets-pill-btn${poolId === preset.id ? " active" : ""}`}
                  onClick={() => onChangePool(preset.id)}
                >
                  {preset.label}
                </button>
              ))}
            </div>
          )}
          <div className="markets-pill-group">
            {(
              [
                ["cap", "按市值权重"],
                ["turnover", "按成交额"],
                ["move", "按涨跌幅"],
                ["equal", "等面积"],
              ] as const
            ).map(([m, label]) => (
              <button
                key={m}
                type="button"
                className={`markets-pill-btn${sizeMode === m ? " active" : ""}`}
                onClick={() => onSizeModeChange(m)}
              >
                {label}
              </button>
            ))}
          </div>
          {activeSector && onSelectSector && (
            <button
              type="button"
              className="markets-zoom-back-btn"
              onClick={() => onSelectSector("ALL")}
            >
              ← 返回全部板块（{activeSector}）
            </button>
          )}
          <div className="markets-hm-breadth">
            <span style={{ color: toneColor(trendTone(1, colorScheme)) }}>
              涨 {breadth.rise}
            </span>
            <span>平 {breadth.flat}</span>
            <span style={{ color: toneColor(trendTone(-1, colorScheme)) }}>
              跌 {breadth.fall}
            </span>
            <span>共 {breadth.total} 家</span>
          </div>
          {onPoolQueryChange && (
            <label className="markets-pool-filter">
              <Search size={12} />
              <input
                type="search"
                value={poolQuery}
                onChange={(e) => onPoolQueryChange(e.target.value)}
                placeholder="池内筛选代码/名称/板块…"
                aria-label="在当前股票池内筛选"
              />
            </label>
          )}
        </div>
        <div className="markets-hm-bar-right">
          <div className="markets-hm-legend">
            {legendStops.map((s) => (
              <span key={s.label} className="markets-hm-swatch-wrap">
                <i
                  className="markets-hm-swatch"
                  style={{ background: s.color }}
                />
                <span>{s.label}</span>
              </span>
            ))}
          </div>
          {isFullscreen && (
            <button
              type="button"
              className="markets-fs-btn"
              onClick={onRefresh}
              disabled={isRefreshing}
              title="刷新实时行情"
            >
              <RefreshCw
                size={12}
                className={isRefreshing ? "markets-spin" : undefined}
              />
              <span>{lastUpdatedStr}</span>
            </button>
          )}
          <button
            type="button"
            className={`markets-fs-btn${isFullscreen ? " active" : ""}`}
            onClick={onToggleFullscreen}
            title={isFullscreen ? "退出全屏 (Esc)" : "全屏大盘云图 (F)"}
          >
            {isFullscreen ? (
              <>
                <Minimize2 size={12} />
                <span>退出全屏</span>
              </>
            ) : (
              <>
                <Maximize2 size={12} />
                <span>全屏</span>
              </>
            )}
          </button>
        </div>
      </div>

      {items.length === 0 ? (
        <div className="markets-empty-cell">
          {poolQuery.trim()
            ? `当前股票池中没有匹配「${poolQuery.trim()}」的公司`
            : "当前分类暂无自选股，可在右上角搜索代码添加"}
        </div>
      ) : (
        <div className={`markets-hm-canvas ${canvasTier}`}>
          {groupBy !== "none"
            ? layout.groups.map(({ rect: gr, children }) => {
                const avgColor = toneColor(
                  trendTone(gr.data.avgPct, colorScheme),
                );
                const canZoom = groupBy === "sector" && !!onSelectSector;
                return (
                  <div
                    key={gr.id}
                    className="markets-hm-group"
                    style={{
                      left: `${(gr.x / mapW) * 100}%`,
                      top: `${(gr.y / mapH) * 100}%`,
                      width: `${(gr.w / mapW) * 100}%`,
                      height: `${(gr.h / mapH) * 100}%`,
                    }}
                  >
                    {canZoom ? (
                      <button
                        type="button"
                        className="markets-hm-group-head clickable"
                        title={`点击放大查看「${gr.data.key}」板块`}
                        onClick={() => onSelectSector(gr.data.key)}
                      >
                        <span>{gr.data.label}</span>
                        <span style={{ color: avgColor }}>
                          均 {fmtSigned(gr.data.avgPct, "%")}
                        </span>
                      </button>
                    ) : (
                      <div className="markets-hm-group-head">
                        <span>{gr.data.label}</span>
                        <span style={{ color: avgColor }}>
                          均 {fmtSigned(gr.data.avgPct, "%")}
                        </span>
                      </div>
                    )}
                    <div className="markets-hm-group-body">
                      {children.map((child) =>
                        renderTile(child, {
                          x: gr.x + 2,
                          y: gr.y + GROUP_HEADER_H,
                          w: Math.max(1, gr.w - 4),
                          h: Math.max(1, gr.h - GROUP_HEADER_H - 2),
                        }),
                      )}
                    </div>
                  </div>
                );
              })
            : layout.tiles.map((tile) => renderTile(tile))}
        </div>
      )}

      {tip && (
        <div className="markets-hm-tooltip" style={{ left: tip.x, top: tip.y }}>
          <div className="markets-hm-tip-head">
            <strong>
              {tip.quote?.name || tip.item.name || tip.item.symbol}
            </strong>
            <code>
              {tip.item.sector
                ? `${tip.item.sector} · ${tip.item.symbol}`
                : tip.item.symbol}
            </code>
          </div>
          <div className="markets-hm-tip-price">
            <strong>{fmtPrice(tip.quote?.price, tip.quote?.currency)}</strong>
            <span
              style={{
                color: toneColor(trendTone(tip.quote?.change, colorScheme)),
              }}
            >
              {fmtSigned(tip.quote?.change)} (
              {fmtSigned(tip.quote?.changePercent, "%")})
            </span>
          </div>
          {tip.quote?.extendedPrice != null && tip.quote.extendedLabel && (
            <div className="markets-hm-tip-row">
              {tip.quote.extendedLabel}{" "}
              {fmtPrice(tip.quote.extendedPrice, tip.quote.currency)} (
              {fmtSigned(tip.quote.extendedChangePercent, "%")})
            </div>
          )}
          <div className="markets-hm-tip-row">
            高 {fmtPrice(tip.quote?.high, tip.quote?.currency)} · 低{" "}
            {fmtPrice(tip.quote?.low, tip.quote?.currency)} · 量{" "}
            {fmtVolume(tip.quote?.volume)}
          </div>
          {tip.quote?.marketCap != null && tip.quote.marketCap > 0 && (
            <div className="markets-hm-tip-row">
              总市值 {fmtMarketCap(tip.quote.marketCap, tip.quote.currency)}
            </div>
          )}
          <div className="markets-hm-tip-foot">
            <span>
              {tip.quote?.source ?? "—"} · {fmtClock(tip.quote?.quoteTime)}
            </span>
            <span>点击查看下方走势图</span>
          </div>
        </div>
      )}
    </div>
  );
}
