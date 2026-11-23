export interface WatchItem {
  symbol: string;
  name: string;
  market: string;
  sector?: string;
  capHint?: number;
}

export interface MarketsConfig {
  indices: WatchItem[];
  watchlist: WatchItem[];
  colorScheme: "cn" | "us";
  refreshSec: number;
}

export interface ChartPoint {
  ts: number;
  price: number;
}

export interface StockQuote {
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
  /** Exact traded value in local currency when the feed reports it (Tencent). */
  turnover?: number | null;
  extendedPrice: number | null;
  extendedChangePercent: number | null;
  extendedLabel: string | null;
  marketState: string;
  source: string;
  quoteTime: number | null;
  sparkline: ChartPoint[];
  error: string | null;
}

export interface StockChartDetail {
  symbol: string;
  range: string;
  currency: string;
  prevClose: number | null;
  source: string;
  points: ChartPoint[];
}

export interface SearchHit {
  symbol: string;
  name: string;
  market: string;
  exchange: string;
}

export type MarketFilter = "ALL" | "US" | "TW" | "CN" | "HK" | "GLOBAL";

export type ChartRange = "1d" | "5d" | "1mo" | "3mo" | "1y";

export type WatchViewMode = "heatmap" | "list";

export type TreemapSizeMode = "cap" | "turnover" | "move" | "equal";

export const VIEW_MODE_KEY = "observatory.markets.view-mode";

export const POOL_ID_KEY = "observatory.markets.pool-id";

export const MARKET_META: Record<string, { label: string; short: string }> = {
  US: { label: "美股", short: "US" },
  TW: { label: "台股", short: "TW" },
  CN: { label: "A 股", short: "CN" },
  HK: { label: "港股", short: "HK" },
  GLOBAL: { label: "全球", short: "GL" },
};

export const CURRENCY_SYM: Record<string, string> = {
  USD: "$",
  CNY: "¥",
  HKD: "HK$",
  TWD: "NT$",
  JPY: "¥",
  EUR: "€",
};

export function fmtPrice(
  v: number | null | undefined,
  currency?: string,
): string {
  if (v == null || !Number.isFinite(v)) return "—";
  const prefix = currency ? (CURRENCY_SYM[currency] ?? "") : "";
  const decimals = v >= 1000 ? 2 : v >= 1 ? 2 : 4;
  return `${prefix}${v.toLocaleString("en-US", {
    minimumFractionDigits: decimals,
    maximumFractionDigits: decimals,
  })}`;
}

export function fmtSigned(v: number | null | undefined, suffix = ""): string {
  if (v == null || !Number.isFinite(v)) return "—";
  const sign = v > 0 ? "+" : "";
  return `${sign}${v.toFixed(2)}${suffix}`;
}

export function fmtVolume(v: number | null | undefined): string {
  if (v == null || !Number.isFinite(v) || v <= 0) return "—";
  if (v >= 1_000_000_000) return `${(v / 1_000_000_000).toFixed(2)}B`;
  if (v >= 1_000_000) return `${(v / 1_000_000).toFixed(2)}M`;
  if (v >= 10_000) return `${(v / 10_000).toFixed(1)}万`;
  return v.toLocaleString();
}

export function fmtMarketCap(
  v: number | null | undefined,
  currency?: string,
): string {
  if (v == null || !Number.isFinite(v) || v <= 0) return "—";
  const prefix = currency ? (CURRENCY_SYM[currency] ?? "") : "";
  if (currency === "CNY" || currency === "HKD" || currency === "TWD") {
    if (v >= 1e12) return `${prefix}${(v / 1e12).toFixed(2)}万亿`;
    if (v >= 1e10) return `${prefix}${(v / 1e8).toFixed(0)}亿`;
    if (v >= 1e8) return `${prefix}${(v / 1e8).toFixed(2)}亿`;
    if (v >= 1e4) return `${prefix}${(v / 1e4).toFixed(0)}万`;
    return `${prefix}${Math.round(v).toLocaleString()}`;
  }
  if (v >= 1e12) return `${prefix}${(v / 1e12).toFixed(2)}T`;
  if (v >= 1e9) return `${prefix}${(v / 1e9).toFixed(1)}B`;
  if (v >= 1e6) return `${prefix}${(v / 1e6).toFixed(0)}M`;
  return `${prefix}${v.toLocaleString()}`;
}

export const FX_TO_USD: Record<string, number> = {
  USD: 1,
  CNY: 7.2,
  HKD: 7.8,
  TWD: 32,
  JPY: 150,
  EUR: 0.92,
};

export function marketCapUsdBillions(
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

/**
 * Today's traded value: the feed's exact figure when present (Tencent: A-shares,
 * Hong Kong, US), otherwise price × volume, which is only an estimate because it
 * prices every share at the last trade (TWSE and Yahoo report no amount).
 */
export function turnoverOf(
  q: StockQuote | undefined,
): { value: number; estimated: boolean } | null {
  if (q?.turnover != null && Number.isFinite(q.turnover) && q.turnover > 0) {
    return { value: q.turnover, estimated: false };
  }
  if (q?.price != null && q.volume != null && q.price > 0 && q.volume > 0) {
    return { value: q.price * q.volume, estimated: true };
  }
  return null;
}

export function fmtTurnover(q: StockQuote | undefined): string {
  const t = turnoverOf(q);
  if (!t) return "—";
  return `${t.estimated ? "≈" : ""}${fmtMarketCap(t.value, q?.currency)}`;
}

/**
 * A quote whose last trade is older than this is treated as suspended or delisted.
 * Ten days clears weekends plus the longest exchange holidays (Spring Festival,
 * Golden Week) while still catching names that have stopped trading.
 */
export const STALE_AFTER_DAYS = 10;

export const STALE_HINT = `超过 ${STALE_AFTER_DAYS} 天无成交，可能已停牌或退市`;

export function isStale(
  q: StockQuote | undefined,
  nowMs = Date.now(),
): boolean {
  return (
    q?.quoteTime != null &&
    q.quoteTime > 0 &&
    nowMs / 1000 - q.quoteTime > STALE_AFTER_DAYS * 86_400
  );
}

export function fmtLastTrade(tsSec: number): string {
  return new Date(tsSec * 1000).toLocaleDateString("zh-CN", {
    year: "numeric",
    month: "2-digit",
    day: "2-digit",
  });
}

export function fmtClock(tsSec: number | null | undefined): string {
  if (!tsSec) return "—";
  return new Date(tsSec * 1000).toLocaleTimeString("zh-CN", { hour12: false });
}

export function fmtChartTime(tsSec: number, range: ChartRange): string {
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

export function trendTone(
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

export function toneColor(tone: "up" | "down" | "flat"): string {
  if (tone === "up") return "var(--accent-red)";
  if (tone === "down") return "var(--accent-green)";
  return "var(--muted)";
}

export const MARKET_ORDER = ["US", "TW", "CN", "HK", "GLOBAL"];

export const GROUP_HEADER_H = 24;
