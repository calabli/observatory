import {
  fmtClock,
  fmtLastTrade,
  fmtMarketCap,
  fmtPrice,
  fmtSigned,
  fmtTurnover,
  fmtVolume,
  type FxRates,
  GROUP_HEADER_H,
  isStale,
  MARKET_META,
  MARKET_ORDER,
  type MarketFilter,
  type ResolvedPool,
  STALE_HINT,
  type StockQuote,
  toneColor,
  type TreemapSizeMode,
  trendTone,
  type WatchItem,
  tileWeights,
} from "./marketsShared";
import { useLayoutEffect, useMemo, useRef, useState } from "react";
import { heatTileColor, layoutSquarified } from "./treemap";
import {
  FolderPlus,
  Maximize2,
  Minimize2,
  Pencil,
  RefreshCw,
  Search,
} from "lucide-react";

/** Smallest map height; below it a tiny window scrolls rather than squashing tiles. */
const HEATMAP_MIN_H = 280;
/** Space kept under the map so its border is not flush with the window edge. */
const HEATMAP_BOTTOM_GAP = 20;

export function MarketHeatmap({
  items,
  quoteBySymbol,
  colorScheme,
  groupBy,
  sectors,
  totalPoolCount,
  activeSector,
  onSelectSector,
  marketFilter,
  onSelectMarketFilter,
  poolQuery,
  onPoolQueryChange,
  sizeMode,
  onSizeModeChange,
  selectedSymbol,
  onSelect,
  isFullscreen,
  onToggleFullscreen,
  poolId,
  pools,
  watchlistCount,
  onChangePool,
  onEditPool,
  onCreatePool,
  onRefresh,
  isRefreshing,
  lastUpdatedStr,
  fx,
  swapKey,
  pending,
  loading,
}: {
  items: WatchItem[];
  quoteBySymbol: Map<string, StockQuote>;
  colorScheme: "cn" | "us";
  groupBy: "sector" | "market" | "none";
  sectors: string[];
  totalPoolCount: number;
  activeSector: string | null;
  onSelectSector?: (sector: string) => void;
  marketFilter?: MarketFilter;
  onSelectMarketFilter?: (m: MarketFilter) => void;
  poolQuery: string;
  onPoolQueryChange?: (q: string) => void;
  sizeMode: TreemapSizeMode;
  onSizeModeChange: (m: TreemapSizeMode) => void;
  selectedSymbol: string;
  onSelect: (symbol: string) => void;
  isFullscreen: boolean;
  onToggleFullscreen: () => void;
  poolId: string;
  pools: ResolvedPool[];
  watchlistCount: number;
  onChangePool: (id: string) => void;
  onEditPool?: () => void;
  onCreatePool?: () => void;
  onRefresh: () => void;
  isRefreshing: boolean;
  lastUpdatedStr: string;
  /** Live USD rates for comparing sizes across currencies. */
  fx: FxRates;
  /** Identifies the pool snapshot on screen; a new key fades the map in. */
  swapKey: string;
  /** A new pool's quotes are loading; the current map stays, dimmed. */
  pending: boolean;
  /** Nothing to show yet (first load). */
  loading: boolean;
}) {
  const [tip, setTip] = useState<{
    x: number;
    y: number;
    item: WatchItem;
    quote?: StockQuote;
  } | null>(null);

  // Measure the canvas's real pixel size in both normal view (fills the rest of
  // the window without scrolling) and fullscreen (fills flex: 1 below the bars).
  const canvasRef = useRef<HTMLDivElement>(null);
  const [fit, setFit] = useState<{ w: number; h: number } | null>(null);
  const hasCanvas = items.length > 0;
  useLayoutEffect(() => {
    const el = canvasRef.current;
    if (!el) return;
    const measure = () => {
      const rect = el.getBoundingClientRect();
      const w = Math.floor(rect.width);
      const h = isFullscreen
        ? Math.max(HEATMAP_MIN_H, Math.floor(rect.height))
        : Math.max(
            HEATMAP_MIN_H,
            Math.floor(
              window.innerHeight -
                (rect.top + window.scrollY) -
                HEATMAP_BOTTOM_GAP,
            ),
          );
      setFit((prev) =>
        prev && prev.w === w && prev.h === h ? prev : { w, h },
      );
    };
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(el);
    observer.observe(document.body);
    window.addEventListener("resize", measure);
    return () => {
      observer.disconnect();
      window.removeEventListener("resize", measure);
    };
  }, [isFullscreen, hasCanvas]);

  const canvasTier =
    items.length > 200 ? "mega" : items.length > 80 ? "large" : "normal";
  const mapW = Math.max(1, fit?.w ?? 1400);
  const mapH = Math.max(1, fit?.h ?? 660);

  const breadth = useMemo(() => {
    let rise = 0;
    let fall = 0;
    let flat = 0;
    let stale = 0;
    const now = Date.now();
    for (const it of items) {
      const q = quoteBySymbol.get(it.symbol);
      if (isStale(q, now)) {
        stale += 1;
        continue;
      }
      const pct = q?.changePercent;
      if (pct == null || !Number.isFinite(pct)) continue;
      if (pct > 0.005) rise += 1;
      else if (pct < -0.005) fall += 1;
      else flat += 1;
    }
    return { rise, fall, flat, stale, total: items.length };
  }, [items, quoteBySymbol]);

  const layout = useMemo(() => {
    const weights = tileWeights(items, quoteBySymbol, sizeMode, fx);
    if (items.length === 0) {
      return { groups: [], tiles: [] };
    }
    if (groupBy === "none") {
      const nodes = items.map((item) => ({
        id: item.symbol,
        weight: weights.get(item.symbol) ?? 1,
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
        .map((it) => quoteBySymbol.get(it.symbol))
        .filter((q) => !isStale(q))
        .map((q) => q?.changePercent)
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
          ? list.reduce((acc, it) => acc + (weights.get(it.symbol) ?? 1), 0)
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
        weight: weights.get(item.symbol) ?? 1,
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
  }, [items, quoteBySymbol, groupBy, sizeMode, mapW, mapH, fx]);

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
    const stale = isStale(q);
    const bg = stale ? undefined : heatTileColor(q?.changePercent, colorScheme);
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
        className={`markets-hm-tile${isSelected ? " active" : ""}${compact ? " compact" : ""}${tiny ? " tiny" : ""}${micro ? " micro" : ""}${stale ? " stale" : ""}`}
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
      {isFullscreen && (
        <div className="markets-fs-top-bar">
          <div className="markets-pill-group markets-fs-pools">
            <button
              type="button"
              className={`markets-pill-btn${poolId === "watchlist" ? " active" : ""}`}
              onClick={() => onChangePool("watchlist")}
            >
              自选 ({watchlistCount})
            </button>
            {pools.map((p) => (
              <button
                key={p.id}
                type="button"
                className={`markets-pill-btn${poolId === p.id ? " active" : ""}`}
                onClick={() => onChangePool(p.id)}
              >
                <span>{p.label}</span>
                {p.edited && (
                  <i className="markets-edited-dot" aria-label="已编辑" />
                )}
              </button>
            ))}
          </div>
          <div className="markets-hm-bar-right">
            {onEditPool && (
              <button
                type="button"
                className="markets-fs-btn"
                onClick={onEditPool}
                title="增删或调整当前板块中的公司"
              >
                <Pencil size={12} />
                <span>编辑板块</span>
              </button>
            )}
            {onCreatePool && (
              <button
                type="button"
                className="markets-fs-btn"
                onClick={onCreatePool}
                title="新建自定义股票板块"
              >
                <FolderPlus size={12} />
                <span>新建板块</span>
              </button>
            )}
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
            <button
              type="button"
              className="markets-fs-btn active"
              onClick={onToggleFullscreen}
              title="退出全屏 (Esc)"
            >
              <Minimize2 size={12} />
              <span>退出全屏</span>
            </button>
          </div>
        </div>
      )}

      <div className="markets-hm-bar">
        <div className="markets-hm-bar-left">
          {isFullscreen &&
            (onSelectSector && sectors.length > 0 ? (
              <div className="markets-pill-group markets-fs-sectors">
                <button
                  type="button"
                  className={`markets-pill-btn${!activeSector ? " active" : ""}`}
                  onClick={() => onSelectSector("ALL")}
                >
                  全部板块 ({totalPoolCount})
                </button>
                {sectors.map((sec) => (
                  <button
                    key={sec}
                    type="button"
                    className={`markets-pill-btn${activeSector === sec ? " active" : ""}`}
                    onClick={() => onSelectSector(sec)}
                  >
                    {sec}
                  </button>
                ))}
              </div>
            ) : onSelectMarketFilter ? (
              <div className="markets-pill-group markets-fs-sectors">
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
                    className={`markets-pill-btn${marketFilter === m ? " active" : ""}`}
                    onClick={() => onSelectMarketFilter(m)}
                  >
                    {label}
                  </button>
                ))}
              </div>
            ) : null)}
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
          {!isFullscreen && activeSector && onSelectSector && (
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
            {breadth.stale > 0 && (
              <span title={STALE_HINT}>停 {breadth.stale}</span>
            )}
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
          {!isFullscreen && (
            <button
              type="button"
              className="markets-fs-btn"
              onClick={onToggleFullscreen}
              title="全屏大盘云图 (F)"
            >
              <Maximize2 size={12} />
              <span>全屏</span>
            </button>
          )}
        </div>
      </div>

      {items.length === 0 ? (
        <div className="markets-empty-cell">
          {poolQuery.trim()
            ? `当前股票池中没有匹配「${poolQuery.trim()}」的公司`
            : "当前分类暂无自选股，可在右上角搜索代码添加"}
        </div>
      ) : (
        <div
          ref={canvasRef}
          className={`markets-hm-canvas ${canvasTier}${pending ? " pending" : ""}`}
          style={isFullscreen || !fit ? undefined : { height: fit.h }}
          aria-busy={pending || loading}
        >
          {pending && <div className="markets-hm-progress" aria-hidden />}
          {loading ? (
            <div className="markets-hm-skeleton" aria-label="正在获取行情" />
          ) : (
            // Keyed by the snapshot, so a new pool fades in as a whole; within one
            // pool, tiles keep their keys and glide to new sizes and colors.
            <div className="markets-hm-layer" key={swapKey}>
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
          {isStale(tip.quote) && (
            <div className="markets-hm-tip-row markets-err-text">
              {STALE_HINT}（最后成交 {fmtLastTrade(tip.quote!.quoteTime!)}）
            </div>
          )}
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
            {fmtVolume(tip.quote?.volume)} · 额 {fmtTurnover(tip.quote)}
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
