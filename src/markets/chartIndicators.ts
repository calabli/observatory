import type { ChartPoint } from "./marketsShared";

export function calcSMA(
  closes: number[],
  period: number,
): Array<number | null> {
  const out: Array<number | null> = new Array(closes.length).fill(null);
  if (period <= 0 || closes.length < period) return out;
  let sum = 0;
  for (let i = 0; i < closes.length; i++) {
    sum += closes[i];
    if (i >= period) sum -= closes[i - period];
    if (i >= period - 1) out[i] = sum / period;
  }
  return out;
}

export function calcEMA(closes: number[], period: number): number[] {
  if (closes.length === 0) return [];
  const k = 2 / (period + 1);
  const out: number[] = [closes[0]];
  for (let i = 1; i < closes.length; i++) {
    out.push(closes[i] * k + out[i - 1] * (1 - k));
  }
  return out;
}

export interface MacdSeries {
  dif: number[];
  dea: number[];
  hist: number[];
}

export function calcMACD(
  closes: number[],
  fast = 12,
  slow = 26,
  signal = 9,
): MacdSeries {
  if (closes.length === 0) return { dif: [], dea: [], hist: [] };
  const emaFast = calcEMA(closes, fast);
  const emaSlow = calcEMA(closes, slow);
  const dif = closes.map((_, i) => emaFast[i] - emaSlow[i]);
  const dea = calcEMA(dif, signal);
  const hist = dif.map((v, i) => (v - dea[i]) * 2);
  return { dif, dea, hist };
}

export function calcRSI(
  closes: number[],
  period: number,
): Array<number | null> {
  const out: Array<number | null> = new Array(closes.length).fill(null);
  if (period <= 0 || closes.length <= period) return out;
  let avgGain = 0;
  let avgLoss = 0;
  for (let i = 1; i <= period; i++) {
    const diff = closes[i] - closes[i - 1];
    if (diff > 0) avgGain += diff;
    else avgLoss -= diff;
  }
  avgGain /= period;
  avgLoss /= period;
  out[period] = avgLoss === 0 ? 100 : 100 - 100 / (1 + avgGain / avgLoss);
  for (let i = period + 1; i < closes.length; i++) {
    const diff = closes[i] - closes[i - 1];
    const gain = diff > 0 ? diff : 0;
    const loss = diff < 0 ? -diff : 0;
    avgGain = (avgGain * (period - 1) + gain) / period;
    avgLoss = (avgLoss * (period - 1) + loss) / period;
    out[i] = avgLoss === 0 ? 100 : 100 - 100 / (1 + avgGain / avgLoss);
  }
  return out;
}

export interface KdjSeries {
  k: number[];
  d: number[];
  j: number[];
}

export function calcKDJ(
  points: ChartPoint[],
  period = 9,
  m1 = 3,
  m2 = 3,
): KdjSeries {
  const n = points.length;
  const kArr: number[] = [];
  const dArr: number[] = [];
  const jArr: number[] = [];
  let prevK = 50;
  let prevD = 50;
  for (let i = 0; i < n; i++) {
    const start = Math.max(0, i - period + 1);
    let hi = -Infinity;
    let lo = Infinity;
    for (let j = start; j <= i; j++) {
      const h = points[j].high ?? points[j].price;
      const l = points[j].low ?? points[j].price;
      if (h > hi) hi = h;
      if (l < lo) lo = l;
    }
    const rsv = hi > lo ? ((points[i].price - lo) / (hi - lo)) * 100 : 50;
    const k = ((m1 - 1) / m1) * prevK + (1 / m1) * rsv;
    const d = ((m2 - 1) / m2) * prevD + (1 / m2) * k;
    const jVal = 3 * k - 2 * d;
    kArr.push(k);
    dArr.push(d);
    jArr.push(jVal);
    prevK = k;
    prevD = d;
  }
  return { k: kArr, d: dArr, j: jArr };
}

export interface RangeStats {
  firstPrice: number;
  lastPrice: number;
  change: number;
  changePct: number;
  high: number;
  low: number;
  maxDrawdownPct: number;
  amplitudePct: number;
  avgVolume: number | null;
}

export function calcRangeStats(
  points: ChartPoint[],
  prevClose?: number | null,
): RangeStats | null {
  if (points.length === 0) return null;
  const firstPrice = prevClose && prevClose > 0 ? prevClose : points[0].price;
  const lastPrice = points[points.length - 1].price;
  const change = lastPrice - firstPrice;
  const changePct = firstPrice > 0 ? (change / firstPrice) * 100 : 0;

  let high = -Infinity;
  let low = Infinity;
  let peak = -Infinity;
  let maxDd = 0;
  let volSum = 0;
  let volCount = 0;

  for (const pt of points) {
    const h = pt.high ?? pt.price;
    const l = pt.low ?? pt.price;
    if (h > high) high = h;
    if (l < low) low = l;
    if (h > peak) peak = h;
    if (peak > 0) {
      const dd = ((l - peak) / peak) * 100;
      if (dd < maxDd) maxDd = dd;
    }
    if (pt.volume != null && pt.volume > 0) {
      volSum += pt.volume;
      volCount += 1;
    }
  }

  const amplitudePct = low > 0 ? ((high - low) / low) * 100 : 0;
  return {
    firstPrice,
    lastPrice,
    change,
    changePct,
    high,
    low,
    maxDrawdownPct: maxDd,
    amplitudePct,
    avgVolume: volCount > 0 ? volSum / volCount : null,
  };
}

export interface AlignedBenchmark {
  stockPct: number[];
  benchPct: Array<number | null>;
  stockTotalPct: number;
  benchTotalPct: number | null;
  alphaPct: number | null;
}

/**
 * Align benchmark points to stock timestamps using nearest-neighbor lookup,
 * normalizing both series to 0% at the first point of the visible slice.
 */
export function alignBenchmarkReturns(
  stockPoints: ChartPoint[],
  benchPoints: ChartPoint[],
): AlignedBenchmark | null {
  if (stockPoints.length < 2 || benchPoints.length < 2) return null;
  const stockBase = stockPoints[0].price;
  if (!(stockBase > 0)) return null;

  const stockPct = stockPoints.map(
    (p) => ((p.price - stockBase) / stockBase) * 100,
  );

  // Two-pointer nearest-timestamp alignment (benchPoints is sorted by ts asc)
  let j = 0;
  const matchedBench: Array<number | null> = [];
  for (const sp of stockPoints) {
    while (
      j + 1 < benchPoints.length &&
      Math.abs(benchPoints[j + 1].ts - sp.ts) <=
        Math.abs(benchPoints[j].ts - sp.ts)
    ) {
      j++;
    }
    matchedBench.push(benchPoints[j].price);
  }
  const benchBase = matchedBench.find((v): v is number => v != null && v > 0);
  if (!benchBase) return null;

  const benchPct = matchedBench.map((v) =>
    v != null && v > 0 ? ((v - benchBase) / benchBase) * 100 : null,
  );
  const stockTotalPct = stockPct[stockPct.length - 1];
  const lastBench = [...benchPct].reverse().find((v): v is number => v != null);
  return {
    stockPct,
    benchPct,
    stockTotalPct,
    benchTotalPct: lastBench ?? null,
    alphaPct: lastBench != null ? stockTotalPct - lastBench : null,
  };
}
