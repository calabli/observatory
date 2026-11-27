export interface WatchItem {
  symbol: string;
  name: string;
  market: string;
  sector?: string;
  capHint?: number;
}

export interface PoolEdit {
  added: WatchItem[];
  removed: string[];
}

export interface CustomPool {
  id: string;
  name: string;
  description?: string;
  items: WatchItem[];
}

export interface ResolvedPool {
  id: string;
  /** Display name without count suffix, e.g. "中概股" */
  name: string;
  /** Button label with current constituent count, e.g. "中概股 (53)" */
  label: string;
  description: string;
  items: WatchItem[];
  kind: "preset" | "custom";
  edited: boolean;
  removedItems: WatchItem[];
}

export interface MarketsConfig {
  indices: WatchItem[];
  watchlist: WatchItem[];
  colorScheme: "cn" | "us";
  refreshSec: number;
  poolEdits?: Record<string, PoolEdit>;
  customPools?: CustomPool[];
}

export interface ChartPoint {
  ts: number;
  price: number;
  open?: number | null;
  high?: number | null;
  low?: number | null;
  volume?: number | null;
}

export interface PriceExtremes {
  ath: number;
  athTs?: number | null;
  atl: number;
  atlTs?: number | null;
  fetchedAt: number;
}

export interface ExtremesBatchResponse {
  items: Record<string, PriceExtremes>;
  remaining: number;
  /** Symbols this call tried to fetch, and how many succeeded. */
  attempted: number;
  fetched: number;
}

export interface LiveExtremes {
  ath: number;
  athTs: number | null;
  atl: number;
  atlTs: number | null;
  /** Percentage change from all-time high (<= 0, e.g. -24.5 means down 24.5% from ATH). */
  fromAthPct: number | null;
  /** Percentage change from all-time low (>= 0, e.g. +185.2 means up 185.2% from ATL). */
  fromAtlPct: number | null;
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
  fiftyTwoWeekHigh?: number | null;
  fiftyTwoWeekLow?: number | null;
}

export interface SearchHit {
  symbol: string;
  name: string;
  market: string;
  exchange: string;
}

export type FinancialPeriodMode = "annual" | "quarterly";

export interface FinancialPeriod {
  asOfDate: string;
  revenue?: number | null;
  grossProfit?: number | null;
  operatingIncome?: number | null;
  netIncome?: number | null;
  dilutedEps?: number | null;
  operatingCashFlow?: number | null;
  freeCashFlow?: number | null;
}

export interface StockFinancials {
  symbol: string;
  /** Financial statement reporting currency (e.g. CNY for 0700.HK). */
  currency: string;
  /** Trading currency of market-cap snapshots (e.g. HKD for 0700.HK, USD for TSM). */
  marketCapCurrency?: string | null;
  latestMarketCap?: number | null;
  latestMarketCapDate?: string | null;
  peakMarketCap?: number | null;
  peakMarketCapDate?: string | null;
  trailingPe?: number | null;
  trailingPs?: number | null;
  trailingPb?: number | null;
  annual: FinancialPeriod[];
  quarterly: FinancialPeriod[];
  source: string;
  fetchedAt: number;
}

export interface ComputedFinancialPeriod extends FinancialPeriod {
  grossMarginPct: number | null;
  operatingMarginPct: number | null;
  netMarginPct: number | null;
  revenueYoYPct: number | null;
  netIncomeYoYPct: number | null;
  revenueQoQPct: number | null;
}

export interface CurrentMarketCapInfo {
  value: number;
  estimated: boolean;
  asOfDate: string | null;
}

export interface AthMarketCapInfo {
  value: number;
  dateLabel: string | null;
  fromAthCapPct: number | null;
  estimated: boolean;
  note: string;
}

export type MarketFilter = "ALL" | "US" | "TW" | "CN" | "HK" | "GLOBAL";

export type ChartRange =
  | "1d"
  | "5d"
  | "1mo"
  | "3mo"
  | "6mo"
  | "1y"
  | "2y"
  | "5y"
  | "max";

export const CHART_RANGES: ReadonlyArray<readonly [ChartRange, string]> = [
  ["1d", "分时"],
  ["5d", "5日"],
  ["1mo", "1月"],
  ["3mo", "3月"],
  ["6mo", "6月"],
  ["1y", "1年"],
  ["2y", "2年"],
  ["5y", "5年"],
  ["max", "全部"],
];

export type WatchViewMode = "heatmap" | "list";

export type TreemapSizeMode = "cap" | "turnover" | "move" | "equal";

export type HeatmapMetricMode = "change" | "fromAth" | "fromAtl";

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
  GBP: "£",
  KRW: "₩",
  SGD: "S$",
  AUD: "A$",
  CAD: "CA$",
  CHF: "CHF ",
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

/** Units of each currency per 1 USD. */
export type FxRates = Record<string, number>;

/**
 * Used until live rates arrive (`markets_fx`, refreshed daily) or when they cannot be
 * fetched. Rough values: only for comparing sizes across markets, never for prices.
 */
export const FALLBACK_FX_TO_USD: FxRates = {
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
  fx: FxRates = FALLBACK_FX_TO_USD,
): number | null {
  if (q?.marketCap != null && Number.isFinite(q.marketCap) && q.marketCap > 0) {
    const rate = fx[q.currency] ?? FALLBACK_FX_TO_USD[q.currency] ?? 1;
    return q.marketCap / rate / 1e9;
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
  if (range === "max" || range === "5y") {
    return d.toLocaleDateString("zh-CN", {
      year: "numeric",
      month: "2-digit",
    });
  }
  return d.toLocaleDateString("zh-CN", {
    year: "numeric",
    month: "2-digit",
    day: "2-digit",
  });
}

export function fmtYearMonth(tsSec: number | null | undefined): string {
  if (!tsSec || !Number.isFinite(tsSec) || tsSec <= 0) return "";
  return new Date(tsSec * 1000).toLocaleDateString("zh-CN", {
    year: "numeric",
    month: "2-digit",
  });
}

export function fmtIsoYearMonth(dateStr: string | null | undefined): string {
  if (!dateStr) return "";
  const m = /^(\d{4})-(\d{2})/.exec(dateStr.trim());
  return m ? `${m[1]}/${m[2]}` : "";
}

const CN_INDEX_SYMBOLS = new Set([
  "000001.SS",
  "000300.SS",
  "000016.SS",
  "000905.SS",
  "000688.SS",
  "399001.SZ",
  "399006.SZ",
]);

/** False for market indices, forex pairs and crypto tickers that have no company financials. */
export function isEquitySymbol(symbol: string | null | undefined): boolean {
  const upper = (symbol ?? "").trim().toUpperCase();
  if (!upper) return false;
  if (
    upper.startsWith("^") ||
    upper.includes("=") ||
    upper.endsWith("-USD") ||
    CN_INDEX_SYMBOLS.has(upper)
  ) {
    return false;
  }
  return true;
}

/** Format financial statement values, supporting positive, zero, and negative figures. */
export function fmtFinancialValue(
  v: number | null | undefined,
  currency?: string,
): string {
  if (v == null || !Number.isFinite(v)) return "—";
  const prefix = currency ? (CURRENCY_SYM[currency] ?? "") : "";
  if (v === 0) return `${prefix}0`;
  const sign = v < 0 ? "-" : "";
  const abs = Math.abs(v);
  if (currency === "CNY" || currency === "HKD" || currency === "TWD") {
    if (abs >= 1e12) return `${sign}${prefix}${(abs / 1e12).toFixed(2)}万亿`;
    if (abs >= 1e10) return `${sign}${prefix}${(abs / 1e8).toFixed(0)}亿`;
    if (abs >= 1e8) return `${sign}${prefix}${(abs / 1e8).toFixed(2)}亿`;
    if (abs >= 1e4) return `${sign}${prefix}${(abs / 1e4).toFixed(1)}万`;
    return `${sign}${prefix}${Math.round(abs).toLocaleString("en-US")}`;
  }
  if (abs >= 1e12) return `${sign}${prefix}${(abs / 1e12).toFixed(2)}T`;
  if (abs >= 1e9) return `${sign}${prefix}${(abs / 1e9).toFixed(1)}B`;
  if (abs >= 1e6) return `${sign}${prefix}${(abs / 1e6).toFixed(1)}M`;
  if (abs >= 1e3) return `${sign}${prefix}${(abs / 1e3).toFixed(1)}K`;
  return `${sign}${prefix}${abs.toFixed(2)}`;
}

export function fmtFinancialPeriod(
  asOfDate: string,
  mode: FinancialPeriodMode,
): string {
  const m = /^(\d{4})-(\d{2})/.exec(asOfDate.trim());
  if (!m) return asOfDate;
  const [, year, month] = m;
  if (mode === "annual") {
    return month === "12" ? year : `${year}/${month}`;
  }
  return `${year.slice(2)}/${month}`;
}

function currencyMatches(
  a: string | null | undefined,
  b: string | null | undefined,
): boolean {
  if (!a || !b) return true;
  return a.trim().toUpperCase() === b.trim().toUpperCase();
}

/** Resolve live market cap, falling back to the latest Yahoo snapshot when a feed omits it (e.g. TWSE MIS). */
export function resolveCurrentMarketCap(
  quote: StockQuote | undefined,
  financials?: StockFinancials | null,
): CurrentMarketCapInfo | null {
  if (
    quote?.marketCap != null &&
    Number.isFinite(quote.marketCap) &&
    quote.marketCap > 0
  ) {
    return { value: quote.marketCap, estimated: false, asOfDate: null };
  }
  if (
    financials?.latestMarketCap != null &&
    Number.isFinite(financials.latestMarketCap) &&
    financials.latestMarketCap > 0 &&
    currencyMatches(financials.marketCapCurrency, quote?.currency)
  ) {
    return {
      value: financials.latestMarketCap,
      estimated: true,
      asOfDate: financials.latestMarketCapDate ?? null,
    };
  }
  return null;
}

/**
 * Resolve All-Time High market capitalization in the stock's trading currency.
 *
 * Combines live price-scaled estimate (`currentCap * ATH / price`, split-invariant because ATH
 * comes from split-adjusted history) with directly observed historical market-cap snapshots from
 * Yahoo fundamentals timeseries when available.
 */
export function resolveAthMarketCap(
  quote: StockQuote | undefined,
  ext: LiveExtremes | undefined,
  financials?: StockFinancials | null,
): AthMarketCapInfo | null {
  const currentInfo = resolveCurrentMarketCap(quote, financials);
  const currentCap = currentInfo?.value ?? null;
  const livePrice =
    quote?.price != null && Number.isFinite(quote.price) && quote.price > 0
      ? quote.price
      : null;

  const scaledCap =
    currentCap != null && livePrice != null && ext != null && ext.ath > 0
      ? Math.max(currentCap, currentCap * (ext.ath / livePrice))
      : null;

  const observedPeak =
    financials?.peakMarketCap != null &&
    Number.isFinite(financials.peakMarketCap) &&
    financials.peakMarketCap > 0 &&
    currencyMatches(financials.marketCapCurrency, quote?.currency)
      ? financials.peakMarketCap
      : null;

  if (scaledCap == null && observedPeak == null) return null;

  let value: number;
  let dateLabel: string | null;
  let estimated: boolean;
  let note: string;

  if (observedPeak != null && (scaledCap == null || observedPeak > scaledCap)) {
    value = Math.max(observedPeak, currentCap ?? 0);
    dateLabel =
      fmtIsoYearMonth(financials?.peakMarketCapDate) ||
      fmtYearMonth(ext?.athTs) ||
      null;
    estimated = false;
    note = financials?.peakMarketCapDate
      ? `取自历史市值峰值记录（${financials.peakMarketCapDate}）`
      : "取自历史市值峰值记录";
  } else {
    value = scaledCap!;
    dateLabel = fmtYearMonth(ext?.athTs) || null;
    const atHigh =
      currentCap != null &&
      !currentInfo?.estimated &&
      value <= currentCap * 1.0001;
    estimated = !atHigh;
    note = atHigh
      ? "当前市值即处于历史最高水平"
      : "按历史最高复权价 × 当前总股本推算（若期间存在回购或增发会有小幅偏差）";
  }

  const fromAthCapPct =
    currentCap != null && value > 0
      ? Math.min(0, ((currentCap - value) / value) * 100)
      : (ext?.fromAthPct ?? null);

  return {
    value,
    dateLabel,
    fromAthCapPct,
    estimated,
    note,
  };
}

function parseIsoDayMs(dateStr: string): number | null {
  const m = /^(\d{4})-(\d{2})-(\d{2})/.exec(dateStr.trim());
  if (!m) return null;
  return Date.UTC(Number(m[1]), Number(m[2]) - 1, Number(m[3]));
}

/** Enrich raw annual/quarterly financial rows with margin percentages and YoY/QoQ growth. */
export function computeFinancialPeriods(
  periods: FinancialPeriod[],
  mode: FinancialPeriodMode,
): ComputedFinancialPeriod[] {
  const dayMs = 86_400_000;
  const timestamps = periods.map((p) => parseIsoDayMs(p.asOfDate));

  const pctChange = (
    cur: number | null | undefined,
    prev: number | null | undefined,
  ): number | null => {
    if (
      cur == null ||
      prev == null ||
      !Number.isFinite(cur) ||
      !Number.isFinite(prev)
    ) {
      return null;
    }
    if (prev <= 0) return null;
    return ((cur - prev) / prev) * 100;
  };

  return periods.map((p, i) => {
    const rev = p.revenue ?? null;
    const grossMarginPct =
      rev != null && rev > 0 && p.grossProfit != null
        ? (p.grossProfit / rev) * 100
        : null;
    const operatingMarginPct =
      rev != null && rev > 0 && p.operatingIncome != null
        ? (p.operatingIncome / rev) * 100
        : null;
    const netMarginPct =
      rev != null && rev > 0 && p.netIncome != null
        ? (p.netIncome / rev) * 100
        : null;

    let yoyBase: FinancialPeriod | null = null;
    let qoqBase: FinancialPeriod | null = null;
    const curTs = timestamps[i];

    if (mode === "annual") {
      if (i > 0) {
        const prevTs = timestamps[i - 1];
        if (
          curTs == null ||
          prevTs == null ||
          Math.abs((curTs - prevTs) / dayMs - 365) <= 45
        ) {
          yoyBase = periods[i - 1];
        }
      }
    } else if (curTs != null) {
      for (let j = i - 1; j >= 0; j--) {
        const prevTs = timestamps[j];
        if (prevTs == null) continue;
        const diffDays = (curTs - prevTs) / dayMs;
        if (qoqBase == null && diffDays >= 65 && diffDays <= 115) {
          qoqBase = periods[j];
        }
        if (yoyBase == null && diffDays >= 330 && diffDays <= 400) {
          yoyBase = periods[j];
          break;
        }
      }
    }

    return {
      ...p,
      grossMarginPct,
      operatingMarginPct,
      netMarginPct,
      revenueYoYPct: pctChange(p.revenue, yoyBase?.revenue),
      netIncomeYoYPct: pctChange(p.netIncome, yoyBase?.netIncome),
      revenueQoQPct: pctChange(p.revenue, qoqBase?.revenue),
    };
  });
}

/**
 * Merge live intraday quote with cached ATH/ATL so new highs/lows made today
 * reflect immediately, then compute drawdown from ATH and rebound from ATL.
 */
export function resolveLiveExtremes(
  quote: StockQuote | undefined,
  cached: PriceExtremes | undefined,
): LiveExtremes | null {
  if (!cached || !(cached.ath > 0) || !(cached.atl > 0)) return null;
  let ath = cached.ath;
  let athTs = cached.athTs ?? null;
  let atl = cached.atl;
  let atlTs = cached.atlTs ?? null;

  const liveHigh =
    quote?.high != null && Number.isFinite(quote.high) && quote.high > 0
      ? quote.high
      : null;
  const liveLow =
    quote?.low != null && Number.isFinite(quote.low) && quote.low > 0
      ? quote.low
      : null;
  const livePrice =
    quote?.price != null && Number.isFinite(quote.price) && quote.price > 0
      ? quote.price
      : null;

  const topCandidate = Math.max(liveHigh ?? 0, livePrice ?? 0);
  if (topCandidate > ath) {
    ath = topCandidate;
    athTs = quote?.quoteTime ?? null;
  }
  const lowCandidates = [liveLow, livePrice].filter(
    (v): v is number => v != null && v > 0,
  );
  if (lowCandidates.length > 0) {
    const botCandidate = Math.min(...lowCandidates);
    if (botCandidate < atl) {
      atl = botCandidate;
      atlTs = quote?.quoteTime ?? null;
    }
  }

  const fromAthPct =
    livePrice != null && ath > 0
      ? Math.min(0, ((livePrice - ath) / ath) * 100)
      : null;
  const fromAtlPct =
    livePrice != null && atl > 0
      ? Math.max(0, ((livePrice - atl) / atl) * 100)
      : null;

  return { ath, athTs, atl, atlTs, fromAthPct, fromAtlPct };
}

/** Compact formatting for large rebounds from all-time lows (e.g. +18.4%, +245%, +12.4倍). */
export function fmtExtremePct(v: number | null | undefined): string {
  if (v == null || !Number.isFinite(v)) return "—";
  if (v >= 1000) {
    const mult = v / 100;
    return `+${mult >= 100 ? Math.round(mult).toLocaleString() : mult.toFixed(1)}倍`;
  }
  if (v >= 100) {
    return `+${Math.round(v)}%`;
  }
  const sign = v > 0 ? "+" : "";
  return `${sign}${v.toFixed(2)}%`;
}

export function benchmarkForMarket(market: string): {
  symbol: string;
  name: string;
} {
  switch (market) {
    case "CN":
      return { symbol: "000300.SS", name: "沪深300" };
    case "HK":
      return { symbol: "^HSI", name: "恒生指数" };
    case "TW":
      return { symbol: "^TWII", name: "台湾加权" };
    default:
      return { symbol: "^GSPC", name: "标普500" };
  }
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

/**
 * Tile weight for one stock, or null when the measure the mode sizes by is
 * missing (no market cap, no traded value). Each mode has its own scale, so a
 * missing value must never fall back to another mode's number: a market-cap
 * weight is ~1-150 while a traded-value weight is in the tens of thousands.
 */
export function tileWeight(
  item: WatchItem,
  q: StockQuote | undefined,
  mode: TreemapSizeMode,
  fx: FxRates = FALLBACK_FX_TO_USD,
): number | null {
  if (mode === "equal") return 1;
  if (mode === "move") {
    return Math.abs(q?.changePercent ?? 0) + 0.6;
  }
  if (mode === "cap") {
    const capUsdB = marketCapUsdBillions(item, q, fx);
    // Smooth Finviz-style dampening: mega-caps dominate clearly while 500th-ranked names stay clickable.
    return capUsdB != null && capUsdB > 0
      ? Math.pow(Math.max(capUsdB, 1.5), 0.6)
      : null;
  }
  // Compare in USD so CNY / HKD / TWD amounts are not inflated in mixed pools.
  const t = turnoverOf(q);
  const currency = q?.currency ?? "";
  const rate = fx[currency] ?? FALLBACK_FX_TO_USD[currency] ?? 1;
  const turnoverUsd = t ? t.value / rate : 0;
  return turnoverUsd > 0 ? Math.pow(turnoverUsd, 0.42) : null;
}

/**
 * Weights for a set of stocks: a stock whose measure is missing gets the median
 * weight of the others, so it stays a normal-sized, clickable tile instead of
 * dwarfing or vanishing next to them.
 */
export function tileWeights(
  items: WatchItem[],
  quoteBySymbol: Map<string, StockQuote>,
  mode: TreemapSizeMode,
  fx: FxRates = FALLBACK_FX_TO_USD,
): Map<string, number> {
  const raw = items.map(
    (it) =>
      [
        it.symbol,
        tileWeight(it, quoteBySymbol.get(it.symbol), mode, fx),
      ] as const,
  );
  const known = raw
    .map(([, w]) => w)
    .filter((w): w is number => w != null)
    .sort((a, b) => a - b);
  const median = known.length ? known[Math.floor(known.length / 2)] : 1;
  return new Map(raw.map(([symbol, w]) => [symbol, w ?? median]));
}

function normSym(symbol: string): string {
  return symbol.trim().toUpperCase();
}

/**
 * Apply delta edits (`added` / `removed`) on top of a built-in preset's base constituents.
 * Newly added companies appear at the front so users immediately see them in lists/editors;
 * edited existing companies keep their original position with updated sector/name.
 */
export function applyPoolEdit(
  baseItems: WatchItem[],
  edit: PoolEdit | undefined,
): WatchItem[] {
  if (!edit || (edit.added.length === 0 && edit.removed.length === 0)) {
    return baseItems;
  }
  const removedSet = new Set(edit.removed.map(normSym));
  const overrides = new Map<string, WatchItem>();
  for (const item of edit.added) {
    overrides.set(normSym(item.symbol), item);
  }

  const seen = new Set<string>();
  const kept: WatchItem[] = [];
  for (const base of baseItems) {
    const key = normSym(base.symbol);
    if (removedSet.has(key)) continue;
    seen.add(key);
    const over = overrides.get(key);
    if (over) {
      kept.push({
        ...base,
        ...over,
        capHint: over.capHint ?? base.capHint,
      });
    } else {
      kept.push(base);
    }
  }

  const fresh: WatchItem[] = [];
  for (const item of edit.added) {
    const key = normSym(item.symbol);
    if (!seen.has(key) && !removedSet.has(key)) {
      seen.add(key);
      fresh.push(item);
    }
  }
  return [...fresh, ...kept];
}

function cleanPoolEdit(edit: PoolEdit): PoolEdit | undefined {
  if (edit.added.length === 0 && edit.removed.length === 0) return undefined;
  return edit;
}

/** Add a company to (or update its sector in) a built-in pool's delta edit. */
export function addCompanyToPoolEdit(
  baseItems: WatchItem[],
  edit: PoolEdit | undefined,
  item: WatchItem,
): PoolEdit | undefined {
  const key = normSym(item.symbol);
  const removed = (edit?.removed ?? []).filter((s) => normSym(s) !== key);
  const baseMatch = baseItems.find((b) => normSym(b.symbol) === key);
  const otherAdded = (edit?.added ?? []).filter(
    (a) => normSym(a.symbol) !== key,
  );
  // If restoring a previously removed built-in item with the exact same sector & name, no override needed.
  if (
    baseMatch &&
    (item.sector ?? "") === (baseMatch.sector ?? "") &&
    item.name === baseMatch.name &&
    item.market === baseMatch.market
  ) {
    return cleanPoolEdit({ added: otherAdded, removed });
  }
  return cleanPoolEdit({
    added: [item, ...otherAdded],
    removed,
  });
}

/** Remove a company from a built-in pool's delta edit (and collapse back to undefined if empty). */
export function removeCompanyFromPoolEdit(
  baseItems: WatchItem[],
  edit: PoolEdit | undefined,
  symbol: string,
): PoolEdit | undefined {
  const key = normSym(symbol);
  const added = (edit?.added ?? []).filter((a) => normSym(a.symbol) !== key);
  const inBase = baseItems.some((b) => normSym(b.symbol) === key);
  const prevRemoved = edit?.removed ?? [];
  const removed =
    inBase && !prevRemoved.some((s) => normSym(s) === key)
      ? [...prevRemoved, symbol]
      : prevRemoved;
  return cleanPoolEdit({ added, removed });
}

/** Update a built-in preset label so its displayed count tracks added/removed companies. */
export function formatPoolLabel(
  baseLabel: string,
  actualCount: number,
  edited: boolean,
): string {
  if (!edited) return baseLabel;
  if (/\(\d+\)$/.test(baseLabel)) {
    return baseLabel.replace(/\(\d+\)$/, `(${actualCount})`);
  }
  return `${baseLabel} (${actualCount})`;
}

/**
 * Most stocks one pool or watchlist can hold: the engine accepts at most this many
 * symbols per quote request (`MAX_QUOTE_ITEMS` in native/src/markets.rs), and a
 * larger pool would fail to load entirely.
 */
export const MAX_POOL_SIZE = 600;
export const POOL_LIMIT_HINT = `每个板块最多 ${MAX_POOL_SIZE} 家公司，请先移除一些再添加`;

/**
 * Place a floating chart tooltip beside an anchor point inside a chart container:
 * prefers the right side of the anchor, flips to the left when room runs out,
 * and clamps horizontally & vertically so the tooltip stays inside the plot box.
 */
export function calcFloatingTooltipPos(params: {
  anchorX: number;
  anchorY: number;
  containerW: number;
  containerH: number;
  tipW: number;
  tipH: number;
  gap?: number;
  pad?: number;
}): { left: number; top: number; side: "left" | "right" } {
  const gap = params.gap ?? 14;
  const pad = params.pad ?? 6;
  const maxLeft = Math.max(pad, params.containerW - params.tipW - pad);
  const maxTop = Math.max(pad, params.containerH - params.tipH - pad);

  const rightX = params.anchorX + gap;
  const leftX = params.anchorX - gap - params.tipW;

  let side: "left" | "right";
  let rawLeft: number;
  if (rightX + params.tipW <= params.containerW - pad) {
    side = "right";
    rawLeft = rightX;
  } else if (leftX >= pad) {
    side = "left";
    rawLeft = leftX;
  } else {
    const roomRight = params.containerW - params.anchorX;
    const roomLeft = params.anchorX;
    if (roomRight >= roomLeft) {
      side = "right";
      rawLeft = rightX;
    } else {
      side = "left";
      rawLeft = leftX;
    }
  }

  const left = Math.round(Math.max(pad, Math.min(maxLeft, rawLeft)));
  const rawTop = params.anchorY - params.tipH * 0.42;
  const top = Math.round(Math.max(pad, Math.min(maxTop, rawTop)));
  return { left, top, side };
}
