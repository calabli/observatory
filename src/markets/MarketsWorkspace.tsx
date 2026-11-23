import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  type ChartRange,
  fmtClock,
  FALLBACK_FX_TO_USD,
  fmtLastTrade,
  type FxRates,
  fmtMarketCap,
  fmtPrice,
  fmtSigned,
  fmtTurnover,
  fmtVolume,
  isStale,
  MARKET_META,
  type MarketFilter,
  type MarketsConfig,
  POOL_ID_KEY,
  type SearchHit,
  STALE_HINT,
  type StockChartDetail,
  type StockQuote,
  toneColor,
  type TreemapSizeMode,
  trendTone,
  turnoverOf,
  VIEW_MODE_KEY,
  type WatchItem,
  type WatchViewMode,
} from "./marketsShared";
import { MARKET_PRESETS } from "./presets";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { usageCall } from "../native/usageClient";
import {
  Check,
  LayoutGrid,
  List,
  Plus,
  RefreshCw,
  Search,
  Star,
  Trash2,
  TrendingUp,
  X,
} from "lucide-react";
import { InteractivePriceChart, MiniSparkline } from "./MarketCharts";
import { MarketHeatmap } from "./MarketHeatmap";

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
    // Apply the edit right away so a second quick edit builds on this one
    // instead of the config from before the first save returned.
    onMutate: (next) => {
      const previous = qc.getQueryData<MarketsConfig>(["markets", "config"]);
      qc.setQueryData(["markets", "config"], next);
      return { previous };
    },
    onError: (_error, _next, context) => {
      if (context?.previous)
        qc.setQueryData(["markets", "config"], context.previous);
    },
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

  // Live USD rates for comparing sizes across markets; the built-in values cover
  // the gap before they load or if the source is unreachable.
  const fxQuery = useQuery({
    queryKey: ["markets", "fx"],
    queryFn: () =>
      usageCall<{
        rates: FxRates;
        updatedAt: number | null;
        source: string;
      }>("markets_fx"),
    staleTime: 6 * 3600_000,
    refetchInterval: 6 * 3600_000,
    retry: false,
  });
  const fx = useMemo(
    () => ({ ...FALLBACK_FX_TO_USD, ...(fxQuery.data?.rates ?? {}) }),
    [fxQuery.data],
  );

  // One line in heat-map view (full text on hover) so the map gets the height.
  const sourcesNote =
    "A 股 / 港股走腾讯实时（1–3s）· 台股走 TWSE 官方实时（5s）· 美股走 Yahoo 实时（含盘前/盘后），仅在页面可见时自动刷新 · " +
    (fxQuery.data
      ? `跨币种比较用 ${fxQuery.data.source} 汇率${fxQuery.data.updatedAt ? `（${fmtLastTrade(fxQuery.data.updatedAt)}）` : ""}`
      : "跨币种比较暂用内置近似汇率");

  // The pool's stocks and their quotes are shown as one snapshot: after a pool or
  // view switch the previous complete picture stays on screen until every quote
  // of the new one has arrived, then both swap at once (no half-filled map).
  const viewKey = `${poolId}|${poolKey}|${viewMode}`;
  const [shown, setShown] = useState<{
    key: string;
    items: WatchItem[];
    quotes: StockQuote[];
  } | null>(null);
  useEffect(() => {
    if (activePoolItems.length === 0) {
      setShown({ key: viewKey, items: [], quotes: [] });
    } else if (quotesQuery.data && !quotesQuery.isPlaceholderData) {
      setShown({
        key: viewKey,
        items: activePoolItems,
        quotes: quotesQuery.data,
      });
    }
  }, [
    quotesQuery.data,
    quotesQuery.isPlaceholderData,
    viewKey,
    activePoolItems,
  ]);
  const shownItems = shown?.items ?? activePoolItems;
  const poolPending = shown !== null && shown.key !== viewKey;

  const quoteBySymbol = useMemo(() => {
    const map = new Map<string, StockQuote>();
    for (const q of indicesQuery.data ?? []) {
      map.set(q.symbol, q);
    }
    for (const q of shown?.quotes ?? []) {
      map.set(q.symbol, q);
    }
    return map;
  }, [indicesQuery.data, shown]);

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
    let base = shownItems;
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
  }, [shownItems, isCustomWatchlist, filter, sectorFilter, poolQuery]);

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
    <div
      className={`markets-page${viewMode === "heatmap" ? " heatmap-mode" : ""}`}
    >
      <div className="markets-toolbar">
        <div className="markets-title-block">
          <div className="markets-heading">
            <TrendingUp size={17} />
            <span>全球主要市场实时行情</span>
            <span className="markets-badge neutral">免注册 · 零 Key</span>
          </div>
          <p className="markets-sub" title={sourcesNote}>
            {sourcesNote}
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
              fx={fx}
              swapKey={shown?.key ?? "loading"}
              pending={poolPending}
              loading={shown === null}
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
                            ) : isStale(q) ? (
                              <>
                                <span
                                  className="markets-err-text"
                                  title={STALE_HINT}
                                >
                                  停牌或退市？
                                </span>
                                <small>
                                  最后成交 {fmtLastTrade(q!.quoteTime!)}
                                </small>
                              </>
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
                    {isStale(selectedQuote) && (
                      <span className="markets-err-text">
                        · {STALE_HINT}（最后成交{" "}
                        {fmtLastTrade(selectedQuote!.quoteTime!)}）
                      </span>
                    )}
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
                <div>
                  <span>成交额</span>
                  <strong
                    title={
                      turnoverOf(selectedQuote)?.estimated
                        ? "估算：最新价 × 成交量（该行情源不提供成交额）"
                        : undefined
                    }
                  >
                    {fmtTurnover(selectedQuote)}
                  </strong>
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
