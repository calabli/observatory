import {
  fmtClock,
  fmtLastTrade,
  fmtMarketCap,
  fmtPrice,
  fmtSigned,
  fmtTurnover,
  fmtVolume,
  FX_TO_USD,
  GROUP_HEADER_H,
  isStale,
  MARKET_META,
  MARKET_ORDER,
  marketCapUsdBillions,
  STALE_HINT,
  type StockQuote,
  toneColor,
  type TreemapSizeMode,
  trendTone,
  turnoverOf,
  type WatchItem,
} from "./marketsShared";
import { useEffect, useMemo, useState } from "react";
import { heatTileColor, layoutSquarified } from "./treemap";
import { MARKET_PRESETS } from "./presets";
import { Maximize2, Minimize2, RefreshCw, Search } from "lucide-react";

export function tileWeight(
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
  // Compare in USD so CNY / HKD / TWD amounts are not inflated in mixed pools.
  const t = turnoverOf(q);
  const turnoverUsd = t ? t.value / (FX_TO_USD[q?.currency ?? ""] ?? 1) : 0;
  return turnoverUsd > 0 ? Math.pow(turnoverUsd, 0.42) : 1;
}

export function MarketHeatmap({
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
