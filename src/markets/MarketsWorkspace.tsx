import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  addCompanyToPoolEdit,
  applyPoolEdit,
  benchmarkForMarket,
  CHART_RANGES,
  type ChartRange,
  type CustomPool,
  type ExtremesBatchResponse,
  fmtClock,
  fmtExtremePct,
  FALLBACK_FX_TO_USD,
  fmtLastTrade,
  type FxRates,
  fmtMarketCap,
  fmtPrice,
  fmtSigned,
  fmtTurnover,
  fmtVolume,
  fmtYearMonth,
  formatPoolLabel,
  type HeatmapMetricMode,
  isStale,
  type LiveExtremes,
  MARKET_META,
  type MarketFilter,
  type MarketsConfig,
  POOL_ID_KEY,
  type PriceExtremes,
  removeCompanyFromPoolEdit,
  type ResolvedPool,
  resolveLiveExtremes,
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
  ArrowDown,
  ArrowUp,
  ArrowUpDown,
  Check,
  FolderPlus,
  LayoutGrid,
  List,
  Pencil,
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
import { PoolEditorModal } from "./PoolEditorModal";

export function MarketsWorkspace() {
  const qc = useQueryClient();
  const [poolId, setPoolId] = useState<string>(() => {
    try {
      const saved = localStorage.getItem(POOL_ID_KEY);
      if (
        saved &&
        (saved === "watchlist" ||
          saved.startsWith("custom_") ||
          MARKET_PRESETS.some((p) => p.id === saved))
      ) {
        return saved;
      }
    } catch {
      // ignore storage errors
    }
    return "us_sp500";
  });
  const [editorMode, setEditorMode] = useState<"edit" | "create" | null>(null);
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
  const [metricMode, setMetricMode] = useState<HeatmapMetricMode>("change");
  const [listSort, setListSort] = useState<{
    col: "none" | "price" | "changePercent" | "fromAth" | "fromAtl";
    dir: "asc" | "desc";
  }>({ col: "none", dir: "desc" });
  const [extremesMap, setExtremesMap] = useState<Record<string, PriceExtremes>>(
    {},
  );
  const [extremesRemaining, setExtremesRemaining] = useState<number>(0);
  const [selectedSymbol, setSelectedSymbol] = useState<string>("NVDA");
  const [chartRange, setChartRange] = useState<ChartRange>("1y");
  const [showBenchmark, setShowBenchmark] = useState<boolean>(false);
  const [chartModalOpen, setChartModalOpen] = useState<boolean>(false);
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
      if (e.key === "Escape" && chartModalOpen) {
        e.preventDefault();
        setChartModalOpen(false);
        return;
      }
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
  }, [
    chartModalOpen,
    isFullscreen,
    fsDrawerOpen,
    viewMode,
    enterFullscreen,
    exitFullscreen,
  ]);

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
    setFsDrawerOpen(false);
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

  const resolvedPools: ResolvedPool[] = useMemo(() => {
    const edits = config?.poolEdits ?? {};
    const builtIn: ResolvedPool[] = MARKET_PRESETS.map((preset) => {
      const edit = edits[preset.id];
      const items = applyPoolEdit(preset.items, edit);
      const edited = Boolean(
        edit && (edit.added.length > 0 || edit.removed.length > 0),
      );
      const removedSet = new Set(
        (edit?.removed ?? []).map((s) => s.trim().toUpperCase()),
      );
      const removedItems = preset.items.filter((b) =>
        removedSet.has(b.symbol.trim().toUpperCase()),
      );
      const cleanName = preset.label.replace(/\s*\(\d+\)$/, "");
      return {
        id: preset.id,
        name: cleanName,
        label: formatPoolLabel(preset.label, items.length, edited),
        description: preset.description,
        items,
        kind: "preset",
        edited,
        removedItems,
      };
    });
    const custom: ResolvedPool[] = (config?.customPools ?? []).map((cp) => ({
      id: cp.id,
      name: cp.name,
      label: `${cp.name} (${cp.items.length})`,
      description: cp.description || `自定义板块，共 ${cp.items.length} 家公司`,
      items: cp.items,
      kind: "custom",
      edited: false,
      removedItems: [],
    }));
    return [...builtIn, ...custom];
  }, [config?.poolEdits, config?.customPools]);

  const activePool = useMemo(
    () => resolvedPools.find((p) => p.id === poolId) ?? null,
    [resolvedPools, poolId],
  );
  const isCustomWatchlist = poolId === "watchlist";

  // If a previously selected custom pool was deleted (or missing after config load), fall back safely.
  useEffect(() => {
    if (
      config &&
      poolId !== "watchlist" &&
      !resolvedPools.some((p) => p.id === poolId)
    ) {
      setPoolId("us_sp500");
    }
  }, [config, poolId, resolvedPools]);

  const activePoolItems: WatchItem[] = useMemo(
    () => (activePool ? activePool.items : (config?.watchlist ?? [])),
    [activePool, config?.watchlist],
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

  const activeMarketForBench =
    quoteBySymbol.get(activeSymbol)?.market ??
    knownItems.find((i) => i.symbol === activeSymbol)?.market ??
    "US";
  const benchmarkInfo = useMemo(
    () => benchmarkForMarket(activeMarketForBench),
    [activeMarketForBench],
  );
  const canOverlayBench =
    !!activeSymbol && benchmarkInfo.symbol !== activeSymbol;

  const benchChartQuery = useQuery({
    queryKey: ["markets", "chart", benchmarkInfo.symbol, chartRange],
    queryFn: () =>
      usageCall<StockChartDetail>("markets_chart", {
        symbol: benchmarkInfo.symbol,
        range: chartRange,
      }),
    enabled: showBenchmark && canOverlayBench,
    staleTime: 60_000,
    placeholderData: (prev) => prev,
    retry: false,
  });

  // Progressive SQLite-cached ATH/ATL hydration for the visible pool + active symbol.
  const extremesTargetKey = useMemo(() => {
    const syms: string[] = [];
    if (activeSymbol) syms.push(activeSymbol.trim().toUpperCase());
    for (const w of activePoolItems) {
      syms.push(w.symbol.trim().toUpperCase());
    }
    return Array.from(new Set(syms.filter(Boolean))).join(",");
  }, [activeSymbol, activePoolItems]);

  useEffect(() => {
    const symbols = extremesTargetKey
      ? extremesTargetKey.split(",").filter(Boolean)
      : [];
    if (symbols.length === 0) {
      setExtremesRemaining(0);
      return;
    }
    let cancelled = false;
    const hydrate = async () => {
      for (let round = 0; round < 35 && !cancelled; round++) {
        try {
          const res = await usageCall<ExtremesBatchResponse>(
            "markets_extremes",
            { symbols, batchLimit: 18 },
          );
          if (cancelled) return;
          if (res?.items && Object.keys(res.items).length > 0) {
            setExtremesMap((prev) => ({ ...prev, ...res.items }));
          }
          const rem = res?.remaining ?? 0;
          setExtremesRemaining(rem);
          if (rem <= 0) break;
          await new Promise((r) => window.setTimeout(r, 260));
        } catch {
          if (!cancelled) setExtremesRemaining(0);
          break;
        }
      }
    };
    void hydrate();
    return () => {
      cancelled = true;
    };
  }, [extremesTargetKey]);

  const liveExtremesBySymbol = useMemo(() => {
    const map = new Map<string, LiveExtremes>();
    for (const w of knownItems) {
      const key = w.symbol.trim().toUpperCase();
      const ext = resolveLiveExtremes(
        quoteBySymbol.get(w.symbol),
        extremesMap[key],
      );
      if (ext) map.set(key, ext);
    }
    return map;
  }, [knownItems, quoteBySymbol, extremesMap]);

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
    if (!activePool) return [];
    const seen = new Set<string>();
    const out: string[] = [];
    for (const item of activePool.items) {
      if (item.sector && !seen.has(item.sector)) {
        seen.add(item.sector);
        out.push(item.sector);
      }
    }
    return out;
  }, [activePool]);

  const activePoolSymbols = useMemo(
    () => new Set(activePoolItems.map((w) => w.symbol.trim().toUpperCase())),
    [activePoolItems],
  );

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

  const sortedWatchlist = useMemo(() => {
    if (listSort.col === "none") return filteredWatchlist;
    const valOf = (w: WatchItem): number | null => {
      const q = quoteBySymbol.get(w.symbol);
      const ext = liveExtremesBySymbol.get(w.symbol.trim().toUpperCase());
      switch (listSort.col) {
        case "price":
          return q?.price ?? null;
        case "changePercent":
          return q?.changePercent ?? null;
        case "fromAth":
          return ext?.fromAthPct ?? null;
        case "fromAtl":
          return ext?.fromAtlPct ?? null;
        default:
          return null;
      }
    };
    const mult = listSort.dir === "desc" ? -1 : 1;
    return [...filteredWatchlist].sort((a, b) => {
      const va = valOf(a);
      const vb = valOf(b);
      if (va == null && vb == null) return 0;
      if (va == null) return 1;
      if (vb == null) return -1;
      if (va !== vb) return (va - vb) * mult;
      return a.symbol.localeCompare(b.symbol);
    });
  }, [filteredWatchlist, listSort, quoteBySymbol, liveExtremesBySymbol]);

  const toggleListSort = (
    col: "price" | "changePercent" | "fromAth" | "fromAtl",
  ) => {
    setListSort((cur) => {
      if (cur.col !== col) {
        // For "fromAth" (drawdown, negative), asc puts deepest drawdown (-80%) first by default!
        return { col, dir: col === "fromAth" ? "asc" : "desc" };
      }
      if (cur.dir === "desc") return { col, dir: "asc" };
      if (cur.dir === "asc" && col === "fromAth") return { col, dir: "desc" };
      return { col: "none", dir: "desc" };
    });
  };

  const renderSortGlyph = (
    col: "price" | "changePercent" | "fromAth" | "fromAtl",
  ) => {
    if (listSort.col !== col) return <ArrowUpDown size={11} opacity={0.45} />;
    return listSort.dir === "desc" ? (
      <ArrowDown size={11} />
    ) : (
      <ArrowUp size={11} />
    );
  };

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

  const addCompanyToActivePool = (item: WatchItem) => {
    if (!config) return;
    if (isCustomWatchlist) {
      addSymbol(item);
      return;
    }
    if (!activePool) return;
    const sector =
      item.sector?.trim() ||
      (sectorFilter !== "ALL" ? sectorFilter : "") ||
      presetSectors[0] ||
      "自定义";
    const normalized: WatchItem = {
      symbol: item.symbol,
      name: item.name,
      market: item.market,
      sector,
    };

    if (activePool.kind === "preset") {
      const base =
        MARKET_PRESETS.find((p) => p.id === activePool.id)?.items ?? [];
      const prevEdits = config.poolEdits ?? {};
      const nextEdit = addCompanyToPoolEdit(
        base,
        prevEdits[activePool.id],
        normalized,
      );
      const nextEdits = { ...prevEdits };
      if (nextEdit) nextEdits[activePool.id] = nextEdit;
      else delete nextEdits[activePool.id];
      saveCfg.mutate({ ...config, poolEdits: nextEdits });
    } else {
      const nextPools: CustomPool[] = (config.customPools ?? []).map((cp) => {
        if (cp.id !== activePool.id) return cp;
        const key = normalized.symbol.trim().toUpperCase();
        const exists = cp.items.some(
          (i) => i.symbol.trim().toUpperCase() === key,
        );
        const items = exists
          ? cp.items.map((i) =>
              i.symbol.trim().toUpperCase() === key
                ? { ...i, ...normalized }
                : i,
            )
          : [normalized, ...cp.items];
        return { ...cp, items };
      });
      saveCfg.mutate({ ...config, customPools: nextPools });
    }
    setSelectedSymbol(normalized.symbol);
    setSearchText("");
    setSearchOpen(false);
  };

  const removeCompanyFromActivePool = (symbol: string) => {
    if (!config) return;
    if (isCustomWatchlist) {
      removeSymbol(symbol);
      return;
    }
    if (!activePool) return;
    if (activePool.kind === "preset") {
      const base =
        MARKET_PRESETS.find((p) => p.id === activePool.id)?.items ?? [];
      const prevEdits = config.poolEdits ?? {};
      const nextEdit = removeCompanyFromPoolEdit(
        base,
        prevEdits[activePool.id],
        symbol,
      );
      const nextEdits = { ...prevEdits };
      if (nextEdit) nextEdits[activePool.id] = nextEdit;
      else delete nextEdits[activePool.id];
      saveCfg.mutate({ ...config, poolEdits: nextEdits });
    } else {
      const key = symbol.trim().toUpperCase();
      const nextPools: CustomPool[] = (config.customPools ?? []).map((cp) =>
        cp.id === activePool.id
          ? {
              ...cp,
              items: cp.items.filter(
                (i) => i.symbol.trim().toUpperCase() !== key,
              ),
            }
          : cp,
      );
      saveCfg.mutate({ ...config, customPools: nextPools });
    }
  };

  const changeCompanySectorInActivePool = (symbol: string, sector: string) => {
    if (!config || !activePool) return;
    const key = symbol.trim().toUpperCase();
    const existing = activePool.items.find(
      (i) => i.symbol.trim().toUpperCase() === key,
    );
    if (!existing) return;
    addCompanyToActivePool({ ...existing, sector });
  };

  const resetActivePreset = () => {
    if (!config || !activePool || activePool.kind !== "preset") return;
    const nextEdits = { ...(config.poolEdits ?? {}) };
    delete nextEdits[activePool.id];
    saveCfg.mutate({ ...config, poolEdits: nextEdits });
    setSectorFilter("ALL");
  };

  const createCustomPool = (name: string, description: string) => {
    if (!config) return;
    const id = `custom_${Date.now().toString(36)}`;
    const nextPool: CustomPool = { id, name, description, items: [] };
    saveCfg.mutate({
      ...config,
      customPools: [...(config.customPools ?? []), nextPool],
    });
    changePool(id);
    setEditorMode("edit");
  };

  const renameActiveCustomPool = (name: string, description: string) => {
    if (!config || !activePool || activePool.kind !== "custom") return;
    const nextPools = (config.customPools ?? []).map((cp) =>
      cp.id === activePool.id ? { ...cp, name, description } : cp,
    );
    saveCfg.mutate({ ...config, customPools: nextPools });
  };

  const deleteActiveCustomPool = () => {
    if (!config || !activePool || activePool.kind !== "custom") return;
    const nextPools = (config.customPools ?? []).filter(
      (cp) => cp.id !== activePool.id,
    );
    saveCfg.mutate({ ...config, customPools: nextPools });
    setEditorMode(null);
    changePool("us_sp500");
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

      {/* Preset & Custom Pools / Universe Selector */}
      <div className="markets-pool-bar">
        <div className="markets-pill-group">
          <button
            type="button"
            className={`markets-pill-btn${poolId === "watchlist" ? " active" : ""}`}
            onClick={() => changePool("watchlist")}
          >
            我的自选 ({config?.watchlist.length ?? 0})
          </button>
          {resolvedPools.map((pool) => (
            <button
              key={pool.id}
              type="button"
              className={`markets-pill-btn${poolId === pool.id ? " active" : ""}`}
              onClick={() => changePool(pool.id)}
              title={
                pool.edited
                  ? `${pool.description}（已自定义编辑）`
                  : pool.description
              }
            >
              <span>{pool.label}</span>
              {pool.edited && (
                <i className="markets-edited-dot" aria-label="已编辑" />
              )}
            </button>
          ))}
        </div>
        <div className="markets-pool-actions">
          <button
            type="button"
            className="markets-pool-action-btn"
            onClick={() => setEditorMode("edit")}
            disabled={!config}
            title="增删或调整当前板块中的公司"
          >
            <Pencil size={12} />
            <span>编辑板块</span>
          </button>
          <button
            type="button"
            className="markets-pool-action-btn"
            onClick={() => setEditorMode("create")}
            disabled={!config}
            title="新建自定义股票板块"
          >
            <FolderPlus size={12} />
            <span>新建板块</span>
          </button>
        </div>
        {activePool && (
          <span className="markets-pool-desc">{activePool.description}</span>
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
                  placeholder={
                    isCustomWatchlist
                      ? "搜索代码/名称添加自选，如 NVDA、2330、茅台…"
                      : `搜索代码/名称加入「${activePool?.name ?? "当前板块"}」…`
                  }
                  aria-label="搜索并添加公司"
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
                      const exists = isCustomWatchlist
                        ? watchlistSymbols.has(hit.symbol)
                        : activePoolSymbols.has(
                            hit.symbol.trim().toUpperCase(),
                          );
                      return (
                        <button
                          key={hit.symbol}
                          type="button"
                          className="markets-search-item"
                          onClick={() =>
                            isCustomWatchlist
                              ? addSymbol(hit)
                              : exists
                                ? (setSelectedSymbol(hit.symbol),
                                  setSearchOpen(false))
                                : addCompanyToActivePool(hit)
                          }
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
                                <Check size={12} />{" "}
                                {isCustomWatchlist ? "已自选" : "已在板块"}
                              </>
                            ) : (
                              <>
                                <Plus size={12} />{" "}
                                {isCustomWatchlist ? "加自选" : "加板块"}
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
              extremesBySymbol={liveExtremesBySymbol}
              extremesRemaining={extremesRemaining}
              metricMode={metricMode}
              onMetricModeChange={setMetricMode}
              colorScheme={colorScheme}
              groupBy={
                activePool
                  ? sectorFilter === "ALL" && !poolQuery.trim()
                    ? "sector"
                    : "none"
                  : filter === "ALL"
                    ? "market"
                    : "none"
              }
              sectors={presetSectors}
              totalPoolCount={activePoolItems.length}
              activeSector={sectorFilter !== "ALL" ? sectorFilter : null}
              onSelectSector={
                activePool
                  ? (sec) => {
                      setSectorFilter(sec);
                      setFsDrawerOpen(false);
                    }
                  : undefined
              }
              marketFilter={isCustomWatchlist ? filter : undefined}
              onSelectMarketFilter={
                isCustomWatchlist
                  ? (m) => {
                      setFilter(m);
                      setFsDrawerOpen(false);
                    }
                  : undefined
              }
              poolQuery={poolQuery}
              onPoolQueryChange={
                activePoolItems.length > 25 ? setPoolQuery : undefined
              }
              sizeMode={sizeMode}
              onSizeModeChange={setSizeMode}
              selectedSymbol={isFullscreen && !fsDrawerOpen ? "" : activeSymbol}
              onSelect={(sym) => {
                if (isFullscreen) {
                  if (fsDrawerOpen && sym === activeSymbol) {
                    setFsDrawerOpen(false);
                    return;
                  }
                  setSelectedSymbol(sym);
                  setFsDrawerOpen(true);
                } else {
                  setSelectedSymbol(sym);
                }
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
              pools={resolvedPools}
              watchlistCount={config?.watchlist.length ?? 0}
              onChangePool={changePool}
              onEditPool={config ? () => setEditorMode("edit") : undefined}
              onCreatePool={config ? () => setEditorMode("create") : undefined}
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
              {extremesRemaining > 0 && (
                <div className="markets-table-sync-bar">
                  正在后台补齐历史高低点并缓存至本地（剩余 {extremesRemaining}{" "}
                  只）…
                </div>
              )}
              <table className="markets-table">
                <thead>
                  <tr>
                    <th>标的</th>
                    <th
                      className="num sortable"
                      onClick={() => toggleListSort("price")}
                    >
                      <span>最新价 {renderSortGlyph("price")}</span>
                    </th>
                    <th
                      className="num sortable"
                      onClick={() => toggleListSort("changePercent")}
                    >
                      <span>涨跌 {renderSortGlyph("changePercent")}</span>
                    </th>
                    <th
                      className="num sortable"
                      onClick={() => toggleListSort("fromAth")}
                      title="当前价相对历史最高点（ATH）的回撤幅度，点击排序"
                    >
                      <span>距历史高点 {renderSortGlyph("fromAth")}</span>
                    </th>
                    <th
                      className="num sortable"
                      onClick={() => toggleListSort("fromAtl")}
                      title="当前价相对历史最低点（ATL）的反弹幅度，点击排序"
                    >
                      <span>距历史低点 {renderSortGlyph("fromAtl")}</span>
                    </th>
                    <th className="num">分时</th>
                    <th className="num">来源 / 时间</th>
                    <th />
                  </tr>
                </thead>
                <tbody>
                  {sortedWatchlist.map((w) => {
                    const q = quoteBySymbol.get(w.symbol);
                    const ext = liveExtremesBySymbol.get(
                      w.symbol.trim().toUpperCase(),
                    );
                    const tone = trendTone(q?.change, colorScheme);
                    const color = toneColor(tone);
                    const active = activeSymbol === w.symbol;
                    const athTone = trendTone(ext?.fromAthPct, colorScheme);
                    const atlTone = trendTone(ext?.fromAtlPct, colorScheme);
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
                          {ext ? (
                            <div className="markets-ext-cell">
                              <span
                                className="markets-ext-pct"
                                style={{ color: toneColor(athTone) }}
                              >
                                {fmtExtremePct(ext.fromAthPct)}
                              </span>
                              <small>
                                高 {fmtPrice(ext.ath, q?.currency)}
                                {fmtYearMonth(ext.athTs)
                                  ? ` · ${fmtYearMonth(ext.athTs)}`
                                  : ""}
                              </small>
                            </div>
                          ) : (
                            <span className="markets-muted">—</span>
                          )}
                        </td>
                        <td className="num">
                          {ext ? (
                            <div className="markets-ext-cell">
                              <span
                                className="markets-ext-pct"
                                style={{ color: toneColor(atlTone) }}
                              >
                                {fmtExtremePct(ext.fromAtlPct)}
                              </span>
                              <small>
                                低 {fmtPrice(ext.atl, q?.currency)}
                                {fmtYearMonth(ext.atlTs)
                                  ? ` · ${fmtYearMonth(ext.atlTs)}`
                                  : ""}
                              </small>
                            </div>
                          ) : (
                            <span className="markets-muted">—</span>
                          )}
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
                            <span className="markets-row-actions">
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
                              <button
                                type="button"
                                className="markets-del-btn"
                                title={`从「${activePool?.name ?? "当前板块"}」移除`}
                                aria-label={`从板块移除 ${w.symbol}`}
                                onClick={(e) => {
                                  e.stopPropagation();
                                  removeCompanyFromActivePool(w.symbol);
                                }}
                              >
                                <Trash2 size={13} />
                              </button>
                            </span>
                          )}
                        </td>
                      </tr>
                    );
                  })}
                  {sortedWatchlist.length === 0 && (
                    <tr>
                      <td colSpan={8} className="markets-empty-cell">
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
              {isFullscreen && (
                <button
                  type="button"
                  className="markets-drawer-close labeled fs-corner"
                  title="关闭走势图 (Esc，或再次点击该公司方块)"
                  aria-label="关闭走势浮窗"
                  onClick={() => setFsDrawerOpen(false)}
                >
                  <X size={13} />
                  <span>关闭</span>
                </button>
              )}
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
                        <>
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
                          {activePool &&
                            activePoolSymbols.has(
                              activeSymbol.trim().toUpperCase(),
                            ) && (
                              <button
                                type="button"
                                className="markets-star-btn danger-hover"
                                onClick={() =>
                                  removeCompanyFromActivePool(activeSymbol)
                                }
                                title={`将 ${activeSymbol} 从「${activePool.name}」移出`}
                              >
                                <Trash2 size={12} />
                                <span>移出本板块</span>
                              </button>
                            )}
                        </>
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
                  <div className="markets-pill-group markets-range-pills">
                    {CHART_RANGES.map(([r, label]) => (
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
                {(() => {
                  const ext = liveExtremesBySymbol.get(
                    activeSymbol.trim().toUpperCase(),
                  );
                  if (!ext) return null;
                  return (
                    <>
                      <div>
                        <span>
                          历史最高
                          {fmtYearMonth(ext.athTs)
                            ? ` (${fmtYearMonth(ext.athTs)})`
                            : ""}
                        </span>
                        <strong>
                          {fmtPrice(ext.ath, selectedQuote?.currency)}{" "}
                          <em
                            className="markets-ohlc-pct"
                            style={{
                              color: toneColor(
                                trendTone(ext.fromAthPct, colorScheme),
                              ),
                            }}
                          >
                            {fmtExtremePct(ext.fromAthPct)}
                          </em>
                        </strong>
                      </div>
                      <div>
                        <span>
                          历史最低
                          {fmtYearMonth(ext.atlTs)
                            ? ` (${fmtYearMonth(ext.atlTs)})`
                            : ""}
                        </span>
                        <strong>
                          {fmtPrice(ext.atl, selectedQuote?.currency)}{" "}
                          <em
                            className="markets-ohlc-pct"
                            style={{
                              color: toneColor(
                                trendTone(ext.fromAtlPct, colorScheme),
                              ),
                            }}
                          >
                            {fmtExtremePct(ext.fromAtlPct)}
                          </em>
                        </strong>
                      </div>
                    </>
                  );
                })()}
              </div>

              <InteractivePriceChart
                symbol={activeSymbol}
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
                colorScheme={colorScheme}
                loading={
                  chartQuery.isPending && !selectedQuote?.sparkline?.length
                }
                error={
                  chartQuery.isError
                    ? String(chartQuery.error)
                    : (selectedQuote?.error ?? null)
                }
                extremes={
                  liveExtremesBySymbol.get(activeSymbol.trim().toUpperCase()) ??
                  null
                }
                fiftyTwoWeekHigh={chartQuery.data?.fiftyTwoWeekHigh ?? null}
                fiftyTwoWeekLow={chartQuery.data?.fiftyTwoWeekLow ?? null}
                benchmarkName={canOverlayBench ? benchmarkInfo.name : undefined}
                benchmarkPoints={benchChartQuery.data?.points}
                benchmarkLoading={showBenchmark && benchChartQuery.isFetching}
                showBenchmark={showBenchmark && canOverlayBench}
                onToggleBenchmark={
                  canOverlayBench
                    ? () => setShowBenchmark((v) => !v)
                    : undefined
                }
                onExpand={() => setChartModalOpen(true)}
              />
            </>
          ) : (
            <div className="markets-empty-cell">请选择任意标的查看分时走势</div>
          )}
        </section>
      </div>

      {chartModalOpen && activeSymbol && (
        <div
          className="markets-modal-backdrop"
          onClick={() => setChartModalOpen(false)}
        >
          <div
            className="markets-kline-modal"
            onClick={(e) => e.stopPropagation()}
          >
            <div className="markets-kline-modal-head">
              <div className="markets-detail-title">
                <span className="markets-mkt-tag">
                  {selectedQuote?.market ?? selectedItem?.market ?? "US"}
                </span>
                <h2>
                  {selectedQuote?.name ?? selectedItem?.name ?? activeSymbol}
                </h2>
                <code>{activeSymbol}</code>
                <strong className="markets-modal-price">
                  {fmtPrice(selectedQuote?.price, selectedQuote?.currency)}
                </strong>
                <span
                  style={{
                    color: toneColor(
                      trendTone(selectedQuote?.change, colorScheme),
                    ),
                    fontWeight: 600,
                    fontSize: 13,
                  }}
                >
                  {fmtSigned(selectedQuote?.change)} (
                  {fmtSigned(selectedQuote?.changePercent, "%")})
                </span>
              </div>
              <div className="markets-kline-modal-actions">
                <div className="markets-pill-group">
                  {CHART_RANGES.map(([r, label]) => (
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
                <button
                  type="button"
                  className="markets-drawer-close labeled"
                  onClick={() => setChartModalOpen(false)}
                >
                  <X size={13} />
                  <span>关闭</span>
                </button>
              </div>
            </div>

            <InteractivePriceChart
              symbol={activeSymbol}
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
              colorScheme={colorScheme}
              loading={
                chartQuery.isPending && !selectedQuote?.sparkline?.length
              }
              error={
                chartQuery.isError
                  ? String(chartQuery.error)
                  : (selectedQuote?.error ?? null)
              }
              extremes={
                liveExtremesBySymbol.get(activeSymbol.trim().toUpperCase()) ??
                null
              }
              fiftyTwoWeekHigh={chartQuery.data?.fiftyTwoWeekHigh ?? null}
              fiftyTwoWeekLow={chartQuery.data?.fiftyTwoWeekLow ?? null}
              benchmarkName={canOverlayBench ? benchmarkInfo.name : undefined}
              benchmarkPoints={benchChartQuery.data?.points}
              benchmarkLoading={showBenchmark && benchChartQuery.isFetching}
              showBenchmark={showBenchmark && canOverlayBench}
              onToggleBenchmark={
                canOverlayBench ? () => setShowBenchmark((v) => !v) : undefined
              }
              expanded
            />
          </div>
        </div>
      )}

      {editorMode && (
        <PoolEditorModal
          mode={editorMode}
          pool={activePool}
          watchlistItems={config?.watchlist ?? []}
          initialSector={sectorFilter}
          onClose={() => setEditorMode(null)}
          onAddCompany={addCompanyToActivePool}
          onRemoveCompany={removeCompanyFromActivePool}
          onChangeSector={changeCompanySectorInActivePool}
          onRestoreRemoved={(rem) => addCompanyToActivePool(rem)}
          onResetPreset={resetActivePreset}
          onCreatePool={createCustomPool}
          onRenamePool={renameActiveCustomPool}
          onDeletePool={deleteActiveCustomPool}
        />
      )}
    </div>
  );
}
