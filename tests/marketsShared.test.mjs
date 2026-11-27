import test from "node:test";
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import ts from "typescript";

const source = await readFile(
  new URL("../src/markets/marketsShared.ts", import.meta.url),
  "utf8",
);
const code = ts.transpileModule(source, {
  compilerOptions: {
    module: ts.ModuleKind.ESNext,
    target: ts.ScriptTarget.ES2022,
  },
}).outputText;
const {
  tileWeight,
  tileWeights,
  applyPoolEdit,
  addCompanyToPoolEdit,
  removeCompanyFromPoolEdit,
  formatPoolLabel,
  isEquitySymbol,
  fmtFinancialValue,
  fmtFinancialPeriod,
  resolveCurrentMarketCap,
  resolveAthMarketCap,
  computeFinancialPeriods,
  calcFloatingTooltipPos,
} = await import(
  "data:text/javascript;base64," + Buffer.from(code).toString("base64")
);

const item = (symbol) => ({ symbol, name: symbol, market: "US" });
const quote = (symbol, extra) => ({
  symbol,
  currency: "USD",
  price: 100,
  volume: 1e6,
  ...extra,
});

test("a missing market cap never borrows the traded-value scale", () => {
  // AMD from Yahoo: no market cap, but a large traded value.
  const amd = quote("AMD", { marketCap: null, turnover: 1.2e11 });
  assert.equal(tileWeight(item("AMD"), amd, "cap"), null);
  // With a cap it is on the market-cap scale (tens to ~150).
  const nvda = quote("NVDA", { marketCap: 3.99e12 });
  const w = tileWeight(item("NVDA"), nvda, "cap");
  assert.ok(w > 100 && w < 200, String(w));
});

test("missing weights get the median of the others", () => {
  const items = ["NVDA", "AVGO", "MSFT", "AMD"].map(item);
  const quotes = new Map([
    ["NVDA", quote("NVDA", { marketCap: 3.99e12 })],
    ["AVGO", quote("AVGO", { marketCap: 1.5e12 })],
    ["MSFT", quote("MSFT", { marketCap: 3.5e12 })],
    ["AMD", quote("AMD", { marketCap: null, turnover: 1.2e11 })],
  ]);
  const w = tileWeights(items, quotes, "cap");
  assert.ok(w.get("NVDA") > w.get("AMD"), "NVDA outweighs the cap-less AMD");
  assert.equal(w.get("AMD"), w.get("MSFT"), "AMD takes the median weight");
  // Equal mode is unaffected.
  assert.deepEqual(
    [...tileWeights(items, quotes, "equal").values()],
    [1, 1, 1, 1],
  );
});

test("pool edits add, remove, reassign sector, and restore built-in companies cleanly", () => {
  const base = [
    {
      symbol: "NVDA",
      name: "英伟达",
      market: "US",
      sector: "算力芯片",
      capHint: 3500,
    },
    { symbol: "AMD", name: "AMD", market: "US", sector: "算力芯片" },
    { symbol: "TSM", name: "台积电", market: "US", sector: "代工与设备" },
  ];

  // Add a brand-new company
  const edit1 = addCompanyToPoolEdit(base, undefined, {
    symbol: "CRWV",
    name: "CoreWeave",
    market: "US",
    sector: "大模型与云",
  });
  const afterAdd = applyPoolEdit(base, edit1);
  assert.equal(afterAdd.length, 4);
  assert.equal(afterAdd[0].symbol, "CRWV");

  // Remove a built-in company
  const edit2 = removeCompanyFromPoolEdit(base, edit1, "AMD");
  const afterRemove = applyPoolEdit(base, edit2);
  assert.deepEqual(
    afterRemove.map((i) => i.symbol),
    ["CRWV", "NVDA", "TSM"],
  );

  // Move a built-in company to a new sector while preserving capHint
  const edit3 = addCompanyToPoolEdit(base, edit2, {
    symbol: "NVDA",
    name: "英伟达",
    market: "US",
    sector: "AI 核心",
  });
  const afterMove = applyPoolEdit(base, edit3);
  const nvda = afterMove.find((i) => i.symbol === "NVDA");
  assert.equal(nvda.sector, "AI 核心");
  assert.equal(nvda.capHint, 3500);

  // Restoring a removed built-in item and removing the custom addition collapses edit back to undefined
  const restored = addCompanyToPoolEdit(base, edit2, base[1]);
  const clean = removeCompanyFromPoolEdit(base, restored, "CRWV");
  assert.equal(clean, undefined);

  assert.equal(formatPoolLabel("中概股 (53)", 53, false), "中概股 (53)");
  assert.equal(formatPoolLabel("中概股 (53)", 55, true), "中概股 (55)");
  assert.equal(
    formatPoolLabel("台股 Top 150", 148, true),
    "台股 Top 150 (148)",
  );
});

test("resolveCurrentMarketCap and resolveAthMarketCap combine live quotes, ATH and historical snapshots", () => {
  const q = quote("NVDA", { price: 180, marketCap: 4.32e12 });
  const ext = {
    ath: 212.5,
    athTs: 1_760_000_000,
    atl: 0.06,
    atlTs: 1_000_000_000,
    fromAthPct: -15.29,
    fromAtlPct: 300000,
  };
  // Price-scaled ATH market cap: 4.32T * (212.5 / 180) = 5.1T
  const athEst = resolveAthMarketCap(q, ext);
  assert.ok(athEst);
  assert.ok(Math.abs(athEst.value - 5.1e12) < 1e9);
  assert.equal(athEst.estimated, true);
  assert.ok(athEst.fromAthCapPct < -15 && athEst.fromAthCapPct > -16);

  // When an observed historical market-cap peak exceeds the price-scaled estimate
  // (e.g. after share buybacks), the higher observed peak wins as exact.
  const finWithHigherPeak = {
    symbol: "NVDA",
    currency: "USD",
    marketCapCurrency: "USD",
    latestMarketCap: 4.32e12,
    latestMarketCapDate: "2026-11-23",
    peakMarketCap: 5.4e12,
    peakMarketCapDate: "2025-10-31",
    annual: [],
    quarterly: [],
    source: "Yahoo Finance",
    fetchedAt: 1_795_000_000,
  };
  const athObserved = resolveAthMarketCap(q, ext, finWithHigherPeak);
  assert.ok(athObserved);
  assert.equal(athObserved.value, 5.4e12);
  assert.equal(athObserved.estimated, false);
  assert.equal(athObserved.dateLabel, "2025/10");
  assert.equal(Math.round(athObserved.fromAthCapPct), -20);

  // Taiwan stock without live marketCap from TWSE MIS falls back to financials snapshot in TWD.
  const twQuote = quote("2330.TW", {
    currency: "TWD",
    price: 1000,
    marketCap: null,
  });
  const twFin = {
    symbol: "2330.TW",
    currency: "TWD",
    marketCapCurrency: "TWD",
    latestMarketCap: 2.6e13,
    latestMarketCapDate: "2026-11-23",
    peakMarketCap: 2.9e13,
    peakMarketCapDate: "2026-07-31",
    annual: [],
    quarterly: [],
    source: "Yahoo Finance",
    fetchedAt: 1_795_000_000,
  };
  const twCur = resolveCurrentMarketCap(twQuote, twFin);
  assert.equal(twCur.value, 2.6e13);
  assert.equal(twCur.estimated, true);

  // Currency mismatch between snapshot and quote is ignored instead of mixing units.
  const mismatch = resolveCurrentMarketCap(twQuote, {
    ...twFin,
    marketCapCurrency: "USD",
  });
  assert.equal(mismatch, null);
});

test("financial value formatting, period labels, equity detection and margin/YoY calculations", () => {
  assert.equal(isEquitySymbol("NVDA"), true);
  assert.equal(isEquitySymbol("2330.TW"), true);
  assert.equal(isEquitySymbol("^GSPC"), false);
  assert.equal(isEquitySymbol("000300.SS"), false);
  assert.equal(isEquitySymbol("BTC-USD"), false);

  assert.equal(fmtFinancialValue(215.94e9, "USD"), "$215.9B");
  assert.equal(fmtFinancialValue(-2.35e9, "USD"), "-$2.4B");
  assert.equal(fmtFinancialValue(0, "USD"), "$0");
  assert.equal(fmtFinancialValue(1.94e12, "TWD"), "NT$1.94万亿");
  assert.equal(fmtFinancialValue(-1.24e9, "CNY"), "-¥12.40亿");

  assert.equal(fmtFinancialPeriod("2025-12-31", "annual"), "2025");
  assert.equal(fmtFinancialPeriod("2026-01-31", "annual"), "2026/01");
  assert.equal(fmtFinancialPeriod("2026-07-31", "quarterly"), "26/07");

  const annual = computeFinancialPeriods(
    [
      {
        asOfDate: "2024-12-31",
        revenue: 100,
        grossProfit: 60,
        operatingIncome: 40,
        netIncome: 30,
      },
      {
        asOfDate: "2025-12-31",
        revenue: 150,
        grossProfit: 105,
        operatingIncome: 75,
        netIncome: 60,
      },
    ],
    "annual",
  );
  assert.equal(annual[0].revenueYoYPct, null);
  assert.equal(annual[1].revenueYoYPct, 50);
  assert.equal(annual[1].netIncomeYoYPct, 100);
  assert.equal(annual[1].grossMarginPct, 70);
  assert.equal(annual[1].operatingMarginPct, 50);
  assert.equal(annual[1].netMarginPct, 40);

  const quarterly = computeFinancialPeriods(
    [
      { asOfDate: "2025-09-30", revenue: 100, netIncome: -10 },
      { asOfDate: "2026-06-30", revenue: 120, netIncome: 20 },
      { asOfDate: "2026-09-30", revenue: 150, netIncome: 30 },
    ],
    "quarterly",
  );
  assert.equal(quarterly[2].revenueYoYPct, 50);
  assert.equal(quarterly[2].revenueQoQPct, 25);
  // Prior-year net income was negative: YoY percentage is omitted instead of misleading.
  assert.equal(quarterly[2].netIncomeYoYPct, null);
});

test("calcFloatingTooltipPos flips sides near right edge and clamps inside chart bounds", () => {
  // Left side of chart -> opens to the right of anchor
  const rightSide = calcFloatingTooltipPos({
    anchorX: 120,
    anchorY: 120,
    containerW: 720,
    containerH: 260,
    tipW: 200,
    tipH: 120,
  });
  assert.equal(rightSide.side, "right");
  assert.equal(rightSide.left, 134);
  assert.ok(rightSide.top >= 6 && rightSide.top + 120 <= 254);

  // Near right edge -> flips to the left of anchor without clipping
  const leftSide = calcFloatingTooltipPos({
    anchorX: 640,
    anchorY: 20,
    containerW: 720,
    containerH: 260,
    tipW: 200,
    tipH: 120,
  });
  assert.equal(leftSide.side, "left");
  assert.equal(leftSide.left, 426);
  assert.equal(leftSide.top, 6);

  // Near bottom edge -> clamps vertically inside container
  const bottomClamp = calcFloatingTooltipPos({
    anchorX: 200,
    anchorY: 255,
    containerW: 720,
    containerH: 260,
    tipW: 200,
    tipH: 120,
  });
  assert.equal(bottomClamp.top, 134);

  // Very narrow container -> stays clamped cleanly inside [pad, containerW - tipW - pad]
  const narrow = calcFloatingTooltipPos({
    anchorX: 160,
    anchorY: 100,
    containerW: 320,
    containerH: 220,
    tipW: 210,
    tipH: 130,
  });
  assert.ok(narrow.left >= 6 && narrow.left + 210 <= 314);
});
