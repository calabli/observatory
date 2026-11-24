import test from "node:test";
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import ts from "typescript";

async function loadTs(relPath) {
  const source = await readFile(new URL(relPath, import.meta.url), "utf8");
  const code = ts.transpileModule(source, {
    compilerOptions: {
      module: ts.ModuleKind.ESNext,
      target: ts.ScriptTarget.ES2022,
    },
  }).outputText;
  return import(
    "data:text/javascript;base64," + Buffer.from(code).toString("base64")
  );
}

const ind = await loadTs("../src/markets/chartIndicators.ts");
const shared = await loadTs("../src/markets/marketsShared.ts");

test("calcSMA, calcMACD, calcRSI, and calcKDJ produce standard values", () => {
  const closes = [10, 12, 14, 16, 18, 20, 19, 21, 22, 24, 23, 25, 26, 28, 30];
  const sma5 = ind.calcSMA(closes, 5);
  assert.equal(sma5[3], null);
  assert.equal(sma5[4], 14);
  assert.equal(sma5[5], 16);

  const macd = ind.calcMACD(closes, 3, 6, 3);
  assert.equal(macd.dif.length, closes.length);
  assert.ok(macd.dif[closes.length - 1] > 0, "uptrend has positive DIF");

  const rsi6 = ind.calcRSI(closes, 6);
  assert.equal(rsi6[5], null);
  assert.ok(rsi6[closes.length - 1] > 60 && rsi6[closes.length - 1] <= 100);

  const pts = closes.map((p, i) => ({
    ts: 1700000000 + i * 86400,
    price: p,
    open: p - 0.5,
    high: p + 1,
    low: p - 1,
    volume: 1000 + i * 100,
  }));
  const kdj = ind.calcKDJ(pts);
  assert.equal(kdj.k.length, pts.length);
  assert.ok(kdj.k[pts.length - 1] > 50);
});

test("calcRangeStats computes max drawdown and alignBenchmarkReturns computes alpha", () => {
  const pts = [
    { ts: 100, price: 100, high: 105, low: 98, volume: 1000 },
    { ts: 200, price: 120, high: 125, low: 110, volume: 2000 },
    { ts: 300, price: 90, high: 100, low: 80, volume: 3000 },
    { ts: 400, price: 110, high: 112, low: 88, volume: 2000 },
  ];
  const stats = ind.calcRangeStats(pts);
  assert.equal(stats.high, 125);
  assert.equal(stats.low, 80);
  // Peak 125 -> low 80 = -36% max drawdown
  assert.ok(Math.abs(stats.maxDrawdownPct - -36) < 0.01);
  assert.equal(stats.avgVolume, 2000);

  const bench = [
    { ts: 100, price: 500 },
    { ts: 200, price: 525 },
    { ts: 300, price: 510 },
    { ts: 400, price: 525 },
  ];
  const aligned = ind.alignBenchmarkReturns(pts, bench);
  assert.ok(Math.abs(aligned.stockTotalPct - 10) < 0.01);
  assert.ok(Math.abs(aligned.benchTotalPct - 5) < 0.01);
  assert.ok(Math.abs(aligned.alphaPct - 5) < 0.01);
});

test("resolveLiveExtremes merges live highs/lows and computes drawdown/rebound", () => {
  const cached = {
    ath: 200,
    athTs: 1600000000,
    atl: 50,
    atlTs: 1500000000,
    fetchedAt: 1700000000,
  };
  const q1 = { price: 150, high: 155, low: 148, quoteTime: 1710000000 };
  const r1 = shared.resolveLiveExtremes(q1, cached);
  assert.equal(r1.ath, 200);
  assert.equal(r1.atl, 50);
  assert.equal(r1.fromAthPct, -25);
  assert.equal(r1.fromAtlPct, 200);

  // Intraday breakout above cached ATH updates ATH dynamically
  const q2 = { price: 220, high: 220, low: 195, quoteTime: 1710000000 };
  const r2 = shared.resolveLiveExtremes(q2, cached);
  assert.equal(r2.ath, 220);
  assert.equal(r2.fromAthPct, 0);
  assert.equal(shared.fmtExtremePct(1250), "+12.5倍");
});
